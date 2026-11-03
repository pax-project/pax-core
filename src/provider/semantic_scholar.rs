use std::future::Future;
use std::time::Duration;

use ::semantic_scholar::{Paper, PaperSearchResponse};
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::pacing::Paced;
use super::{
    AppliedFilter, CandidateId, CandidateWork, DocType, FilterApplication, FilterKind, Filters,
    Limit, Provider, ProviderError, ProviderId, RawPage, SearchOptions, SearchPage, collect_pages,
};

/// Semantic Scholar's relevance search serves at most 100 papers per request.
const MAX_PAGE_SIZE: usize = 100;

/// Relevance search requires `offset + limit <= 1000`: nothing past the
/// first 1,000 matches is reachable, however far it's paged.
const MAX_RETRIEVABLE: u64 = 1000;

pub struct SemanticScholarProvider {
    /// Sends every request, through [`Self::get_json`], so each attempt,
    /// retries included, waits its turn under [`PACER`]. The
    /// `semantic_scholar` crate supplies only the response types: its own
    /// client retries internally, out of the pacer's reach. (Its `Paper`
    /// also requires a `paperId` that unmatched list entries lack, so the
    /// lists are parsed leniently, see [`ListEntry`].)
    http: reqwest::Client,
    /// The least time between two requests to Semantic Scholar, kept by
    /// [`PACER`] across every provider value.
    min_interval: Duration,
}

/// Paces every request to Semantic Scholar in this process, whichever
/// [`SemanticScholarProvider`] sends it. Its rate limit is per caller (one
/// IP address, or one API key), not per client value, and the library makes
/// a fresh provider per seed, so one shared pacer is what keeps a whole
/// multi-seed run under it.
static PACER: Paced = Paced::new();

impl SemanticScholarProvider {
    /// The least time between two requests unless
    /// [`Self::with_min_interval`] sets another: one request per second,
    /// the rate Semantic Scholar grants an API key by default and about
    /// what its shared unauthenticated pool sustains.
    pub const DEFAULT_MIN_INTERVAL: Duration = Duration::from_secs(1);

    pub fn new(api_key: Option<&str>) -> Result<Self, ProviderError> {
        let mut headers = HeaderMap::new();
        if let Some(key) = api_key {
            let key = HeaderValue::from_str(key)
                .map_err(|_| ProviderError::Request("invalid Semantic Scholar API key".into()))?;
            headers.insert("x-api-key", key);
        }
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        Ok(SemanticScholarProvider {
            http,
            min_interval: Self::DEFAULT_MIN_INTERVAL,
        })
    }

    /// Spaces this provider's requests at least `interval` apart instead of
    /// [`Self::DEFAULT_MIN_INTERVAL`], e.g. shorter for an API key granted a
    /// higher rate. Requests are paced across every provider value in the
    /// process, each waiting its own interval after the one before it. A
    /// `Retry-After` from Semantic Scholar holds every request back
    /// regardless.
    pub fn with_min_interval(mut self, interval: Duration) -> Self {
        self.min_interval = interval;
        self
    }

    /// The papers `paper` cites — its reference list — paged. `paper` is
    /// anything [`Provider::get`] accepts: a paper id, `DOI:<doi>`,
    /// `ARXIV:<id>`, …
    ///
    /// The reference and citation endpoints take no filters, so year and type
    /// are applied after retrieval (see [`citation_filters`]).
    pub async fn references_page(
        &self,
        paper: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        self.citation_list_page(paper, CitationList::References, options)
            .await
    }

    /// The papers that cite `paper`, paged; as for [`Self::references_page`].
    pub async fn citations_page(
        &self,
        paper: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        self.citation_list_page(paper, CitationList::Citations, options)
            .await
    }

