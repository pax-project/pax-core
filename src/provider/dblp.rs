//! dblp, the computer science bibliography: search through its publication
//! search API (`/search/publ/api`, JSON), and lookup by dblp key through its
//! record export (`/rec/<key>.xml`). dblp has no abstracts and no citation
//! graph, so it only adds to search.

use std::time::Duration;

use chrono::Datelike;
use quick_xml::XmlVersion;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use reqwest::StatusCode;
use reqwest::header::RETRY_AFTER;
use serde::Deserialize;

use super::{
    AppliedFilter, CandidateId, CandidateWork, DocType, FilterApplication, FilterKind, Filters,
    Provider, ProviderError, ProviderId, RawPage, SearchOptions, SearchPage, YearRange,
    collect_pages,
};

const SEARCH_API: &str = "https://dblp.org/search/publ/api";
const RECORD_URL: &str = "https://dblp.org/rec";

/// dblp serves at most 1,000 hits per request (`h`).
const MAX_PAGE_SIZE: usize = 1000;

/// How deep pax pages into one query. dblp doesn't document a ceiling for
/// `f`; this cautious one makes a larger result set report itself as
/// truncated rather than risk an error past an undocumented limit.
const MAX_RETRIEVABLE: u64 = 10_000;

/// dblp asks clients to back off when it answers 429; how many times to.
const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF: Duration = Duration::from_secs(2);

/// The earliest year a server-side year filter enumerates from when the
/// range has no start: dblp's oldest records are from the 1930s.
const DBLP_FIRST_YEAR: i32 = 1936;

/// The most years a year filter is applied server-side for. dblp has no
/// range syntax, so a range is sent as one `year:` term per year; a longer
/// one is applied after retrieval instead.
const MAX_SERVER_YEARS: i32 = 30;

pub struct DblpProvider {
    http: reqwest::Client,
}

impl DblpProvider {
    pub fn new() -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .user_agent(format!("pax-core/{}", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        Ok(DblpProvider { http })
    }

    /// GETs `url`, retrying a rate-limited or failed request with backoff,
    /// and returns the body. A 404 is [`ProviderError::NotFound`].
    async fn get_text(&self, url: &str, query: &[(&str, String)]) -> Result<String, ProviderError> {
        let mut backoff = INITIAL_BACKOFF;
        for attempt in 0..=MAX_RETRIES {
            let response = self
                .http
                .get(url)
                .query(query)
                .send()
                .await
                .map_err(|e| ProviderError::Request(e.to_string()))?;
            let status = response.status();
            if status == StatusCode::NOT_FOUND {
                return Err(ProviderError::NotFound);
            }
            let retryable = status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
            if retryable && attempt < MAX_RETRIES {
                let wait = response
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok())
                    .map_or(backoff, Duration::from_secs);
                tokio::time::sleep(wait).await;
                backoff *= 2;
                continue;
            }
            let body = response
                .text()
                .await
                .map_err(|e| ProviderError::Request(e.to_string()))?;
            return check_body(status, body);
        }
        unreachable!("the last attempt always returns")
    }
}

/// A dblp response's body, or why it isn't the data asked for. dblp sits
/// behind a bot check that answers some clients with an HTML challenge page
/// (status 202) instead of the data; that is reported as such rather than as
/// a parse error.
fn check_body(status: StatusCode, body: String) -> Result<String, ProviderError> {
    if body.trim_start().starts_with("<!doctype html") || body.trim_start().starts_with("<html") {
        return Err(ProviderError::Request(format!(
            "dblp answered with a web page (HTTP {status}), probably its bot check, instead of data"
        )));
    }
    if !status.is_success() {
        let excerpt: String = body.chars().take(200).collect();
        return Err(ProviderError::Request(format!(
            "dblp answered {status}: {excerpt}"
        )));
    }
    Ok(body)
}

