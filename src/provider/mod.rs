mod arxiv;
mod crossref;
mod openalex;
mod semantic_scholar;

pub use arxiv::ArxivProvider;
pub use crossref::CrossrefProvider;
pub use openalex::OpenAlexProvider;
pub use semantic_scholar::SemanticScholarProvider;

use std::fmt;
use std::future::Future;
use std::str::FromStr;
use thiserror::Error;

/// Shared result-count cap each provider's `search`/`search_by_author` asks
/// for explicitly, instead of leaving it unset and getting whatever small
/// default that provider's own API falls back to (OpenAlex 25, Crossref 20,
/// Semantic Scholar 10, arXiv 10 as of writing) — the gap between those
/// defaults and a subject with many matches (e.g. a broad CS query) is
/// exactly why `search`'s results can look sparser than the library actually
/// has to offer.
pub(crate) const SEARCH_RESULT_LIMIT: usize = 50;

/// How many results a paged search should retrieve from each provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Limit {
    /// At most this many results.
    Count(usize),
    /// Every result the provider will serve — still bounded by
    /// [`SearchPage::max_retrievable`] for providers that stop paging early.
    All,
}

/// Options for [`Provider::search_page`]. Construct with `default()` and the
/// `with_*` methods: new options (e.g. filters) can then be added without
/// breaking callers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SearchOptions {
    pub limit: Limit,
}

impl Default for SearchOptions {
    /// The same [`SEARCH_RESULT_LIMIT`] the unpaged `search` always used.
    fn default() -> Self {
        SearchOptions {
            limit: Limit::Count(SEARCH_RESULT_LIMIT),
        }
    }
}

impl SearchOptions {
    pub fn with_limit(mut self, limit: Limit) -> Self {
        self.limit = limit;
        self
    }
}

/// One provider's results for a paged search, with the counts needed to say
/// how much of the result set they are — so a cap is never silent.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SearchPage {
    pub works: Vec<CandidateWork>,
    /// How many records the provider reports matching in total. `None` when
    /// the provider doesn't report it.
    pub total: Option<u64>,
    /// The most this provider serves for one query however far it's paged
    /// (e.g. Semantic Scholar's relevance search stops at 1,000). `None` when
    /// there's no such ceiling.
    pub max_retrievable: Option<u64>,
}

impl SearchPage {
    pub fn new(
        works: Vec<CandidateWork>,
        total: Option<u64>,
        max_retrievable: Option<u64>,
    ) -> Self {
        SearchPage {
            works,
            total,
            max_retrievable,
        }
    }

    pub fn retrieved(&self) -> usize {
        self.works.len()
    }

    /// Whether every record the provider matched was retrieved. `false` when
    /// the total is unknown, since completeness can't be shown then.
    pub fn is_complete(&self) -> bool {
        self.total
            .is_some_and(|total| self.retrieved() as u64 >= total)
    }
}

/// Drives offset-based paging for a provider: calls `fetch_page(offset,
/// page_size)` until `options.limit`, the provider's reported total, or
/// `max_retrievable` is reached, or a page comes back short. `fetch_page`
/// returns one page of works plus the provider's reported total.
///
/// Every request uses the same page size, so offsets stay multiples of it —
/// which providers that page by page number (OpenAlex) rely on.
pub(crate) async fn collect_pages<F, Fut>(
    options: &SearchOptions,
    max_page_size: usize,
    max_retrievable: Option<u64>,
    mut fetch_page: F,
) -> Result<SearchPage, ProviderError>
where
    F: FnMut(usize, usize) -> Fut,
    Fut: Future<Output = Result<(Vec<CandidateWork>, u64), ProviderError>>,
{
    let wanted = match options.limit {
        Limit::Count(n) => Some(n),
        Limit::All => None,
    };
    if wanted == Some(0) {
        return Ok(SearchPage::new(Vec::new(), None, max_retrievable));
    }
    let page_size = wanted.map_or(max_page_size, |n| n.min(max_page_size));

    let mut works = Vec::new();
    let total = loop {
        let (page, total) = fetch_page(works.len(), page_size).await?;
        let fetched = page.len();
        works.extend(page);

        let ceiling = [wanted.map(|n| n as u64), Some(total), max_retrievable]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(total);
        if fetched < page_size || works.len() as u64 >= ceiling {
            break total;
        }
    };
    if let Some(n) = wanted {
        works.truncate(n);
    }
    Ok(SearchPage::new(works, Some(total), max_retrievable))
}

/// Which external metadata source a [`CandidateId`] or [`CandidateWork`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    OpenAlex,
    Crossref,
    SemanticScholar,
    ArXiv,
}