    async fn citation_list_page(
        &self,
        paper: &str,
        list: CitationList,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        // The list endpoints report no total; the paper's own counts are it.
        let seed = self
            .get_paper(paper, "paperId,referenceCount,citationCount")
            .await?;
        let total = match list {
            CitationList::References => seed.reference_count,
            CitationList::Citations => seed.citation_count,
        };
        let total = u64::from(total.unwrap_or(0));

        let filters = &options.filters;
        let post_filtered = filters.years.is_some() || filters.doc_type.is_some();
        let fields = list_fields(options);
        let page = collect_pages(
            options,
            MAX_CITATION_PAGE_SIZE,
            Some(MAX_CITATIONS_RETRIEVABLE),
            post_filtered,
            |offset, limit| {
                let path = format!("/paper/{}/{}", seed.paper_id, list.endpoint());
                let fields = &fields;
                async move {
                    let query = [
                        ("offset", offset.to_string()),
                        ("limit", limit.to_string()),
                        ("fields", fields.clone()),
                    ];
                    let response: ListResponse = self.get_json(GRAPH_API, &path, &query).await?;
                    let entries = response.data.ok_or_else(|| withheld(list))?;
                    let fetched = entries.len();
                    let works = entries
                        .into_iter()
                        .filter_map(ListEntry::into_paper)
                        .filter(|paper| keeps(filters, paper))
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
        Ok(page.with_filters(citation_filters(filters)))
    }

    /// The papers Semantic Scholar's Recommendations API recommends for
    /// `paper`, as many as `options.limit` asks for up to
    /// [`MAX_RECOMMENDATIONS`]. `paper` is as for [`Self::references_page`]:
    /// the endpoint takes the same id forms. Recommendations come from the
    /// API's default pool, [`RECOMMENDATION_POOL`].
    ///
    /// The endpoint takes no filters and reports no total, so year and type
    /// are applied after retrieval (see [`citation_filters`]) and the page's
    /// `total` is `None`: a recommendation list is never presented as
    /// complete.
    pub async fn recommendations_page(
        &self,
        paper: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        let filters = &options.filters;
        let applied = citation_filters(filters);
        let wanted = match options.limit {
            Limit::Count(n) => Some(n),
            Limit::All => None,
        };
        if wanted == Some(0) {
            return Ok(
                SearchPage::new(Vec::new(), None, Some(MAX_RECOMMENDATIONS)).with_filters(applied)
            );
        }
        let post_filtered = filters.years.is_some() || filters.doc_type.is_some();
        let limit = recommendation_limit(wanted, post_filtered);
        let path = format!("/papers/forpaper/{paper}");
        let query = [
            ("from", RECOMMENDATION_POOL.to_string()),
            ("limit", limit.to_string()),
            ("fields", list_fields(options)),
        ];
        let response: RecommendationsResponse =
            self.get_json(RECOMMENDATIONS_API, &path, &query).await?;
        let fetched = response.recommended_papers.len();
        let mut works: Vec<CandidateWork> = response
            .recommended_papers
            .into_iter()
            .filter_map(paper_with_id)
            .filter(|paper| keeps(filters, paper))
            .map(CandidateWork::from)
            .collect();
        if let Some(n) = wanted {
            works.truncate(n);
        }
        Ok(SearchPage::new(works, None, Some(MAX_RECOMMENDATIONS))
            .with_scanned(fetched as u64)
            .with_filters(applied))
    }

    /// The paper `paper` (anything [`Provider::get`] accepts) with `fields`.
    /// One Semantic Scholar doesn't have is [`ProviderError::NotFound`].
    async fn get_paper(&self, paper: &str, fields: &str) -> Result<Paper, ProviderError> {
        let path = format!("/paper/{paper}");
        self.get_json(GRAPH_API, &path, &[("fields", fields.to_string())])
            .await
    }

    /// GETs `path` under `api` (the Graph or Recommendations API) and parses
    /// the body as `T`. Every attempt, retries included, waits its turn under
    /// [`PACER`] (see [`paced_retries`]); a 404 is
    /// [`ProviderError::NotFound`].
    async fn get_json<T: DeserializeOwned>(
        &self,
        api: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, ProviderError> {
        let url = &format!("{api}{path}");
        let http = &self.http;
        paced_retries(
            &PACER,
            self.min_interval,
            INITIAL_BACKOFF,
            move || async move {
                let response = http
                    .get(url)
                    .query(query)
                    .send()
                    .await
                    .map_err(|e| Failure::Final(ProviderError::Request(e.to_string())))?;
                let status = response.status();
                if status.is_success() {
                    let body = response
                        .text()
                        .await
                        .map_err(|e| Failure::Final(ProviderError::Request(e.to_string())))?;
                    return serde_json::from_str(&body).map_err(|e| {
                        Failure::Final(ProviderError::Request(format!(
                            "unexpected Semantic Scholar response: {e}"
                        )))
                    });
                }
                let retry_after = response
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok())
                    .map(Duration::from_secs);
                let body = response.text().await.unwrap_or_default();
                Err(failure(status, retry_after, path, &body))
            },
        )
        .await
    }
}

/// What an unsuccessful answer (`status`, with `body`) to a request for
/// `path` means: a 404 is [`ProviderError::NotFound`]; a 429 or a server
/// error is worth retrying, after `retry_after` if Semantic Scholar sent
/// one; anything else fails the request.
fn failure(status: StatusCode, retry_after: Option<Duration>, path: &str, body: &str) -> Failure {
    if status == StatusCode::NOT_FOUND {
        return Failure::Final(ProviderError::NotFound);
    }
    let excerpt: String = body.chars().take(200).collect();
    let error = ProviderError::Request(format!(
        "Semantic Scholar answered {status} for {path}: {excerpt}"
    ));
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        Failure::Retryable { error, retry_after }
    } else {
        Failure::Final(error)
    }
}

/// Why one attempt at a request failed.
enum Failure {
    /// Worth trying again (rate-limited, or a server error): after
    /// `retry_after` if the provider said, else the backoff. `error` is
    /// what the request fails with once the retries run out.
    Retryable {
        error: ProviderError,
        retry_after: Option<Duration>,
    },
    /// Not worth trying again.
    Final(ProviderError),
}

/// Runs `attempt` until it succeeds, fails for good, or has been retried
/// [`MAX_RETRIES`] times. Every attempt, the first and each retry alike,
/// first waits its turn under `pacer`, `interval` after the request before
/// it, so a retry can't jump ahead of a run's other requests. The wait
/// before a retry (`Retry-After`, or a backoff starting at
/// `initial_backoff` and doubling) is put on `pacer` too, so it holds every
/// other request back as well.
async fn paced_retries<T, F, Fut>(
    pacer: &Paced,
    interval: Duration,
    initial_backoff: Duration,
    mut attempt: F,
) -> Result<T, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Failure>>,
{
    let mut backoff = initial_backoff;
    let mut retries = 0;
    loop {
        pacer.wait(interval).await;
        match attempt().await {
            Ok(value) => return Ok(value),
            Err(Failure::Final(error)) => return Err(error),
            Err(Failure::Retryable { error, .. }) if retries == MAX_RETRIES => return Err(error),
            Err(Failure::Retryable { retry_after, .. }) => {
                // The next attempt's wait() waits this out, as does any other
                // request to Semantic Scholar meanwhile.
                pacer.hold_off(retry_after.unwrap_or(backoff));
                backoff *= 2;
                retries += 1;
            }
        }
    }
}

const GRAPH_API: &str = "https://api.semanticscholar.org/graph/v1";
const RECOMMENDATIONS_API: &str = "https://api.semanticscholar.org/recommendations/v1";

/// The Recommendations API serves at most 500 papers per request, and has no
/// paging.
const MAX_RECOMMENDATIONS: u64 = 500;

/// The pool recommendations are drawn from: `recent`, the API's default, is
/// recent papers from every field; the other, `all-cs`, would restrict them
/// to computer science, which a review in another field can't use.
const RECOMMENDATION_POOL: &str = "recent";

/// How many recommendations to ask for: `wanted`, or every one the endpoint
/// serves for `--all` or when a post-retrieval filter may drop most of a
/// short list (it's cut to `wanted` after filtering).
fn recommendation_limit(wanted: Option<usize>, post_filtered: bool) -> usize {
    let max = MAX_RECOMMENDATIONS as usize;
    match wanted {
        Some(n) if !post_filtered => n.min(max),
        _ => max,
    }
}

/// A Recommendations API response. Parsed leniently, like the list
/// endpoints: an entry without a `paperId` is skipped rather than failing
/// the page.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecommendationsResponse {
    #[serde(default)]
    recommended_papers: Vec<serde_json::Value>,
}
const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Which of a paper's citation lists to page through.
#[derive(Clone, Copy)]
enum CitationList {
    References,
    Citations,
}

impl CitationList {
    fn endpoint(self) -> &'static str {
        match self {
            CitationList::References => "references",
            CitationList::Citations => "citations",
        }
    }
}

