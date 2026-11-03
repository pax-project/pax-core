//! dblp, the computer science bibliography, through its SPARQL endpoint
//! (`sparql.dblp.org`, a QLever instance over the dblp knowledge graph).
//! dblp's own search API and record exports sit behind a bot check that
//! answers automated clients with an HTML challenge page; the SPARQL
//! endpoint doesn't. dblp has no abstracts and no citation graph, so it only
//! adds to search.
//!
//! A search first asks how many publications match, then, for each page,
//! which ones are on it (newest first), their fields and their authors: the
//! last two looked up for that page's publications only.

use std::collections::HashMap;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{ACCEPT, RETRY_AFTER};
use serde::Deserialize;

use super::{
    AppliedFilter, CandidateId, CandidateWork, DocType, FilterApplication, FilterKind, Filters,
    Provider, ProviderError, ProviderId, RawPage, SearchOptions, SearchPage, YearRange,
    collect_pages,
};

const SPARQL_ENDPOINT: &str = "https://sparql.dblp.org/sparql";

/// The IRI a dblp key is the last part of, e.g. `conf/icse/Smith20`.
const RECORD_IRI: &str = "https://dblp.org/rec/";

/// How many publications one page asks for. Their fields are then looked up
/// by listing every one of them in the query, which keeps a page's queries
/// small.
const MAX_PAGE_SIZE: usize = 200;

/// Words this long or longer also match as a prefix (`model` matches
/// `models`), as on dblp's own search; shorter ones match only whole.
const MIN_PREFIX_LEN: usize = 4;

/// How many times to retry a rate-limited or failed request.
const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF: Duration = Duration::from_secs(2);

const PREFIXES: &str = "PREFIX dblp: <https://dblp.org/rdf/schema#>\n\
PREFIX ql: <http://qlever.cs.uni-freiburg.de/builtin-functions/>\n";

pub struct DblpProvider {
    http: reqwest::Client,
}

impl DblpProvider {
    pub fn new() -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .user_agent(format!("pax-core/{}", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        Ok(DblpProvider { http })
    }

    /// Runs `query` against the endpoint, retrying a rate-limited or failed
    /// request with backoff, and returns its result rows.
    async fn select(&self, query: &str) -> Result<Vec<Row>, ProviderError> {
        let mut backoff = INITIAL_BACKOFF;
        for attempt in 0..=MAX_RETRIES {
            let response = self
                .http
                .post(SPARQL_ENDPOINT)
                .header(ACCEPT, "application/sparql-results+json")
                .form(&[("query", query)])
                .send()
                .await
                .map_err(|e| ProviderError::Request(e.to_string()))?;
            let status = response.status();
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
            return parse_rows(&check_body(status, body)?);
        }
        unreachable!("the last attempt always returns")
    }

    /// One page of a search: the matching publications at `offset`, with
    /// their fields.
    async fn page(
        &self,
        search: &Search,
        offset: usize,
        size: usize,
    ) -> Result<Vec<Record>, ProviderError> {
        let keys = keys(&self.select(&search.page_query(offset, size)).await?);
        self.records(&keys).await
    }

    /// The records for `keys`, in that order; a key dblp has no publication
    /// for is left out.
    async fn records(&self, keys: &[String]) -> Result<Vec<Record>, ProviderError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let fields = self.select(&fields_query(keys)).await?;
        let authors = self.select(&authors_query(keys)).await?;
        Ok(assemble(keys, &fields, &authors))
    }

    async fn search_with(
        &self,
        matching: Matching,
        query: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        let plan = FilterPlan::new(&options.filters);
        let Some(search) = Search::new(matching, query, &plan) else {
            return Ok(SearchPage::new(Vec::new(), Some(0), None).with_filters(plan.applied()));
        };
        let total = count(&self.select(&search.count_query()).await?)?;
        let page = collect_pages(options, MAX_PAGE_SIZE, None, false, |offset, size| {
            let search = &search;
            async move {
                if offset as u64 >= total {
                    return Ok(RawPage::new(Vec::new(), total));
                }
                let works = self
                    .page(search, offset, size)
                    .await?
                    .into_iter()
                    .map(CandidateWork::from)
                    .collect();
                Ok(RawPage::new(works, total))
            }
        })
        .await?;
        Ok(page.with_filters(plan.applied()))
    }
}