impl Provider for DblpProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Dblp
    }

    async fn search(&self, query: &str) -> Result<Vec<CandidateWork>, ProviderError> {
        let page = self.search_page(query, &SearchOptions::default()).await?;
        Ok(page.works)
    }

    /// dblp's search has no author-only query in its documented API, so
    /// author search is the default fallback to full-text search; author
    /// names are part of what dblp's search matches.
    async fn search_page(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        let plan = FilterPlan::new(&options.filters);
        let q = plan.query(query);
        let page = collect_pages(
            options,
            MAX_PAGE_SIZE,
            Some(MAX_RETRIEVABLE),
            plan.post_filtered(),
            |offset, size| {
                let request = [
                    ("q", q.clone()),
                    ("format", "json".to_string()),
                    ("h", size.to_string()),
                    ("f", offset.to_string()),
                    ("c", "0".to_string()),
                ];
                let plan = &plan;
                async move {
                    let body = self.get_text(SEARCH_API, &request).await?;
                    let (records, total) = parse_search(&body)?;
                    let fetched = records.len();
                    let works = records
                        .into_iter()
                        .filter(|record| plan.keeps(record))
                        .map(CandidateWork::from)
                        .collect();
                    Ok(RawPage {
                        works,
                        total,
                        fetched,
                    })
                }
            },
        )
        .await?;
        Ok(page.with_filters(plan.applied()))
    }

    /// `native_id` is a dblp key, e.g. `conf/icse/Smith20`.
    async fn get(&self, native_id: &str) -> Result<CandidateWork, ProviderError> {
        let url = format!("{RECORD_URL}/{native_id}.xml");
        let body = self.get_text(&url, &[]).await?;
        parse_record(&body).map(CandidateWork::from)
    }
}

/// One dblp publication, from either a search hit or a record export.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Record {
    key: String,
    title: String,
    authors: Vec<String>,
    year: Option<String>,
    venue: Option<String>,
    doi: Option<String>,
    /// Electronic editions: links to the publication, landing pages mostly.
    ee: Vec<String>,
    /// The search API's type name, e.g. `Journal Articles`. `None` from a
    /// record export, which isn't filtered.
    doc_type: Option<String>,
}

impl From<Record> for CandidateWork {
    fn from(record: Record) -> Self {
        let doi = record
            .doi
            .or_else(|| record.ee.iter().find_map(|ee| doi_from_url(ee)));
        CandidateWork {
            id: CandidateId {
                provider: ProviderId::Dblp,
                native_id: record.key,
            },
            title: plain_title(&record.title),
            authors: record.authors.iter().map(|a| plain_author(a)).collect(),
            publish_date: record.year.unwrap_or_default(),
            doi,
            pdf_url: record.ee.iter().find_map(|ee| pdf_url(ee)),
            venue: record.venue.filter(|v| !v.is_empty()),
            abstract_text: None,
        }
    }
}

/// dblp ends every title with punctuation, adding a period where the title
/// has none; that period isn't part of the title.
fn plain_title(title: &str) -> String {
    let title = title.trim();
    title.strip_suffix('.').unwrap_or(title).to_string()
}

/// dblp tells apart people who share a name with a four-digit suffix
/// (`Daiki Chiba 0001`); the suffix isn't part of the name.
fn plain_author(name: &str) -> String {
    match name.rsplit_once(' ') {
        Some((rest, suffix)) if suffix.len() == 4 && suffix.bytes().all(|b| b.is_ascii_digit()) => {
            rest.to_string()
        }
        _ => name.to_string(),
    }
}

/// The DOI a `doi.org` link resolves.
fn doi_from_url(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("doi.org/")?;
    rest.starts_with("10.").then(|| rest.to_string())
}

/// An electronic edition that is a PDF: an arXiv abstract page (as its PDF
/// link) or a link ending in `.pdf`. dblp's other links are landing pages,
/// which `fetch` can't use.
fn pdf_url(ee: &str) -> Option<String> {
    if let Some((_, id)) = ee.split_once("arxiv.org/abs/") {
        return Some(format!("https://arxiv.org/pdf/{id}"));
    }
    ee.to_ascii_lowercase()
        .ends_with(".pdf")
        .then(|| ee.to_string())
}