/// One page of a reference or citation list. `data` is `null` when the
/// publisher has the list withheld.
#[derive(Deserialize)]
struct ListResponse {
    #[serde(default)]
    data: Option<Vec<ListEntry>>,
}

/// A list entry: the cited paper (references) or the citing one (citations).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListEntry {
    #[serde(default, alias = "citingPaper")]
    cited_paper: Option<serde_json::Value>,
}

impl ListEntry {
    /// The entry's paper, or `None` for one Semantic Scholar couldn't match
    /// to a paper: with no `paperId` it can't be addressed, so it's skipped.
    fn into_paper(self) -> Option<Paper> {
        paper_with_id(self.cited_paper?)
    }
}

/// `paper` as a [`Paper`], or `None` when it has no `paperId` (so can't be
/// addressed) or doesn't parse.
fn paper_with_id(paper: serde_json::Value) -> Option<Paper> {
    let has_id = paper
        .get("paperId")
        .and_then(|id| id.as_str())
        .is_some_and(|id| !id.is_empty());
    has_id.then(|| serde_json::from_value(paper).ok()).flatten()
}

/// The reference and citation endpoints serve at most 1,000 papers per
/// request.
const MAX_CITATION_PAGE_SIZE: usize = 1000;

/// The reference and citation endpoints require `offset + limit < 10000`.
const MAX_CITATIONS_RETRIEVABLE: u64 = 9_999;