impl ProviderId {
    fn tag(self) -> &'static str {
        match self {
            ProviderId::OpenAlex => "openalex",
            ProviderId::Crossref => "crossref",
            ProviderId::SemanticScholar => "semanticscholar",
            ProviderId::ArXiv => "arxiv",
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tag())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("unknown provider: {0}")]
pub struct UnknownProviderTag(String);

impl FromStr for ProviderId {
    type Err = UnknownProviderTag;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "openalex" => Ok(ProviderId::OpenAlex),
            "crossref" => Ok(ProviderId::Crossref),
            "semanticscholar" => Ok(ProviderId::SemanticScholar),
            "arxiv" => Ok(ProviderId::ArXiv),
            other => Err(UnknownProviderTag(other.to_string())),
        }
    }
}

/// A fully-qualified, unambiguous reference to a single candidate work at a
/// single provider — e.g. `openalex:W2741809807` or `arxiv:2301.01234`.
///
/// This is the *only* reference `show`/`add` accept: it is always a provider's
/// own primary key, so resolving one is guaranteed to yield at most one
/// candidate. Deliberately does not accept a bare DOI, a title, or free text —
/// disambiguating "which paper did you mean" is `search`'s job, not this
/// type's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateId {
    pub provider: ProviderId,
    pub native_id: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CandidateIdParseError {
    #[error("candidate reference must be in the form 'provider:id', got: {0:?}")]
    MissingSeparator(String),
    #[error(transparent)]
    UnknownProvider(#[from] UnknownProviderTag),
    #[error("candidate reference has an empty id: {0:?}")]
    EmptyId(String),
}

impl fmt::Display for CandidateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.provider, self.native_id)
    }
}

impl FromStr for CandidateId {
    type Err = CandidateIdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (tag, native_id) = s
            .split_once(':')
            .ok_or_else(|| CandidateIdParseError::MissingSeparator(s.to_string()))?;
        if native_id.is_empty() {
            return Err(CandidateIdParseError::EmptyId(s.to_string()));
        }
        let provider = tag.parse::<ProviderId>()?;
        Ok(CandidateId {
            provider,
            native_id: native_id.to_string(),
        })
    }
}

/// An unresolved search hit from a single provider — not yet declared in the
/// local library. See [`CandidateId`] for how it's addressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateWork {
    pub id: CandidateId,
    pub title: String,
    pub authors: Vec<String>,
    pub publish_date: String,
    pub doi: Option<String>,
    pub pdf_url: Option<String>,
    pub venue: Option<String>,
    pub abstract_text: Option<String>,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("request failed: {0}")]
    Request(String),
    #[error("not found")]
    NotFound,
    #[error("fetching by id is not supported by this provider yet")]
    GetByIdUnsupported,
}

/// A single external metadata source PAX can search and resolve candidates
/// from. Implementations live one per module in this directory.
// Implementations are only ever awaited directly (never spawned onto another
// task), so the missing `Send` bound on the returned futures doesn't matter here.
#[allow(async_fn_in_trait)]
pub trait Provider {
    fn id(&self) -> ProviderId;
    async fn search(&self, query: &str) -> Result<Vec<CandidateWork>, ProviderError>;
    async fn get(&self, native_id: &str) -> Result<CandidateWork, ProviderError>;

    /// Author-scoped search. Default: falls back to plain full-text `search` —
    /// the right behavior for a provider with no true field-scoped author query
    /// (e.g. Semantic Scholar's search endpoint has no author filter).
    async fn search_by_author(&self, author: &str) -> Result<Vec<CandidateWork>, ProviderError> {
        self.search(author).await
    }

    /// Paged search reporting the provider's total. Default: a single
    /// unpaged `search`, trimmed to the limit, with the total unknown — every
    /// built-in provider overrides it with real paging.
    async fn search_page(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        let mut works = self.search(query).await?;
        if let Limit::Count(n) = options.limit {
            works.truncate(n);
        }
        Ok(SearchPage::new(works, None, None))
    }

    /// Paged counterpart of `search_by_author`. Default: falls back to
    /// `search_page`, mirroring `search_by_author`'s fallback to `search`.
    async fn search_by_author_page(
        &self,
        author: &str,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        self.search_page(author, options).await
    }

    /// Resolve directly by DOI. Default: not supported — a provider without any
    /// DOI-based lookup (e.g. arXiv) reports that explicitly rather than
    /// guessing at a fallback that wouldn't usefully match anything.
    async fn get_by_doi(&self, _doi: &str) -> Result<CandidateWork, ProviderError> {
        Err(ProviderError::GetByIdUnsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    /// Polls a future that never actually waits (the fake provider below
    /// answers immediately), so the paging tests need no async runtime.
    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("fake provider future should be ready immediately"),
        }
    }