/// A response's body, or why it isn't the data asked for. dblp.org answers
/// automated clients with an HTML bot check (status 202); should the
/// endpoint ever do the same, that is reported as such rather than as a
/// parse error.
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

    async fn search_by_author(&self, author: &str) -> Result<Vec<CandidateWork>, ProviderError> {
        let page = self
            .search_by_author_page(author, &SearchOptions::default())
            .await?;
        Ok(page.works)
    }

    /// Publications whose title has every word of `query`.
    async fn search_page(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        self.search_with(Matching::Title, query, options).await
    }

    /// Publications with an author (or, for a proceedings volume, an editor)
    /// whose name has every word of `author`.
    async fn search_by_author_page(
        &self,
        author: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        self.search_with(Matching::Author, author, options).await
    }

    /// `native_id` is a dblp key, e.g. `conf/icse/Smith20`.
    async fn get(&self, native_id: &str) -> Result<CandidateWork, ProviderError> {
        if !is_valid_key(native_id) {
            return Err(ProviderError::NotFound);
        }
        self.records(&[native_id.to_string()])
            .await?
            .into_iter()
            .next()
            .map(CandidateWork::from)
            .ok_or(ProviderError::NotFound)
    }
}

/// Whether `key` can go in a query as part of an IRI: dblp keys are
/// slash-separated letters, digits and a little punctuation.
fn is_valid_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_alphanumeric() || "/-_.:'+".contains(c))
}

/// Which literal a search's words are matched in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Matching {
    Title,
    Author,
}

/// A search, ready to be sent: which publications match, as a query pattern
/// binding `?p`, filters included.
#[derive(Debug)]
struct Search {
    pattern: String,
}

impl Search {
    /// `None` when `query` has no words to search for.
    fn new(matching: Matching, query: &str, plan: &FilterPlan) -> Option<Self> {
        let words = words(query, matching == Matching::Title)?;
        let mut pattern = format!("?t ql:contains-word \"{words}\" . ");
        pattern.push_str(match matching {
            Matching::Title => "?t ql:contains-entity ?title . ?p dblp:title ?title .",
            Matching::Author => {
                "?t ql:contains-entity ?name . ?s dblp:signatureDblpName ?name . \
                 ?s dblp:signaturePublication ?p ."
            }
        });
        pattern.push_str(&plan.pattern());
        Some(Search { pattern })
    }

    fn count_query(&self) -> String {
        format!(
            "{PREFIXES}SELECT (COUNT(DISTINCT ?p) AS ?n) WHERE {{ {} }}",
            self.pattern
        )
    }

    /// Newest first, then by key, so that pages don't overlap.
    fn page_query(&self, offset: usize, size: usize) -> String {
        format!(
            "{PREFIXES}SELECT DISTINCT ?p ?year WHERE {{ {} \
             OPTIONAL {{ ?p dblp:yearOfPublication ?year }} }} \
             ORDER BY DESC(?year) ?p LIMIT {size} OFFSET {offset}",
            self.pattern
        )
    }
}