/// Every field `CandidateWork` reads (but the abstract, which `get` always
/// asks for and the lists only with [`SearchOptions::abstracts`]), plus what
/// post-retrieval filters need. Without it the client asks for its own
/// defaults, which leave out the DOI, date, venue and PDF.
const PAPER_FIELDS: &str =
    "paperId,title,authors,year,publicationDate,externalIds,venue,openAccessPdf,publicationTypes";

/// The error for a list page that came back with `data: null`: the
/// publisher has Semantic Scholar withhold the list.
fn withheld(list: CitationList) -> ProviderError {
    ProviderError::Withheld {
        message: format!(
            "Semantic Scholar withholds this paper's {} at the publisher's request",
            list.endpoint()
        ),
    }
}

const ABSTRACT_FIELD: &str = "abstract";

/// The `fields` a search, citation-list or recommendations request asks for:
/// [`PAPER_FIELDS`], plus the abstract when `options` asks for abstracts.
fn list_fields(options: &SearchOptions) -> String {
    if options.abstracts {
        format!("{PAPER_FIELDS},{ABSTRACT_FIELD}")
    } else {
        PAPER_FIELDS.to_string()
    }
}

/// How the reference and citation lists apply `filters`: year and (mapped)
/// type after retrieval, since those endpoints take no filters; language
/// not at all, since Semantic Scholar records carry none.
fn citation_filters(filters: &Filters) -> Vec<AppliedFilter> {
    filters
        .kinds()
        .into_iter()
        .map(|kind| {
            let application = match kind {
                FilterKind::Year => FilterApplication::PostRetrieval,
                FilterKind::DocType if filters.doc_type.and_then(publication_type).is_some() => {
                    FilterApplication::PostRetrieval
                }
                _ => FilterApplication::NotApplied,
            };
            AppliedFilter::new(kind, application)
        })
        .collect()
}

/// Whether `paper` passes the year and type filters applied after
/// retrieval. A paper missing the field doesn't match a filter on it.
fn keeps(filters: &Filters, paper: &Paper) -> bool {
    let year_ok = filters.years.is_none_or(|years| {
        paper
            .year
            .and_then(|year| i32::try_from(year).ok())
            .is_some_and(|year| years.contains(year))
    });
    let type_ok = filters
        .doc_type
        .and_then(publication_type)
        .is_none_or(|wanted| {
            paper
                .publication_types
                .as_ref()
                .is_some_and(|types| types.iter().any(|t| t == wanted))
        });
    year_ok && type_ok
}

impl Provider for SemanticScholarProvider {
    fn id(&self) -> ProviderId {
        ProviderId::SemanticScholar
    }

    async fn search(&self, query: &str) -> Result<Vec<CandidateWork>, ProviderError> {
        let page = self.search_page(query, &SearchOptions::default()).await?;
        Ok(page.works)
    }