/// The search API's type name for a pax document type, where dblp has a
/// type that is the same thing. dblp files books and theses under one type
/// and has no review type, so those aren't filtered.
fn dblp_type(doc_type: DocType) -> Option<&'static str> {
    match doc_type {
        DocType::JournalArticle => Some("Journal Articles"),
        DocType::ConferencePaper => Some("Conference and Workshop Papers"),
        // dblp's informal publications are overwhelmingly CoRR (arXiv).
        DocType::Preprint => Some("Informal and Other Publications"),
        DocType::BookChapter => Some("Parts in Books or Collections"),
        DocType::Dataset => Some("Data and Artifacts"),
        _ => None,
    }
}

/// How a search applies its filters. A year range of at most
/// [`MAX_SERVER_YEARS`] years goes in the query as `year:` terms joined by
/// `|`, dblp's OR; a longer one, and a type, are applied after retrieval,
/// from each hit's `year` and `type`. dblp records carry no language.
struct FilterPlan {
    filters: Filters,
    /// The years to put in the query, when the year filter is server-side.
    server_years: Option<Vec<i32>>,
    doc_type: Option<&'static str>,
}

impl FilterPlan {
    fn new(filters: &Filters) -> Self {
        let this_year = chrono::Utc::now().year();
        FilterPlan {
            filters: filters.clone(),
            server_years: filters
                .years
                .and_then(|years| server_years(years, this_year)),
            doc_type: filters.doc_type.and_then(dblp_type),
        }
    }

    fn query(&self, query: &str) -> String {
        match &self.server_years {
            Some(years) => {
                let terms: Vec<String> = years.iter().map(|y| format!("year:{y}")).collect();
                format!("{query} {}", terms.join("|"))
            }
            None => query.to_string(),
        }
    }

    fn years_after(&self) -> Option<YearRange> {
        self.filters.years.filter(|_| self.server_years.is_none())
    }

    fn post_filtered(&self) -> bool {
        self.years_after().is_some() || self.doc_type.is_some()
    }

    /// Whether `record` passes the filters applied after retrieval. A record
    /// missing the field doesn't match a filter on it.
    fn keeps(&self, record: &Record) -> bool {
        let year_ok = self.years_after().is_none_or(|years| {
            record
                .year
                .as_deref()
                .and_then(|y| y.parse().ok())
                .is_some_and(|y| years.contains(y))
        });
        let type_ok = self
            .doc_type
            .is_none_or(|wanted| record.doc_type.as_deref() == Some(wanted));
        year_ok && type_ok
    }

    fn applied(&self) -> Vec<AppliedFilter> {
        self.filters
            .kinds()
            .into_iter()
            .map(|kind| {
                let application = match kind {
                    FilterKind::Year if self.server_years.is_some() => FilterApplication::Server,
                    FilterKind::Year => FilterApplication::PostRetrieval,
                    FilterKind::DocType if self.doc_type.is_some() => {
                        FilterApplication::PostRetrieval
                    }
                    _ => FilterApplication::NotApplied,
                };
                AppliedFilter::new(kind, application)
            })
            .collect()
    }
}

/// The years to send for `years`, an open end closed at dblp's first year or
/// next year; `None` when that's more than [`MAX_SERVER_YEARS`] years.
fn server_years(years: YearRange, this_year: i32) -> Option<Vec<i32>> {
    let from = years.from.unwrap_or(DBLP_FIRST_YEAR);
    let to = years.to.unwrap_or(this_year + 1);
    (to - from < MAX_SERVER_YEARS).then(|| (from..=to).collect())
}

#[derive(Deserialize)]
struct SearchResponse {
    result: SearchResult,
}

#[derive(Deserialize)]
struct SearchResult {
    hits: Hits,
}

#[derive(Deserialize)]
struct Hits {
    #[serde(rename = "@total")]
    total: String,
    /// Absent when nothing matched.
    #[serde(default)]
    hit: Vec<Hit>,
}