/// `query`'s words for a text search, lowercased: everything between
/// characters that aren't letters or digits. With `prefix`, a word of at
/// least [`MIN_PREFIX_LEN`] characters matches as a prefix. `None` when there
/// are no words.
fn words(query: &str, prefix: bool) -> Option<String> {
    let words: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let w = w.to_lowercase();
            if prefix && w.chars().count() >= MIN_PREFIX_LEN {
                format!("{w}*")
            } else {
                w
            }
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// `keys` as a `VALUES ?p { … }` clause.
fn values(keys: &[String]) -> String {
    let iris: Vec<String> = keys.iter().map(|k| format!("<{RECORD_IRI}{k}>")).collect();
    format!("VALUES ?p {{ {} }}", iris.join(" "))
}

/// Each publication's fields, one row per combination of their values.
fn fields_query(keys: &[String]) -> String {
    format!(
        "{PREFIXES}SELECT ?p ?title ?year ?venue ?doi ?ee WHERE {{ {} \
         ?p dblp:title ?title . \
         OPTIONAL {{ ?p dblp:yearOfPublication ?year }} \
         OPTIONAL {{ ?p dblp:publishedIn ?venue }} \
         OPTIONAL {{ ?p dblp:doi ?doi }} \
         OPTIONAL {{ ?p dblp:documentPage ?ee }} }}",
        values(keys)
    )
}

/// Each publication's authors (or editors), with their position.
fn authors_query(keys: &[String]) -> String {
    format!(
        "{PREFIXES}SELECT ?p ?ord ?name WHERE {{ {} \
         ?s dblp:signaturePublication ?p . ?s dblp:signatureOrdinal ?ord . \
         ?s dblp:signatureDblpName ?name . }}",
        values(keys)
    )
}

/// One publication.
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

/// One result row: each bound variable's value. An unbound variable (an
/// `OPTIONAL` that didn't match) is absent.
type Row = HashMap<String, String>;

#[derive(Deserialize)]
struct SparqlResponse {
    results: SparqlResults,
}

#[derive(Deserialize)]
struct SparqlResults {
    bindings: Vec<HashMap<String, Term>>,
}

#[derive(Deserialize)]
struct Term {
    value: String,
}

/// A SPARQL JSON results document's rows.
fn parse_rows(body: &str) -> Result<Vec<Row>, ProviderError> {
    let response: SparqlResponse = serde_json::from_str(body)
        .map_err(|e| ProviderError::Request(format!("unexpected dblp response: {e}")))?;
    Ok(response
        .results
        .bindings
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|(var, term)| (var, term.value))
                .collect()
        })
        .collect())
}

/// The count a count query returned.
fn count(rows: &[Row]) -> Result<u64, ProviderError> {
    let n = rows.first().and_then(|row| row.get("n"));
    n.and_then(|n| n.parse().ok())
        .ok_or_else(|| ProviderError::Request(format!("unexpected dblp count: {n:?}")))
}

/// The dblp key of a record IRI.
fn key_of(iri: &str) -> Option<&str> {
    iri.strip_prefix(RECORD_IRI)
}

/// The dblp keys a page query returned, in order, each once.
fn keys(rows: &[Row]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for key in rows.iter().filter_map(|row| key_of(row.get("p")?)) {
        if !keys.iter().any(|k| k == key) {
            keys.push(key.to_string());
        }
    }
    keys
}

/// The records for `keys`, in that order, from the rows of a fields query and
/// an authors query. A key with no fields row is left out.
fn assemble(keys: &[String], fields: &[Row], authors: &[Row]) -> Vec<Record> {
    let mut records: HashMap<&str, Record> = HashMap::new();
    for row in fields {
        let Some(key) = row.get("p").and_then(|p| key_of(p)) else {
            continue;
        };
        let record = records.entry(key).or_insert_with(|| Record {
            key: key.to_string(),
            ..Record::default()
        });
        if let Some(title) = row.get("title")
            && record.title.is_empty()
        {
            record.title = title.clone();
        }
        set_once(&mut record.year, row.get("year"));
        set_once(&mut record.venue, row.get("venue"));
        let doi = row.get("doi").and_then(|d| doi_from_url(d));
        set_once(&mut record.doi, doi.as_ref());
        if let Some(ee) = row.get("ee")
            && !record.ee.contains(ee)
        {
            record.ee.push(ee.clone());
        }
    }

    let mut names: HashMap<&str, Vec<(u32, &str)>> = HashMap::new();
    for row in authors {
        let (Some(key), Some(ord), Some(name)) = (
            row.get("p").and_then(|p| key_of(p)),
            row.get("ord").and_then(|o| o.parse().ok()),
            row.get("name"),
        ) else {
            continue;
        };
        let list = names.entry(key).or_default();
        if !list.contains(&(ord, name.as_str())) {
            list.push((ord, name));
        }
    }

    keys.iter()
        .filter_map(|key| {
            let mut record = records.remove(key.as_str())?;
            let mut list = names.remove(key.as_str()).unwrap_or_default();
            list.sort();
            record.authors = list.into_iter().map(|(_, name)| name.to_string()).collect();
            Some(record)
        })
        .collect()
}

/// Sets `field` to `value` unless it's already set, so the first row wins.
fn set_once(field: &mut Option<String>, value: Option<&String>) {
    if field.is_none() {
        *field = value.cloned();
    }
}