    async fn search_page(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        let years = options.filters.years.map(|years| years.to_string());
        let publication_type = options.filters.doc_type.and_then(publication_type);
        let fields = list_fields(options);
        let page = collect_pages(
            options,
            MAX_PAGE_SIZE,
            Some(MAX_RETRIEVABLE),
            false,
            |offset, limit| {
                let mut params = vec![
                    ("query", query.to_string()),
                    ("fields", fields.clone()),
                    ("offset", offset.to_string()),
                    ("limit", limit.to_string()),
                ];
                if let Some(years) = &years {
                    params.push(("year", years.clone()));
                }
                if let Some(publication_type) = publication_type {
                    params.push(("publicationTypes", publication_type.to_string()));
                }
                async move {
                    let result: PaperSearchResponse =
                        self.get_json(GRAPH_API, "/paper/search", &params).await?;
                    let works = result.data.into_iter().map(CandidateWork::from).collect();
                    Ok(RawPage::new(works, result.total))
                }
            },
        )
        .await?;
        Ok(page.with_filters(applied_filters(&options.filters)))
    }

    async fn get(&self, native_id: &str) -> Result<CandidateWork, ProviderError> {
        let paper = self
            .get_paper(native_id, &format!("{PAPER_FIELDS},{ABSTRACT_FIELD}"))
            .await?;
        Ok(CandidateWork::from(paper))
    }

    async fn get_by_doi(&self, doi: &str) -> Result<CandidateWork, ProviderError> {
        self.get(&format!("DOI:{doi}")).await
    }
}

/// Semantic Scholar's `publicationTypes` value for `doc_type`, if it has an
/// equivalent: it has no preprint or thesis type.
fn publication_type(doc_type: DocType) -> Option<&'static str> {
    match doc_type {
        DocType::JournalArticle => Some("JournalArticle"),
        DocType::ConferencePaper => Some("Conference"),
        DocType::Review => Some("Review"),
        DocType::Book => Some("Book"),
        DocType::BookChapter => Some("BookSection"),
        DocType::Dataset => Some("Dataset"),
        DocType::Preprint | DocType::Thesis => None,
    }
}

/// Year and (mapped) type are filtered server-side; Semantic Scholar records
/// carry no language, so a language filter can't be applied at all.
fn applied_filters(filters: &Filters) -> Vec<AppliedFilter> {
    filters
        .kinds()
        .into_iter()
        .map(|kind| {
            let application = match kind {
                FilterKind::Year => FilterApplication::Server,
                FilterKind::DocType if filters.doc_type.and_then(publication_type).is_some() => {
                    FilterApplication::Server
                }
                _ => FilterApplication::NotApplied,
            };
            AppliedFilter::new(kind, application)
        })
        .collect()
}