#[derive(Deserialize)]
struct Hit {
    info: Info,
}

/// A hit's fields. dblp's JSON is converted from XML, so a field that can
/// repeat is a single value when it occurs once and a list when it repeats.
#[derive(Deserialize)]
struct Info {
    key: String,
    #[serde(default)]
    title: String,
    authors: Option<Authors>,
    year: Option<String>,
    venue: Option<OneOrMany<String>>,
    doi: Option<String>,
    ee: Option<OneOrMany<String>>,
    #[serde(rename = "type")]
    doc_type: Option<String>,
}

#[derive(Deserialize)]
struct Authors {
    author: OneOrMany<Author>,
}

#[derive(Deserialize)]
struct Author {
    text: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    fn into_vec(self) -> Vec<T> {
        match self {
            OneOrMany::One(one) => vec![one],
            OneOrMany::Many(many) => many,
        }
    }
}

impl From<Info> for Record {
    fn from(info: Info) -> Self {
        Record {
            key: info.key,
            title: info.title,
            authors: info
                .authors
                .map(|a| a.author.into_vec().into_iter().map(|a| a.text).collect())
                .unwrap_or_default(),
            year: info.year,
            venue: info.venue.and_then(|v| v.into_vec().into_iter().next()),
            doi: info.doi.filter(|doi| !doi.is_empty()),
            ee: info.ee.map(OneOrMany::into_vec).unwrap_or_default(),
            doc_type: info.doc_type,
        }
    }
}

/// A search API response's records and the total it reports.
fn parse_search(body: &str) -> Result<(Vec<Record>, u64), ProviderError> {
    let response: SearchResponse = serde_json::from_str(body)
        .map_err(|e| ProviderError::Request(format!("unexpected dblp response: {e}")))?;
    let hits = response.result.hits;
    let total = hits
        .total
        .parse()
        .map_err(|_| ProviderError::Request(format!("unexpected dblp total: {:?}", hits.total)))?;
    let records = hits.hit.into_iter().map(|hit| hit.info.into()).collect();
    Ok((records, total))
}