/// The dblp class for a pax document type, where dblp has a class that is
/// the same thing. dblp files books and theses under one class and has no
/// review class, so those aren't filtered.
fn dblp_class(doc_type: DocType) -> Option<&'static str> {
    match doc_type {
        DocType::JournalArticle => Some("Article"),
        DocType::ConferencePaper => Some("Inproceedings"),
        // dblp's informal publications are overwhelmingly CoRR (arXiv).
        DocType::Preprint => Some("Informal"),
        DocType::BookChapter => Some("Incollection"),
        DocType::Dataset => Some("Data"),
        _ => None,
    }
}

/// How a search applies its filters: a year range and a type dblp has a
/// class for go in the query. dblp records carry no language.
struct FilterPlan {
    filters: Filters,
    years: Option<YearRange>,
    class: Option<&'static str>,
}

impl FilterPlan {
    fn new(filters: &Filters) -> Self {
        FilterPlan {
            filters: filters.clone(),
            years: filters.years,
            class: filters.doc_type.and_then(dblp_class),
        }
    }

    /// The query pattern for the filters, to follow the one binding `?p`.
    fn pattern(&self) -> String {
        let mut pattern = String::new();
        if let Some(class) = self.class {
            pattern.push_str(&format!(" ?p a dblp:{class} ."));
        }
        if let Some(years) = self.years {
            let mut conditions = Vec::new();
            if let Some(from) = years.from {
                conditions.push(format!("YEAR(?fy) >= {from}"));
            }
            if let Some(to) = years.to {
                conditions.push(format!("YEAR(?fy) <= {to}"));
            }
            if !conditions.is_empty() {
                pattern.push_str(&format!(
                    " ?p dblp:yearOfPublication ?fy . FILTER({})",
                    conditions.join(" && ")
                ));
            }
        }
        pattern
    }