impl From<::semantic_scholar::Paper> for CandidateWork {
    fn from(value: ::semantic_scholar::Paper) -> Self {
        CandidateWork {
            id: CandidateId {
                provider: ProviderId::SemanticScholar,
                native_id: value.paper_id.clone(),
            },
            title: value.title.unwrap_or_default(),
            authors: value
                .authors
                .unwrap_or_default()
                .into_iter()
                .filter_map(|author| author.name)
                .collect(),
            publish_date: value.publication_date.unwrap_or_default(),
            doi: value.external_ids.as_ref().and_then(|ids| ids.doi.clone()),
            // An arXiv id always has a PDF behind it; `openAccessPdf` is
            // sometimes a repository landing page instead.
            pdf_url: value
                .external_ids
                .as_ref()
                .and_then(|ids| ids.arxiv.as_deref())
                .map(|arxiv| format!("https://arxiv.org/pdf/{arxiv}"))
                .or_else(|| {
                    value
                        .open_access_pdf
                        .as_ref()
                        .and_then(|pdf| pdf.url.clone())
                }),
            venue: value.venue,
            abstract_text: value.abstract_text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::semantic_scholar::{ExternalIds, OpenAccessPdf, Paper};
    use std::sync::Mutex;
    use std::time::Instant;

    fn rate_limited() -> Failure {
        Failure::Retryable {
            error: ProviderError::Request("Semantic Scholar answered 429".into()),
            retry_after: None,
        }
    }

    /// Runs [`paced_retries`] on its own pacer, every attempt rate-limited
    /// until the `succeed_on`-th (1-based), and returns the result and when
    /// each attempt was sent. The backoff is zero, so any spacing between
    /// attempts comes from the pacer. `on_attempt` runs as each one is sent.
    async fn attempts(
        pacer: &Paced,
        interval: Duration,
        succeed_on: usize,
        on_attempt: impl Fn(usize),
    ) -> (Result<usize, ProviderError>, Vec<Instant>) {
        let sent = Mutex::new(Vec::new());
        let result = paced_retries(pacer, interval, Duration::ZERO, || {
            let n = {
                let mut sent = sent.lock().unwrap();
                sent.push(Instant::now());
                sent.len()
            };
            on_attempt(n);
            async move {
                if n == succeed_on {
                    Ok(n)
                } else {
                    Err(rate_limited())
                }
            }
        })
        .await;
        (result, sent.into_inner().unwrap())
    }

    #[tokio::test]
    async fn every_retry_waits_its_turn_with_the_pacer() {
        let pacer = Paced::new();
        let interval = Duration::from_millis(40);
        let (result, sent) = attempts(&pacer, interval, 3, |_| {}).await;
        assert_eq!(result.unwrap(), 3);
        assert_eq!(sent.len(), 3);
        for pair in sent.windows(2) {
            assert!(
                pair[1] - pair[0] >= interval,
                "a retry went out {:?} after the attempt before it",
                pair[1] - pair[0]
            );
        }
    }

    #[tokio::test]
    async fn a_retry_waits_behind_a_hold_off_from_another_request() {
        // Another request's Retry-After, set while this one is in flight,
        // holds this one's retry back too.
        let pacer = Paced::new();
        let hold = Duration::from_millis(120);
        let (result, sent) = attempts(&pacer, Duration::ZERO, 2, |n| {
            if n == 1 {
                pacer.hold_off(hold);
            }
        })
        .await;
        assert_eq!(result.unwrap(), 2);
        assert!(sent[1] - sent[0] >= hold);
    }

    #[tokio::test]
    async fn the_first_attempt_waits_its_turn_too() {
        let pacer = Paced::new();
        let interval = Duration::from_millis(60);
        let start = Instant::now();
        pacer.wait(interval).await; // the run's previous request
        let (result, sent) = attempts(&pacer, interval, 1, |_| {}).await;
        assert_eq!(result.unwrap(), 1);
        assert!(sent[0] - start >= interval);
    }

    #[tokio::test]
    async fn retries_stop_after_the_last_and_return_its_error() {
        let pacer = Paced::new();
        let (result, sent) = attempts(&pacer, Duration::ZERO, usize::MAX, |_| {}).await;
        assert_eq!(sent.len(), MAX_RETRIES as usize + 1);
        assert!(matches!(result, Err(ProviderError::Request(m)) if m.contains("429")));
    }

    #[tokio::test]
    async fn a_final_failure_is_not_retried() {
        let pacer = Paced::new();
        let calls = Mutex::new(0);
        let result: Result<(), _> = paced_retries(&pacer, Duration::ZERO, Duration::ZERO, || {
            *calls.lock().unwrap() += 1;
            async { Err(Failure::Final(ProviderError::NotFound)) }
        })
        .await;
        assert!(matches!(result, Err(ProviderError::NotFound)));
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[test]
    fn language_and_unmapped_types_are_not_applied() {
        let filters = Filters::default()
            .with_years("2020".parse().unwrap())
            .with_doc_type(DocType::Preprint)
            .with_language("en");
        let applications: Vec<_> = applied_filters(&filters)
            .into_iter()
            .map(|f| f.application)
            .collect();
        assert_eq!(
            applications,
            [
                FilterApplication::Server,
                FilterApplication::NotApplied,
                FilterApplication::NotApplied,
            ]
        );
        let review = Filters::default().with_doc_type(DocType::Review);
        assert_eq!(
            applied_filters(&review)[0].application,
            FilterApplication::Server
        );
    }

    #[test]
    fn citation_lists_filter_year_and_type_after_retrieval() {
        let filters = Filters::default()
            .with_years("2015-2020".parse().unwrap())
            .with_doc_type(DocType::ConferencePaper)
            .with_language("en");
        let applications: Vec<_> = citation_filters(&filters)
            .into_iter()
            .map(|f| f.application)
            .collect();
        assert_eq!(
            applications,
            [
                FilterApplication::PostRetrieval,
                FilterApplication::PostRetrieval,
                FilterApplication::NotApplied,
            ]
        );

        let mut matching = paper("Hit");
        matching.year = Some(2018);
        matching.publication_types = Some(vec!["Conference".to_string()]);
        assert!(keeps(&filters, &matching));

        let mut too_old = matching.clone();
        too_old.year = Some(2010);
        assert!(!keeps(&filters, &too_old));

        let mut journal = matching.clone();
        journal.publication_types = Some(vec!["JournalArticle".to_string()]);
        assert!(!keeps(&filters, &journal));

        assert!(!keeps(&filters, &paper("No year or type")));
        assert!(keeps(&Filters::default(), &paper("Unfiltered")));
    }

    #[test]
    fn list_pages_parse_leniently() {
        // An unmatched reference has a null paperId; a withheld list, null data.
        let body = r#"{"offset": 0, "data": [
            {"citedPaper": {"paperId": "abc", "title": "Matched", "year": 2017,
                            "publicationTypes": null, "externalIds": null}},
            {"citedPaper": {"paperId": null, "title": "Unmatched"}},
            {"citedPaper": null}
        ]}"#;
        let response: ListResponse = serde_json::from_str(body).unwrap();
        let papers: Vec<Paper> = response
            .data
            .unwrap()
            .into_iter()
            .filter_map(ListEntry::into_paper)
            .collect();
        assert_eq!(papers.len(), 1);
        assert_eq!(papers[0].paper_id, "abc");

        let citations = r#"{"data": [{"citingPaper": {"paperId": "def"}}]}"#;
        let response: ListResponse = serde_json::from_str(citations).unwrap();
        let entry = response.data.unwrap().into_iter().next().unwrap();
        assert_eq!(entry.into_paper().unwrap().paper_id, "def");

        let withheld: ListResponse = serde_json::from_str(r#"{"data": null}"#).unwrap();
        assert!(withheld.data.is_none());
    }

    #[test]
    fn recommendations_parse_leniently() {
        let body = r#"{"recommendedPapers": [
            {"paperId": "e42e3e09af8a56381c65aa8a8ddaee63fb321936", "title": "Transformer++",
             "year": 2020, "externalIds": {"ArXiv": "2003.04974"}},
            {"paperId": null, "title": "Unaddressable"},
            {"paperId": "2e7ddc611c414a0bf26e546f8dc743a5ac5842cf",
             "title": "Parallel Attention Mechanisms in Neural Machine Translation"}
        ]}"#;
        let response: RecommendationsResponse = serde_json::from_str(body).unwrap();
        let works: Vec<CandidateWork> = response
            .recommended_papers
            .into_iter()
            .filter_map(paper_with_id)
            .map(CandidateWork::from)
            .collect();
        let titles: Vec<&str> = works.iter().map(|w| w.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Transformer++",
                "Parallel Attention Mechanisms in Neural Machine Translation"
            ]
        );
        assert_eq!(
            works[0].pdf_url.as_deref(),
            Some("https://arxiv.org/pdf/2003.04974")
        );