/// The record in a `/rec/<key>.xml` export: `<dblp>` around one publication
/// element (`article`, `inproceedings`, …) whose `key` attribute is the dblp
/// key. Markup inside a title (`<i>`, `<sub>`, …) is kept as its text.
fn parse_record(body: &str) -> Result<Record, ProviderError> {
    let bad = |what: String| ProviderError::Request(format!("unexpected dblp record: {what}"));
    let mut reader = Reader::from_str(body);
    let mut record: Option<Record> = None;
    let mut depth = 0usize;
    // The field being read, and the depth its element opened at.
    let mut field: Option<(Vec<u8>, usize)> = None;
    let mut text = String::new();
    loop {
        match reader.read_event().map_err(|e| bad(e.to_string()))? {
            Event::Start(e) => {
                depth += 1;
                match (depth, &record, &field) {
                    // `<dblp>` is depth 1, the publication depth 2.
                    (2, None, _) => {
                        let key = e
                            .try_get_attribute("key")
                            .map_err(|e| bad(e.to_string()))?
                            .ok_or_else(|| bad("no key attribute".into()))?
                            .normalized_value(XmlVersion::Explicit1_0)
                            .map_err(|e| bad(e.to_string()))?
                            .into_owned();
                        record = Some(Record {
                            key,
                            ..Record::default()
                        });
                    }
                    (3, Some(_), None) => {
                        field = Some((e.local_name().as_ref().to_vec(), depth));
                        text.clear();
                    }
                    _ => {}
                }
            }
            Event::Text(t) if field.is_some() => {
                text.push_str(&t.decode().map_err(|e| bad(e.to_string()))?);
            }
            Event::CData(t) if field.is_some() => {
                text.push_str(&t.decode().map_err(|e| bad(e.to_string()))?);
            }
            Event::GeneralRef(r) if field.is_some() => {
                match r.resolve_char_ref().map_err(|e| bad(e.to_string()))? {
                    Some(c) => text.push(c),
                    None => {
                        let name = r.decode().map_err(|e| bad(e.to_string()))?;
                        let resolved = resolve_predefined_entity(&name)
                            .ok_or_else(|| bad(format!("undeclared entity &{name};")))?;
                        text.push_str(resolved);
                    }
                }
            }
            Event::End(_) => {
                let closes_field = field.as_ref().is_some_and(|(_, at)| *at == depth);
                if let (true, Some((name, _)), Some(record)) =
                    (closes_field, &field, record.as_mut())
                {
                    let value = text.trim().to_string();
                    match name.as_slice() {
                        b"title" => record.title = value,
                        b"author" => record.authors.push(value),
                        b"year" => record.year = Some(value),
                        b"journal" | b"booktitle" if record.venue.is_none() => {
                            record.venue = Some(value)
                        }
                        b"ee" => record.ee.push(value),
                        _ => {}
                    }
                    field = None;
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    record.ok_or_else(|| bad("no publication in it".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Recorded dblp responses. `search-last-page.json` is a recorded search
    // (offset 1000 of 1089 hits) cut down to five of its 89 hits;
    // `search-toc.json` is a whole recorded response; the `rec-*.xml` files
    // are recorded `/rec/<key>.xml` exports.
    const SEARCH_LAST_PAGE: &str = include_str!("../../tests/fixtures/dblp/search-last-page.json");
    const SEARCH_TOC: &str = include_str!("../../tests/fixtures/dblp/search-toc.json");
    const REC_ARTICLE: &str = include_str!("../../tests/fixtures/dblp/rec-article.xml");
    const REC_INPROCEEDINGS: &str = include_str!("../../tests/fixtures/dblp/rec-inproceedings.xml");
    const REC_NO_DOI: &str = include_str!("../../tests/fixtures/dblp/rec-no-doi.xml");

    fn works(body: &str) -> (Vec<CandidateWork>, u64) {
        let (records, total) = parse_search(body).unwrap();
        (
            records.into_iter().map(CandidateWork::from).collect(),
            total,
        )
    }

    #[test]
    fn a_search_page_gives_its_hits_and_the_total() {
        let (works, total) = works(SEARCH_LAST_PAGE);
        assert_eq!(total, 1089);
        assert_eq!(works.len(), 5);
        let first = &works[0];
        assert_eq!(first.id.to_string(), "dblp:conf/saint/ChibaTMG12");
        assert_eq!(
            first.title,
            "Detecting Malicious Websites by Learning IP Address Features"
        );
        assert_eq!(
            first.authors,
            [
                "Daiki Chiba",
                "Kazuhiro Tobe",
                "Tatsuya Mori",
                "Shigeki Goto"
            ]
        );
        assert_eq!(first.publish_date, "2012");
        assert_eq!(first.doi.as_deref(), Some("10.1109/SAINT.2012.14"));
        assert_eq!(first.venue.as_deref(), Some("SAINT"));
        // A doi.org link is a landing page, not a PDF.
        assert_eq!(first.pdf_url, None);
        assert_eq!(first.abstract_text, None);
    }

    #[test]
    fn a_single_author_is_an_object_not_a_list() {
        let (works, _) = works(SEARCH_LAST_PAGE);
        let egan = works
            .iter()
            .find(|w| w.id.native_id == "conf/issa/Egan11")
            .unwrap();
        assert_eq!(egan.authors, ["Shaun Egan"]);
        assert_eq!(egan.doi, None);
        assert_eq!(
            egan.pdf_url.as_deref(),
            Some("http://icsa.cs.up.ac.za/issa/2011/Proceedings/Full/49_Paper.pdf")
        );
    }

    #[test]
    fn a_book_without_venue_or_doi_still_parses() {
        let (works, _) = works(SEARCH_LAST_PAGE);
        let book = works
            .iter()
            .find(|w| w.id.native_id == "books/daglib/0027496")
            .unwrap();
        assert_eq!(book.venue, None);
        assert_eq!(book.doi, None);
        assert_eq!(book.pdf_url, None);
    }

    #[test]
    fn a_corr_record_links_its_arxiv_pdf() {
        let (works, _) = works(SEARCH_LAST_PAGE);
        let corr = works
            .iter()
            .find(|w| w.id.native_id == "journals/corr/abs-1109-1074")
            .unwrap();
        assert_eq!(corr.venue.as_deref(), Some("CoRR"));
        assert_eq!(
            corr.pdf_url.as_deref(),
            Some("https://arxiv.org/pdf/1109.1074")
        );
        // Its arXiv id is then an identity key, so it merges with arXiv's record.
        assert!(
            crate::identity::identity_keys(corr)
                .contains(&crate::IdentityKey::Arxiv("1109.1074".into()))
        );
    }

    #[test]
    fn a_whole_result_set_parses_with_non_ascii_names() {
        let (works, total) = works(SEARCH_TOC);
        assert_eq!(total, 16);
        assert_eq!(works.len(), 16);
        assert!(
            works
                .iter()
                .any(|w| w.authors.iter().any(|a| a == "Thomas H. Møller"))
        );
        assert!(
            works
                .iter()
                .all(|w| w.venue.as_deref() == Some("Data Knowl. Eng."))
        );
    }

    #[test]
    fn no_hits_is_an_empty_page() {
        // dblp leaves `hit` out when nothing matched.
        let body = r#"{"result":{"query":"zzzz*","status":{"@code":"200","text":"OK"},
            "hits":{"@total":"0","@computed":"0","@sent":"0","@first":"0"}}}"#;
        let (works, total) = works(body);
        assert_eq!(total, 0);
        assert!(works.is_empty());
    }

    #[test]
    fn a_venue_or_ee_list_parses() {
        let body = r#"{"result":{"hits":{"@total":"1","hit":[{"info":{
            "key":"conf/x/Y20","title":"T.","venue":["A","B"],"year":"2020",
            "ee":["https://example.org/landing","https://doi.org/10.1/x"]}}]}}}"#;
        let (works, _) = works(body);
        assert_eq!(works[0].venue.as_deref(), Some("A"));
        assert_eq!(works[0].doi.as_deref(), Some("10.1/x"));
        assert!(works[0].authors.is_empty());
    }

    #[test]
    fn a_bot_check_page_is_reported_as_one() {
        let page = "<!doctype html><html><head><title>Making sure you're not a bot!</title>";
        let err = check_body(StatusCode::ACCEPTED, page.to_string()).unwrap_err();
        assert!(err.to_string().contains("bot check"), "{err}");
        assert!(parse_search(page).is_err());
    }

    #[test]
    fn a_record_export_gives_the_record() {
        let work = CandidateWork::from(parse_record(REC_ARTICLE).unwrap());
        assert_eq!(work.id.to_string(), "dblp:journals/access/AkhtarM18");
        assert_eq!(
            work.title,
            "Threat of Adversarial Attacks on Deep Learning in Computer Vision: A Survey"
        );
        assert_eq!(work.authors, ["Naveed Akhtar", "Ajmal S. Mian"]);
        assert_eq!(work.publish_date, "2018");
        assert_eq!(work.venue.as_deref(), Some("IEEE Access"));
        assert_eq!(work.doi.as_deref(), Some("10.1109/ACCESS.2018.2807385"));
    }

    #[test]
    fn a_conference_record_takes_its_booktitle_as_venue() {
        let work = CandidateWork::from(parse_record(REC_INPROCEEDINGS).unwrap());
        assert_eq!(work.id.native_id, "conf/iccv/YeLX0CLZZMW19");
        assert_eq!(
            work.title,
            "Adversarial Robustness vs. Model Compression, or Both?"
        );
        assert_eq!(work.authors.len(), 10);
        assert!(work.authors.contains(&"Sijia Liu".to_string()));
        assert_eq!(work.venue.as_deref(), Some("ICCV"));
        assert_eq!(work.doi.as_deref(), Some("10.1109/ICCV.2019.00020"));
    }

    #[test]
    fn a_record_without_a_doi_has_none() {
        let work = CandidateWork::from(parse_record(REC_NO_DOI).unwrap());
        assert_eq!(work.id.native_id, "conf/icml/Croce020a");
        assert_eq!(work.doi, None);
        assert_eq!(work.pdf_url, None);
        assert_eq!(work.authors, ["Francesco Croce", "Matthias Hein"]);
    }

    #[test]
    fn a_record_title_keeps_markup_text_and_character_references() {
        let body = r#"<?xml version="1.0" encoding="US-ASCII"?>
<dblp>
<article key="journals/x/Y20" mdate="2020-01-01">
<author>Jan G&#246;bel</author>
<title>On <i>k</i>-anonymity &amp; more.</title>
<year>2020</year>
<journal>X</journal>
</article></dblp>"#;
        let work = CandidateWork::from(parse_record(body).unwrap());
        assert_eq!(work.authors, ["Jan Göbel"]);
        assert_eq!(work.title, "On k-anonymity & more");
    }

    #[test]
    fn short_year_ranges_go_in_the_query() {
        let plan = FilterPlan::new(&Filters::default().with_years("2019-2021".parse().unwrap()));
        assert_eq!(plan.query("actor"), "actor year:2019|year:2020|year:2021");
        assert!(!plan.post_filtered());
        assert_eq!(
            plan.applied(),
            [AppliedFilter::new(
                FilterKind::Year,
                FilterApplication::Server
            )]
        );
    }

    #[test]
    fn an_open_year_range_is_closed_at_next_year() {
        assert_eq!(
            server_years("2024-".parse().unwrap(), 2026),
            Some(vec![2024, 2025, 2026, 2027])
        );
    }

    #[test]
    fn long_year_ranges_and_types_apply_after_retrieval() {
        let filters = Filters::default()
            .with_years("-2000".parse().unwrap())
            .with_doc_type(DocType::JournalArticle)
            .with_language("en");
        let plan = FilterPlan::new(&filters);
        assert_eq!(plan.query("actor"), "actor");
        assert!(plan.post_filtered());
        assert_eq!(
            plan.applied(),
            [
                AppliedFilter::new(FilterKind::Year, FilterApplication::PostRetrieval),
                AppliedFilter::new(FilterKind::DocType, FilterApplication::PostRetrieval),
                AppliedFilter::new(FilterKind::Language, FilterApplication::NotApplied),
            ]
        );
        let (records, _) = parse_search(SEARCH_LAST_PAGE).unwrap();
        let kept: Vec<_> = records.iter().filter(|r| plan.keeps(r)).collect();
        assert!(
            kept.is_empty(),
            "every hit is from 2011-2012 or not a journal article"
        );
        let plan = FilterPlan::new(&Filters::default().with_doc_type(DocType::JournalArticle));
        let kept: Vec<_> = records.iter().filter(|r| plan.keeps(r)).collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].key, "journals/eswa/HeHFKRLCS11");
    }

    #[test]
    fn types_dblp_lumps_together_are_not_applied() {
        for doc_type in [DocType::Book, DocType::Thesis, DocType::Review] {
            let plan = FilterPlan::new(&Filters::default().with_doc_type(doc_type));
            assert!(!plan.post_filtered());
            assert_eq!(
                plan.applied(),
                [AppliedFilter::new(
                    FilterKind::DocType,
                    FilterApplication::NotApplied
                )]
            );
        }
    }

    #[test]
    fn a_homonym_number_is_not_part_of_the_name() {
        assert_eq!(plain_author("Matthias Hein 0001"), "Matthias Hein");
        assert_eq!(plain_author("Ada Lovelace"), "Ada Lovelace");
        assert_eq!(plain_author("Group 2020x"), "Group 2020x");
    }
}