    fn applied(&self) -> Vec<AppliedFilter> {
        self.filters
            .kinds()
            .into_iter()
            .map(|kind| {
                let application = match kind {
                    FilterKind::Year => FilterApplication::Server,
                    FilterKind::DocType if self.class.is_some() => FilterApplication::Server,
                    _ => FilterApplication::NotApplied,
                };
                AppliedFilter::new(kind, application)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Recorded responses from sparql.dblp.org (2026-11-03) to the queries
    // `fields_query` and `authors_query` build for the keys in
    // `RECORDED_KEYS`, and to a page query.
    const FIELDS: &str = include_str!("../../tests/fixtures/dblp/fields.json");
    const AUTHORS: &str = include_str!("../../tests/fixtures/dblp/authors.json");
    const PAGE: &str = include_str!("../../tests/fixtures/dblp/page.json");
    const COUNT: &str = include_str!("../../tests/fixtures/dblp/count.json");
    const RECORDED_KEYS: [&str; 5] = [
        "conf/icse/Chattopadhyay0A20",
        "conf/apsipa/ChienY21",
        "journals/corr/abs-1109-1074",
        "books/daglib/0027496",
        "conf/issa/Egan11",
    ];

    fn recorded() -> Vec<CandidateWork> {
        let keys: Vec<String> = RECORDED_KEYS.iter().map(|k| k.to_string()).collect();
        let fields = parse_rows(FIELDS).unwrap();
        let authors = parse_rows(AUTHORS).unwrap();
        assemble(&keys, &fields, &authors)
            .into_iter()
            .map(CandidateWork::from)
            .collect()
    }

    fn by_key<'a>(works: &'a [CandidateWork], key: &str) -> &'a CandidateWork {
        works.iter().find(|w| w.id.native_id == key).unwrap()
    }

    #[test]
    fn records_come_back_in_the_order_asked_for() {
        let works = recorded();
        let keys: Vec<&str> = works.iter().map(|w| w.id.native_id.as_str()).collect();
        assert_eq!(keys, RECORDED_KEYS);
    }

    #[test]
    fn a_conference_paper_has_its_fields_and_ordered_authors() {
        let works = recorded();
        let work = by_key(&works, "conf/icse/Chattopadhyay0A20");
        assert_eq!(work.id.to_string(), "dblp:conf/icse/Chattopadhyay0A20");
        assert_eq!(
            work.title,
            "A tale from the trenches: cognitive biases and software development"
        );
        assert_eq!(
            work.authors,
            [
                "Souti Chattopadhyay",
                "Nicholas Nelson",
                "Audrey Au",
                "Natalia Morales",
                "Christopher A. Sanchez",
                "Rahul Pandita",
                "Anita Sarma",
            ]
        );
        assert_eq!(work.publish_date, "2020");
        assert_eq!(work.venue.as_deref(), Some("ICSE"));
        assert_eq!(work.doi.as_deref(), Some("10.1145/3377811.3380330"));
        // A doi.org link is a landing page, not a PDF.
        assert_eq!(work.pdf_url, None);
        assert_eq!(work.abstract_text, None);
    }

    #[test]
    fn a_corr_record_links_its_arxiv_doi() {
        let works = recorded();
        let corr = by_key(&works, "journals/corr/abs-1109-1074");
        assert_eq!(corr.venue.as_deref(), Some("CoRR"));
        assert_eq!(corr.doi.as_deref(), Some("10.48550/ARXIV.1109.1074"));
        // Its arXiv DOI is then an identity key, so it merges with arXiv's record.
        assert!(
            crate::identity::identity_keys(corr)
                .contains(&crate::IdentityKey::Arxiv("1109.1074".into()))
        );
    }

    #[test]
    fn a_book_without_venue_or_doi_still_parses() {
        let works = recorded();
        let book = by_key(&works, "books/daglib/0027496");
        assert_eq!(book.venue, None);
        assert_eq!(book.doi, None);
        assert_eq!(book.pdf_url, None);
        assert_eq!(book.authors, ["Jan Göbel", "Andreas Dewald"]);
    }

    #[test]
    fn a_pdf_edition_is_the_pdf_url() {
        let works = recorded();
        let egan = by_key(&works, "conf/issa/Egan11");
        assert_eq!(egan.authors, ["Shaun Egan"]);
        assert_eq!(egan.doi, None);
        assert_eq!(
            egan.pdf_url.as_deref(),
            Some("http://icsa.cs.up.ac.za/issa/2011/Proceedings/Full/49_Paper.pdf")
        );
    }

    #[test]
    fn a_key_without_fields_is_left_out() {
        let keys = vec!["conf/x/Missing99".to_string(), RECORDED_KEYS[0].to_string()];
        let records = assemble(
            &keys,
            &parse_rows(FIELDS).unwrap(),
            &parse_rows(AUTHORS).unwrap(),
        );
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].key, RECORDED_KEYS[0]);
    }

    #[test]
    fn a_page_query_gives_its_keys_in_order() {
        let keys = keys(&parse_rows(PAGE).unwrap());
        assert_eq!(keys.len(), 5);
        assert_eq!(keys[0], "conf/ahfe/KuhneM21");
        assert!(keys.iter().all(|k| !k.starts_with("https://")));
    }

    #[test]
    fn a_count_query_gives_the_total() {
        assert_eq!(count(&parse_rows(COUNT).unwrap()).unwrap(), 17);
        assert!(count(&[]).is_err());
    }

    #[test]
    fn a_bot_check_page_is_reported_as_one() {
        let page = "<!doctype html><html><head><title>Making sure you're not a bot!</title>";
        let err = check_body(StatusCode::ACCEPTED, page.to_string()).unwrap_err();
        assert!(err.to_string().contains("bot check"), "{err}");
        assert!(parse_rows(page).is_err());
    }

    #[test]
    fn an_endpoint_error_is_reported_with_its_message() {
        let err = check_body(StatusCode::BAD_REQUEST, "{\"exception\":\"bad\"}".into())
            .unwrap_err()
            .to_string();
        assert!(err.contains("400") && err.contains("bad"), "{err}");
    }

    #[test]
    fn query_words_are_lowercased_and_long_ones_match_as_prefixes() {
        assert_eq!(
            words("The Actor-Model, of \"C\"!", true).as_deref(),
            Some("the actor* model* of c")
        );
        assert_eq!(words("Carl Hewitt", false).as_deref(), Some("carl hewitt"));
        assert_eq!(words(" -- ", true), None);
        assert_eq!(words("Göbel", true).as_deref(), Some("göbel*"));
    }

    #[test]
    fn a_query_without_words_is_not_sent() {
        let plan = FilterPlan::new(&Filters::default());
        assert!(Search::new(Matching::Title, "?!", &plan).is_none());
    }

    #[test]
    fn a_title_search_matches_title_words_and_pages_newest_first() {
        let plan = FilterPlan::new(&Filters::default());
        let search = Search::new(Matching::Title, "actor model", &plan).unwrap();
        let page = search.page_query(400, 200);
        assert!(
            page.contains("ql:contains-word \"actor* model*\""),
            "{page}"
        );
        assert!(page.contains("?p dblp:title ?title"), "{page}");
        assert!(
            page.ends_with("ORDER BY DESC(?year) ?p LIMIT 200 OFFSET 400"),
            "{page}"
        );
        assert!(search.count_query().contains("COUNT(DISTINCT ?p)"));
    }

    #[test]
    fn an_author_search_matches_signature_names() {
        let plan = FilterPlan::new(&Filters::default());
        let search = Search::new(Matching::Author, "Carl Hewitt", &plan).unwrap();
        let count = search.count_query();
        assert!(count.contains("\"carl hewitt\""), "{count}");
        assert!(count.contains("dblp:signatureDblpName ?name"), "{count}");
    }

    #[test]
    fn year_and_type_filters_go_in_the_query() {
        let filters = Filters::default()
            .with_years("2019-2021".parse().unwrap())
            .with_doc_type(DocType::ConferencePaper)
            .with_language("en");
        let plan = FilterPlan::new(&filters);
        assert_eq!(
            plan.pattern(),
            " ?p a dblp:Inproceedings . ?p dblp:yearOfPublication ?fy . \
             FILTER(YEAR(?fy) >= 2019 && YEAR(?fy) <= 2021)"
        );
        assert_eq!(
            plan.applied(),
            [
                AppliedFilter::new(FilterKind::Year, FilterApplication::Server),
                AppliedFilter::new(FilterKind::DocType, FilterApplication::Server),
                AppliedFilter::new(FilterKind::Language, FilterApplication::NotApplied),
            ]
        );
    }

    #[test]
    fn an_open_year_range_has_one_bound() {
        let plan = FilterPlan::new(&Filters::default().with_years("-2000".parse().unwrap()));
        assert_eq!(
            plan.pattern(),
            " ?p dblp:yearOfPublication ?fy . FILTER(YEAR(?fy) <= 2000)"
        );
    }

    #[test]
    fn types_dblp_lumps_together_are_not_applied() {
        for doc_type in [DocType::Book, DocType::Thesis, DocType::Review] {
            let plan = FilterPlan::new(&Filters::default().with_doc_type(doc_type));
            assert_eq!(plan.pattern(), "");
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
    fn only_key_like_ids_are_looked_up() {
        assert!(is_valid_key("conf/icse/Smith20"));
        assert!(is_valid_key("journals/corr/abs-1109-1074"));
        assert!(!is_valid_key(""));
        assert!(!is_valid_key("conf/x> } DROP"));
        assert!(!is_valid_key("a b"));
    }

    #[test]
    fn a_homonym_number_is_not_part_of_the_name() {
        assert_eq!(plain_author("Matthias Hein 0001"), "Matthias Hein");
        assert_eq!(plain_author("Ada Lovelace"), "Ada Lovelace");
        assert_eq!(plain_author("Group 2020x"), "Group 2020x");
    }

    /// Asks the live endpoint. Run by hand: `cargo test --all-features -- --ignored live_`.
    /// Needs tokio's runtime, which only the `cli` feature turns on.
    #[cfg(feature = "cli")]
    #[test]
    #[ignore = "network: asks sparql.dblp.org"]
    fn live_search_and_get() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let dblp = DblpProvider::new().unwrap();
            let options = SearchOptions::default()
                .with_filters(Filters::default().with_years("2019-2021".parse().unwrap()));
            let page = dblp.search_page("actor model", &options).await.unwrap();
            assert!(page.total.unwrap() > 0);
            assert!(!page.works.is_empty());
            assert!(page.works.iter().all(|w| {
                let year: i32 = w.publish_date.parse().unwrap();
                (2019..=2021).contains(&year)
            }));
            let work = dblp.get("conf/icse/Chattopadhyay0A20").await.unwrap();
            assert_eq!(work.doi.as_deref(), Some("10.1145/3377811.3380330"));
            assert_eq!(work.authors.len(), 7);
            assert!(matches!(
                dblp.get("conf/x/NoSuchKey99").await,
                Err(ProviderError::NotFound)
            ));
        });
    }
}