        let empty: RecommendationsResponse =
            serde_json::from_str(r#"{"recommendedPapers": []}"#).unwrap();
        assert!(empty.recommended_papers.is_empty());
    }

    #[test]
    fn recommendations_ask_for_the_limit_or_all_when_filtered_after() {
        assert_eq!(recommendation_limit(Some(20), false), 20);
        assert_eq!(recommendation_limit(Some(2000), false), 500);
        assert_eq!(recommendation_limit(Some(20), true), 500);
        assert_eq!(recommendation_limit(None, false), 500);
    }

    #[test]
    fn a_missing_seed_is_not_found() {
        let missing = failure(StatusCode::NOT_FOUND, None, "/paper/x", "Paper not found");
        assert!(matches!(missing, Failure::Final(ProviderError::NotFound)));
    }

    #[test]
    fn rate_limits_and_server_errors_are_retried_and_bad_requests_are_not() {
        let wait = Some(Duration::from_secs(3));
        assert!(matches!(
            failure(StatusCode::TOO_MANY_REQUESTS, wait, "/paper/x", ""),
            Failure::Retryable { retry_after, .. } if retry_after == wait
        ));
        assert!(matches!(
            failure(StatusCode::BAD_GATEWAY, None, "/paper/x", ""),
            Failure::Retryable { .. }
        ));
        assert!(matches!(
            failure(StatusCode::BAD_REQUEST, None, "/paper/x", "bad fields"),
            Failure::Final(ProviderError::Request(m)) if m.contains("400") && m.contains("bad fields")
        ));
    }