    fn work(n: usize) -> CandidateWork {
        CandidateWork {
            id: CandidateId {
                provider: ProviderId::OpenAlex,
                native_id: format!("W{n}"),
            },
            title: format!("Paper {n}"),
            authors: Vec::new(),
            publish_date: String::new(),
            doi: None,
            pdf_url: None,
            venue: None,
            abstract_text: None,
        }
    }

    /// Pages through a fake provider holding `available` records out of a
    /// reported `total`, recording each request's (offset, page_size).
    fn page_through(
        options: SearchOptions,
        max_page_size: usize,
        max_retrievable: Option<u64>,
        available: usize,
        total: u64,
    ) -> (SearchPage, Vec<(usize, usize)>) {
        let mut requests = Vec::new();
        let page = block_on(collect_pages(
            &options,
            max_page_size,
            max_retrievable,
            |offset, size| {
                requests.push((offset, size));
                let end = (offset + size).min(available);
                let works = (offset.min(end)..end).map(work).collect();
                std::future::ready(Ok((works, total)))
            },
        ))
        .unwrap();
        (page, requests)
    }

    #[test]
    fn collect_pages_stops_at_the_limit() {
        let options = SearchOptions::default().with_limit(Limit::Count(250));
        let (page, requests) = page_through(options, 100, None, 1000, 1000);
        assert_eq!(page.retrieved(), 250);
        assert_eq!(page.total, Some(1000));
        assert!(!page.is_complete());
        assert_eq!(requests, vec![(0, 100), (100, 100), (200, 100)]);
    }

    #[test]
    fn collect_pages_uses_a_smaller_page_for_a_small_limit() {
        let options = SearchOptions::default().with_limit(Limit::Count(20));
        let (page, requests) = page_through(options, 100, None, 1000, 1000);
        assert_eq!(page.retrieved(), 20);
        assert_eq!(requests, vec![(0, 20)]);
    }

    #[test]
    fn collect_pages_all_stops_at_the_total() {
        let options = SearchOptions::default().with_limit(Limit::All);
        let (page, requests) = page_through(options, 100, None, 230, 230);
        assert_eq!(page.retrieved(), 230);
        assert!(page.is_complete());
        assert_eq!(requests.len(), 3);
    }

    #[test]
    fn collect_pages_all_stops_at_the_provider_ceiling() {
        let options = SearchOptions::default().with_limit(Limit::All);
        let (page, requests) = page_through(options, 100, Some(300), 5000, 5000);
        assert_eq!(page.retrieved(), 300);
        assert_eq!(page.max_retrievable, Some(300));
        assert!(!page.is_complete());
        assert_eq!(requests.len(), 3);
    }

    #[test]
    fn collect_pages_stops_on_a_short_page_even_if_the_total_says_more() {
        // Providers' totals can be estimates; a short page means there's no more.
        let options = SearchOptions::default().with_limit(Limit::All);
        let (page, requests) = page_through(options, 100, None, 150, 900);
        assert_eq!(page.retrieved(), 150);
        assert_eq!(requests.len(), 2);
    }

    #[test]
    fn collect_pages_with_zero_limit_makes_no_request() {
        let options = SearchOptions::default().with_limit(Limit::Count(0));
        let (page, requests) = page_through(options, 100, None, 1000, 1000);
        assert_eq!(page.retrieved(), 0);
        assert!(requests.is_empty());
    }

    #[test]
    fn candidate_id_round_trips() {
        let id = CandidateId {
            provider: ProviderId::OpenAlex,
            native_id: "W123".to_string(),
        };
        assert_eq!(id.to_string(), "openalex:W123");
        assert_eq!("openalex:W123".parse::<CandidateId>().unwrap(), id);
    }

    #[test]
    fn candidate_id_rejects_bare_doi() {
        assert!(matches!(
            "10.1145/foo".parse::<CandidateId>(),
            Err(CandidateIdParseError::MissingSeparator(_))
        ));
    }

    #[test]
    fn candidate_id_rejects_free_text() {
        assert!(matches!(
            "actor model".parse::<CandidateId>(),
            Err(CandidateIdParseError::MissingSeparator(_))
        ));
    }

    #[test]
    fn candidate_id_rejects_unknown_provider() {
        assert!(matches!(
            "doi:10.1145/foo".parse::<CandidateId>(),
            Err(CandidateIdParseError::UnknownProvider(_))
        ));
    }

    #[test]
    fn candidate_id_rejects_empty_id() {
        assert!(matches!(
            "openalex:".parse::<CandidateId>(),
            Err(CandidateIdParseError::EmptyId(_))
        ));
    }
}