    fn paper(title: &str) -> Paper {
        Paper {
            title: Some(title.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn pdf_url_comes_from_open_access_pdf() {
        let mut p = paper("On Computable Numbers");
        p.open_access_pdf = Some(OpenAccessPdf {
            url: Some("https://example.org/turing.pdf".to_string()),
            status: None,
        });
        let work = CandidateWork::from(p);
        assert_eq!(
            work.pdf_url.as_deref(),
            Some("https://example.org/turing.pdf")
        );
    }

    #[test]
    fn an_arxiv_id_gives_an_arxiv_pdf_over_open_access_pdf() {
        // ResNet's openAccessPdf is a repository landing page.
        let resnet: Paper = serde_json::from_str(
            r#"{
                "paperId": "2c03df8b48bf3fa39054345bafabfeff15bfd11d",
                "title": "Deep Residual Learning for Image Recognition",
                "externalIds": {"DOI": "10.1109/CVPR.2016.90", "ArXiv": "1512.03385"},
                "openAccessPdf": {
                    "url": "https://repositorio.unal.edu.co/handle/unal/81443",
                    "status": "GREEN"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            CandidateWork::from(resnet).pdf_url.as_deref(),
            Some("https://arxiv.org/pdf/1512.03385")
        );
    }

    #[test]
    fn without_an_arxiv_id_pdf_url_stays_open_access_pdf() {
        let mut p = paper("Communicating Sequential Processes");
        p.external_ids = Some(ExternalIds {
            doi: Some("10.1145/359576.359585".to_string()),
            ..Default::default()
        });
        p.open_access_pdf = Some(OpenAccessPdf {
            url: Some("https://example.org/csp.pdf".to_string()),
            status: None,
        });
        assert_eq!(
            CandidateWork::from(p).pdf_url.as_deref(),
            Some("https://example.org/csp.pdf")
        );
    }

    #[test]
    fn every_request_asks_for_external_ids() {
        // Search, lookup and the citation lists all request PAPER_FIELDS;
        // without externalIds there's no DOI and no arXiv PDF.
        assert!(PAPER_FIELDS.split(',').any(|field| field == "externalIds"));
    }

    #[test]
    fn lists_ask_for_the_abstract_only_when_asked_to() {
        assert_eq!(list_fields(&SearchOptions::default()), PAPER_FIELDS);
        let with = list_fields(&SearchOptions::default().with_abstracts(true));
        assert_eq!(with, format!("{PAPER_FIELDS},abstract"));
    }

    #[test]
    fn a_list_entry_keeps_its_abstract() {
        let body = r#"{"data": [{"citingPaper": {
            "paperId": "abc", "title": "Citing", "abstract": "We cite it."
        }}]}"#;
        let response: ListResponse = serde_json::from_str(body).unwrap();
        let paper = response.data.unwrap().into_iter().next().unwrap();
        let work = CandidateWork::from(paper.into_paper().unwrap());
        assert_eq!(work.abstract_text.as_deref(), Some("We cite it."));
    }

    #[test]
    fn pdf_url_is_none_when_not_open_access() {
        let work = CandidateWork::from(paper("Paywalled Paper"));
        assert_eq!(work.pdf_url, None);
    }

    #[test]
    fn a_list_with_null_data_is_withheld_with_a_readable_message() {
        let response: ListResponse = serde_json::from_str(r#"{"data":null}"#).unwrap();
        assert!(response.data.is_none());
        let error = withheld(CitationList::References);
        assert!(matches!(error, ProviderError::Withheld { .. }));
        assert_eq!(
            error.to_string(),
            "Semantic Scholar withholds this paper's references at the publisher's request"
        );
    }
}
