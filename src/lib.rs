//! `pax-core`: the paper-discovery, declaration, and library domain logic for
//! PAX. This is the primary artifact of this repository — any CLI or other
//! adapter (including the `pax` binary in `src/bin/pax/`, or a future
//! `lazypax` TUI) is a client of this library, not the other way around.

pub mod bibtex;
mod citation_key;
mod citations;
pub mod error;
pub mod exclusions;
pub mod github;
pub mod identity;
pub mod library;
pub mod library_index;
pub mod merge;
mod names;
pub mod nix;
mod nix_attrs;
pub mod paper;
mod process;
pub mod provider;
pub mod search_log;
pub mod stats;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub use citations::{
    CITATION_PROVIDERS, CitationError, CitationResults, Incomplete, IncompleteReason, RecordSource,
    SeedLookupError, SeedMergeReport, SeedMergedWork, SeedResult, SeedResults, cited_by,
    cited_by_many, incomplete_lists, incomplete_reason, references, references_many, related,
    related_many,
};
pub use error::PaxError;
pub use exclusions::{
    Exclusion, ExclusionStage, Exclusions, UnknownExclusionStage, candidate_exclusion_key,
    doi_exclusion_key, exclusion_key,
};
pub use github::publish_to_github_release;
pub use identity::{IdentityKey, identity_key, identity_keys};
pub use library::Library;
pub use library_index::LibraryIndex;
pub use merge::{MergeReport, MergedWork, merge_pages, merge_works};
pub use names::NameHolder;
pub use nix::init_library;
use paper::year_from_publish_date;
pub use paper::{Artifact, Identity, Local, Paper, PaperRef};
pub use provider::{
    AppliedFilter, ArxivProvider, CandidateId, CandidateWork, CrossrefProvider,
    DEFAULT_SEARCH_PROVIDERS, DblpProvider, DocType, FilterApplication, FilterKind, Filters, Limit,
    OpenAlexProvider, Provider, ProviderError, ProviderId, SearchOptions, SearchPage,
    SemanticScholarProvider, UnknownDocType, YearRange, YearRangeParseError,
};
pub use search_log::{
    LogOutcome, Logged, SearchCommand, SearchLog, SearchLogEntry, add_candidate_logged,
    cited_by_logged, cited_by_many_logged, log_results, references_logged, references_many_logged,
    related_logged, related_many_logged, search_all_logged, search_by_author_logged,
};
pub use stats::{
    CriterionCount, CriterionExclusions, FullTextExclusions, Identified, LabelCounts, ReasonCount,
    Stats, stats,
};

/// Per-installation settings a caller (e.g. the CLI, reading `.env`) supplies
/// so `pax-core` never reads the environment itself. Both fields are
/// optional: a missing Semantic Scholar key falls back to unauthenticated
/// requests, and a missing arXiv contact just omits it from the outgoing
/// User-Agent — neither is a hard requirement of the underlying APIs.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub semantic_scholar_api_key: Option<String>,
    pub arxiv_contact: Option<String>,
}

/// Searches every configured provider and aggregates results by provider,
/// retrieving up to the default limit from each. See [`search_all_paged`] for
/// a chosen limit and each provider's total.
pub async fn search_all(
    query: &str,
    config: &Config,
) -> HashMap<ProviderId, Result<Vec<CandidateWork>, ProviderError>> {
    works_only(search_all_paged(query, &SearchOptions::default(), config).await)
}

/// Same shape as [`search_all`], but scoped to an author name — each provider's
/// `search_by_author` decides whether that's a true field-scoped query or (for a
/// provider with no such endpoint) a fallback to plain full-text search.
pub async fn search_by_author(
    author: &str,
    config: &Config,
) -> HashMap<ProviderId, Result<Vec<CandidateWork>, ProviderError>> {
    works_only(search_by_author_paged(author, &SearchOptions::default(), config).await)
}

/// Searches the providers `options` names ([`SearchOptions::search_providers`]:
/// by default every general-purpose one), paging each up to `options.limit`,
/// and aggregates results by provider. A failing provider doesn't fail the
/// whole search — its error is reported alongside whatever providers did
/// succeed, so callers (e.g. the CLI) can decide how to surface it. Each
/// [`SearchPage`] carries the provider's total, so a caller can always say
/// how much of the result set it retrieved.
pub async fn search_all_paged(
    query: &str,
    options: &SearchOptions,
    config: &Config,
) -> HashMap<ProviderId, Result<SearchPage, ProviderError>> {
    search_each(query, SearchField::Text, options, config).await
}

/// Same shape as [`search_all_paged`], but scoped to an author name, the way
/// [`search_by_author`] is.
pub async fn search_by_author_paged(
    author: &str,
    options: &SearchOptions,
    config: &Config,
) -> HashMap<ProviderId, Result<SearchPage, ProviderError>> {
    search_each(author, SearchField::Author, options, config).await
}

/// What a search's input is matched against.
#[derive(Clone, Copy)]
enum SearchField {
    Text,
    Author,
}

/// Asks each of `options`' search providers in turn, one after another.
async fn search_each(
    input: &str,
    field: SearchField,
    options: &SearchOptions,
    config: &Config,
) -> HashMap<ProviderId, Result<SearchPage, ProviderError>> {
    let mut results = HashMap::new();
    for provider in options.search_providers() {
        let page = search_one(provider, input, field, options, config).await;
        results.insert(provider, page);
    }
    results
}

/// One provider's page for a search.
async fn search_one(
    provider: ProviderId,
    input: &str,
    field: SearchField,
    options: &SearchOptions,
    config: &Config,
) -> Result<SearchPage, ProviderError> {
    async fn page<P: Provider>(
        provider: P,
        input: &str,
        field: SearchField,
        options: &SearchOptions,
    ) -> Result<SearchPage, ProviderError> {
        match field {
            SearchField::Text => provider.search_page(input, options).await,
            SearchField::Author => provider.search_by_author_page(input, options).await,
        }
    }
    match provider {
        ProviderId::OpenAlex => page(OpenAlexProvider::new(), input, field, options).await,
        ProviderId::Crossref => page(CrossrefProvider::new()?, input, field, options).await,
        ProviderId::SemanticScholar => {
            let provider =
                SemanticScholarProvider::new(config.semantic_scholar_api_key.as_deref())?;
            page(provider, input, field, options).await
        }
        ProviderId::ArXiv => {
            let provider = ArxivProvider::new(config.arxiv_contact.as_deref())?;
            page(provider, input, field, options).await
        }
        ProviderId::Dblp => page(DblpProvider::new()?, input, field, options).await,
    }
}

/// Drops the paging counts, for the unpaged `search_*` functions.
fn works_only(
    pages: HashMap<ProviderId, Result<SearchPage, ProviderError>>,
) -> HashMap<ProviderId, Result<Vec<CandidateWork>, ProviderError>> {
    pages
        .into_iter()
        .map(|(provider, page)| (provider, page.map(|page| page.works)))
        .collect()
}

/// Resolves a DOI directly against every provider that supports it. Same
/// `HashMap` shape as [`search_all`] (each single resolved candidate wrapped in
/// a one-item `Vec`) so callers can reuse the same display code — a provider
/// with no DOI-based lookup (arXiv) reports `ProviderError::GetByIdUnsupported`
/// via `Provider::get_by_doi`'s default, the same as any other provider error.
pub async fn search_by_doi(
    doi: &str,
    config: &Config,
) -> HashMap<ProviderId, Result<Vec<CandidateWork>, ProviderError>> {
    let mut results = HashMap::new();

    let openalex = OpenAlexProvider::new();
    results.insert(
        ProviderId::OpenAlex,
        openalex.get_by_doi(doi).await.map(|work| vec![work]),
    );

    match CrossrefProvider::new() {
        Ok(crossref) => {
            results.insert(
                ProviderId::Crossref,
                crossref.get_by_doi(doi).await.map(|work| vec![work]),
            );
        }
        Err(e) => {
            results.insert(ProviderId::Crossref, Err(e));
        }
    }

    match SemanticScholarProvider::new(config.semantic_scholar_api_key.as_deref()) {
        Ok(semantic_scholar) => {
            results.insert(
                ProviderId::SemanticScholar,
                semantic_scholar
                    .get_by_doi(doi)
                    .await
                    .map(|work| vec![work]),
            );
        }
        Err(e) => {
            results.insert(ProviderId::SemanticScholar, Err(e));
        }
    }

    results
}

/// Resolves a single, already-unambiguous candidate reference by asking its
/// provider directly for that id — no search or disambiguation involved.
pub async fn resolve_candidate(
    id: &CandidateId,
    config: &Config,
) -> Result<CandidateWork, ProviderError> {
    match id.provider {
        ProviderId::OpenAlex => OpenAlexProvider::new().get(&id.native_id).await,
        ProviderId::Crossref => CrossrefProvider::new()?.get(&id.native_id).await,
        ProviderId::SemanticScholar => {
            SemanticScholarProvider::new(config.semantic_scholar_api_key.as_deref())?
                .get(&id.native_id)
                .await
        }
        ProviderId::ArXiv => {
            ArxivProvider::new(config.arxiv_contact.as_deref())?
                .get(&id.native_id)
                .await
        }
        ProviderId::Dblp => DblpProvider::new()?.get(&id.native_id).await,
    }
}

/// Resolves a candidate and declares it in the local library at `root`
/// (`research/papers.nix`) — metadata only. `Artifact.source_url` is the
/// resolved record's open PDF link; when it has none but has a DOI, the
/// other providers are asked for that DOI ([`search_by_doi`]) and one of
/// their PDF links is used instead: an arXiv-hosted PDF if any provider has
/// one, otherwise the first PDF link in [`merge::MERGE_ORDER`] (see
/// [`first_pdf_url`]). Errors in that fallback don't fail the
/// add — they just leave `source_url` unset. `hash` stays unset: fetching
/// bytes and computing a Nix hash is `fetch`'s job, not `add`'s (lazy
/// materialization, see `docs/mvp.md` at tag `1.0.0`).
///
/// The library is loaded before anything is resolved, so a missing library
/// fails without a network call. A paper already declared is refused with
/// [`PaxError::AlreadyDeclared`], naming its citation key: one with the same
/// DOI, or the same arXiv id in any version ([`identity_keys`]). A Crossref
/// or arXiv reference names its DOI or arXiv id itself, so it's checked
/// before it's resolved too.
///
/// A paper excluded in `excluded.nix` is refused the same way, with
/// [`PaxError::AlreadyExcluded`] naming the entry's key, stage and reason: it
/// matches by the same keys [`LibraryIndex::is_excluded`] flags a result by.
/// A paper can't be both included and excluded, so reversing the exclusion
/// is its own step, [`unexclude_paper`]. Either refusal comes before any
/// write.
///
/// The generated citation key is disambiguated against every name already
/// taken: other citation keys, and the aliases in `papers.nix` and
/// `excluded.nix`, which share one namespace with them.
///
/// To also record the add in the search log, so `stats` counts the paper as
/// identified from another source, see [`add_candidate_logged`].
pub async fn add_candidate(
    id: &CandidateId,
    root: &Path,
    config: &Config,
) -> Result<PaperRef, PaxError> {
    declare_candidate(id, root, config)
        .await
        .map(|(paper_ref, _)| paper_ref)
}

/// [`add_candidate`], returning the resolved record too.
pub(crate) async fn declare_candidate(
    id: &CandidateId,
    root: &Path,
    config: &Config,
) -> Result<(PaperRef, CandidateWork), PaxError> {
    let path = nix::papers_path(root);
    let mut library = Library::load(&path)?;
    let exclusions = Exclusions::load(&nix::excluded_path(root))?;
    refuse_declared(&library, id, &identity::reference_id_keys(id))?;
    refuse_excluded(
        &exclusions,
        id,
        &identity::reference_id_keys(id),
        &[candidate_exclusion_key(id)],
    )?;

    let work = resolve_candidate(id, config).await?;
    refuse_declared(&library, id, &identity_keys(&work))?;
    let (id_keys, mut keys) = library_index::work_exclusion_keys(&work);
    keys.push(candidate_exclusion_key(id));
    refuse_excluded(&exclusions, id, &id_keys, &keys)?;
    let source_url = match (&work.pdf_url, &work.doi) {
        (Some(pdf_url), _) => Some(pdf_url.clone()),
        (None, Some(doi)) => first_pdf_url(
            &search_by_doi(normalize_doi(doi), config).await,
            id.provider,
        ),
        (None, None) => None,
    };

    // Every name already taken, so the new key never equals an alias either.
    let taken: Vec<&str> = library
        .papers()
        .iter()
        .flat_map(|p| {
            [
                Some(p.local.citation_key.as_str()),
                p.local.alias.as_deref(),
            ]
        })
        .chain(exclusions.entries().iter().map(|e| e.alias.as_deref()))
        .flatten()
        .collect();
    let citation_key = citation_key::generate(&work, &taken);

    library.insert(Paper {
        identity: Identity {
            doi: work.doi.clone(),
            title: work.title.clone(),
            authors: work.authors.clone(),
            year: year_from_publish_date(&work.publish_date),
            venue: work.venue.clone(),
        },
        artifact: Artifact {
            source_url,
            hash: None,
        },
        local: Local {
            citation_key: citation_key.clone(),
            tags: Vec::new(),
            notes: None,
            alias: None,
        },
    });
    library.save(&path)?;

    Ok((PaperRef(citation_key), work))
}

/// Refuses adding `id` when a declared paper shares a DOI or arXiv key with
/// `keys`. A title+year key doesn't count, since a declared paper's keys
/// never include one: different papers can share a title and year.
fn refuse_declared(
    library: &Library,
    id: &CandidateId,
    keys: &[IdentityKey],
) -> Result<(), PaxError> {
    let declared = library.papers().iter().find(|paper| {
        identity::paper_id_keys(paper)
            .iter()
            .any(|declared| keys.contains(declared))
    });
    match declared {
        Some(paper) => Err(PaxError::AlreadyDeclared {
            reference: id.to_string(),
            citation_key: paper.local.citation_key.clone(),
        }),
        None => Ok(()),
    }
}

/// Refuses adding `id` when an entry in `exclusions` matches the DOI and
/// arXiv keys `id_keys` or the exclusion keys `keys`
/// ([`library_index::find_exclusion`]).
fn refuse_excluded(
    exclusions: &Exclusions,
    id: &CandidateId,
    id_keys: &[IdentityKey],
    keys: &[String],
) -> Result<(), PaxError> {
    match library_index::find_exclusion(exclusions, id_keys, keys) {
        Some(exclusion) => Err(PaxError::AlreadyExcluded {
            reference: id.to_string(),
            key: exclusion.key.clone(),
            stage: exclusion.stage,
            reason: exclusion.reason.clone(),
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod refuse_declared_tests {
    use super::*;

    fn library() -> Library {
        let paper = |key: &str, doi: Option<&str>, source_url: Option<&str>| Paper {
            identity: Identity {
                doi: doi.map(str::to_string),
                title: "A Study".to_string(),
                year: Some(2016),
                ..Identity::default()
            },
            artifact: Artifact {
                source_url: source_url.map(str::to_string),
                hash: None,
            },
            local: Local {
                citation_key: key.to_string(),
                ..Local::default()
            },
        };
        Library::new(vec![
            paper("he2016", Some("https://doi.org/10.1109/CVPR.2016.90"), None),
            paper(
                "vaswani2017",
                None,
                Some("https://arxiv.org/pdf/1706.03762v5"),
            ),
        ])
    }

    fn work(provider: ProviderId, native_id: &str, doi: Option<&str>) -> CandidateWork {
        CandidateWork {
            id: CandidateId {
                provider,
                native_id: native_id.to_string(),
            },
            title: "A Study".to_string(),
            authors: Vec::new(),
            publish_date: "2016".to_string(),
            doi: doi.map(str::to_string),
            pdf_url: None,
            venue: None,
            abstract_text: None,
        }
    }

    fn refused_as(id: &str, keys: &[IdentityKey]) -> Option<String> {
        let id: CandidateId = id.parse().unwrap();
        let error = refuse_declared(&library(), &id, keys).err()?;
        match error {
            PaxError::AlreadyDeclared { citation_key, .. } => Some(citation_key),
            _ => panic!("not a refusal: {error}"),
        }
    }

    #[test]
    fn the_same_doi_from_any_provider_is_refused_with_its_key() {
        let resolved = work(ProviderId::OpenAlex, "W1", Some("10.1109/cvpr.2016.90"));
        assert_eq!(
            refused_as("openalex:W1", &identity_keys(&resolved)).as_deref(),
            Some("he2016")
        );
        let id: CandidateId = "crossref:10.1109/CVPR.2016.90".parse().unwrap();
        assert_eq!(
            refused_as(
                "crossref:10.1109/CVPR.2016.90",
                &identity::reference_id_keys(&id)
            )
            .as_deref(),
            Some("he2016")
        );
    }

    #[test]
    fn another_arxiv_version_is_the_same_paper() {
        let id: CandidateId = "arxiv:1706.03762v2".parse().unwrap();
        assert_eq!(
            refused_as("arxiv:1706.03762v2", &identity::reference_id_keys(&id)).as_deref(),
            Some("vaswani2017")
        );
        let preprint_doi = work(
            ProviderId::SemanticScholar,
            "abc",
            Some("10.48550/arXiv.1706.03762"),
        );
        assert_eq!(
            refused_as("semanticscholar:abc", &identity_keys(&preprint_doi)).as_deref(),
            Some("vaswani2017")
        );
    }

    #[test]
    fn a_different_paper_with_the_same_title_and_year_is_not_refused() {
        let other = work(ProviderId::OpenAlex, "W2", Some("10.1/other"));
        assert_eq!(refused_as("openalex:W2", &identity_keys(&other)), None);
        let no_doi = work(ProviderId::OpenAlex, "W3", None);
        assert_eq!(refused_as("openalex:W3", &identity_keys(&no_doi)), None);
    }

    #[test]
    fn a_refusal_names_the_reference_and_key() {
        let id: CandidateId = "crossref:10.1109/CVPR.2016.90".parse().unwrap();
        let error =
            refuse_declared(&library(), &id, &identity::reference_id_keys(&id)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "crossref:10.1109/CVPR.2016.90 is already in the library as \"he2016\""
        );
    }
}

/// The open PDF link `add` falls back to, among `results` from providers
/// other than `skip` (the one `add` already resolved against). An
/// arXiv-hosted PDF ([`is_arxiv_pdf`]) is preferred, since other providers'
/// links are sometimes landing pages rather than PDFs; otherwise the first
/// PDF link wins. Both passes take providers in [`merge::MERGE_ORDER`].
/// Failed providers are skipped.
fn first_pdf_url(
    results: &HashMap<ProviderId, Result<Vec<CandidateWork>, ProviderError>>,
    skip: ProviderId,
) -> Option<String> {
    let pdf_urls = || {
        merge::MERGE_ORDER
            .iter()
            .filter(|provider| **provider != skip)
            .filter_map(|provider| results.get(provider)?.as_ref().ok())
            .flatten()
            .filter_map(|work| work.pdf_url.as_deref())
    };
    pdf_urls()
        .find(|url| is_arxiv_pdf(url))
        .or_else(|| pdf_urls().next())
        .map(str::to_string)
}

/// Whether `url` is a PDF served by arXiv: host `arxiv.org` or
/// `export.arxiv.org`, path under `/pdf/` (not an `/abs/` landing page).
fn is_arxiv_pdf(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    matches!(
        host.to_ascii_lowercase().as_str(),
        "arxiv.org" | "export.arxiv.org"
    ) && path.starts_with("/pdf/")
}

#[cfg(test)]
mod first_pdf_url_tests {
    use super::*;

    fn work(provider: ProviderId, pdf_url: Option<&str>) -> CandidateWork {
        CandidateWork {
            id: CandidateId {
                provider,
                native_id: "x".to_string(),
            },
            title: "A Study".to_string(),
            authors: Vec::new(),
            publish_date: "2020".to_string(),
            doi: Some("10.1/a".to_string()),
            pdf_url: pdf_url.map(str::to_string),
            venue: None,
            abstract_text: None,
        }
    }

    #[test]
    fn takes_the_first_pdf_in_provider_order() {
        let mut results = HashMap::new();
        results.insert(
            ProviderId::SemanticScholar,
            Ok(vec![work(
                ProviderId::SemanticScholar,
                Some("https://s2/a.pdf"),
            )]),
        );
        results.insert(
            ProviderId::Crossref,
            Ok(vec![work(
                ProviderId::Crossref,
                Some("https://crossref/a.pdf"),
            )]),
        );
        results.insert(
            ProviderId::OpenAlex,
            Ok(vec![work(ProviderId::OpenAlex, None)]),
        );
        assert_eq!(
            first_pdf_url(&results, ProviderId::ArXiv).as_deref(),
            Some("https://crossref/a.pdf")
        );
    }

    #[test]
    fn skips_the_resolving_provider_and_failed_providers() {
        let mut results = HashMap::new();
        results.insert(
            ProviderId::OpenAlex,
            Ok(vec![work(
                ProviderId::OpenAlex,
                Some("https://openalex/a.pdf"),
            )]),
        );
        results.insert(ProviderId::Crossref, Err(ProviderError::NotFound));
        results.insert(
            ProviderId::SemanticScholar,
            Ok(vec![work(
                ProviderId::SemanticScholar,
                Some("https://s2/a.pdf"),
            )]),
        );
        assert_eq!(
            first_pdf_url(&results, ProviderId::OpenAlex).as_deref(),
            Some("https://s2/a.pdf")
        );
    }

    #[test]
    fn no_pdf_anywhere_gives_none() {
        let mut results = HashMap::new();
        results.insert(ProviderId::OpenAlex, Err(ProviderError::NotFound));
        results.insert(
            ProviderId::Crossref,
            Ok(vec![work(ProviderId::Crossref, None)]),
        );
        assert_eq!(first_pdf_url(&results, ProviderId::SemanticScholar), None);
        assert_eq!(first_pdf_url(&HashMap::new(), ProviderId::OpenAlex), None);
    }

    #[test]
    fn prefers_an_arxiv_pdf_from_a_later_provider() {
        let mut results = HashMap::new();
        results.insert(
            ProviderId::OpenAlex,
            Ok(vec![work(
                ProviderId::OpenAlex,
                Some("https://repository.example/handle/123"),
            )]),
        );
        results.insert(
            ProviderId::SemanticScholar,
            Ok(vec![work(
                ProviderId::SemanticScholar,
                Some("https://arxiv.org/pdf/1512.03385"),
            )]),
        );
        assert_eq!(
            first_pdf_url(&results, ProviderId::Crossref).as_deref(),
            Some("https://arxiv.org/pdf/1512.03385")
        );
    }

    #[test]
    fn without_an_arxiv_pdf_provider_order_applies() {
        let mut results = HashMap::new();
        results.insert(
            ProviderId::OpenAlex,
            Ok(vec![work(
                ProviderId::OpenAlex,
                Some("https://openalex/a.pdf"),
            )]),
        );
        results.insert(
            ProviderId::SemanticScholar,
            Ok(vec![work(
                ProviderId::SemanticScholar,
                Some("https://s2/a.pdf"),
            )]),
        );
        assert_eq!(
            first_pdf_url(&results, ProviderId::Crossref).as_deref(),
            Some("https://openalex/a.pdf")
        );
    }

    #[test]
    fn an_arxiv_abstract_page_is_not_an_arxiv_pdf() {
        let mut results = HashMap::new();
        results.insert(
            ProviderId::OpenAlex,
            Ok(vec![work(
                ProviderId::OpenAlex,
                Some("https://openalex/a.pdf"),
            )]),
        );
        results.insert(
            ProviderId::SemanticScholar,
            Ok(vec![work(
                ProviderId::SemanticScholar,
                Some("https://arxiv.org/abs/1512.03385"),
            )]),
        );
        assert_eq!(
            first_pdf_url(&results, ProviderId::Crossref).as_deref(),
            Some("https://openalex/a.pdf")
        );
        assert!(!is_arxiv_pdf("https://arxiv.org/abs/1512.03385"));
        assert!(is_arxiv_pdf("http://export.arxiv.org/pdf/1512.03385v1"));
        assert!(!is_arxiv_pdf("https://notarxiv.org/pdf/1512.03385"));
    }
}

/// Strips a leading `https://doi.org/` from a DOI, if present. Providers
/// disagree on format — OpenAlex returns a full URL, Crossref/Semantic
/// Scholar/arXiv return a bare DOI — so comparing two DOIs for the same
/// paper (e.g. to detect "already in library") needs this first, or a
/// match can be silently missed depending on which provider each one came
/// from.
pub fn normalize_doi(doi: &str) -> &str {
    doi.trim_start_matches("https://doi.org/")
}

/// The set of (normalized) DOIs already declared in the local library, used
/// to mark `search` results that are already `add`ed. Returns an empty set
/// rather than an error when the library can't be loaded (e.g. `search`
/// before `init`) — search should still work, just with nothing marked as
/// declared.
#[deprecated(
    note = "matches by DOI only; use `LibraryIndex::load(root).contains(work)`, which also matches a paper by its version-less arXiv id"
)]
pub fn known_dois(root: &Path) -> HashSet<String> {
    Library::load(&nix::papers_path(root))
        .map(|library| {
            library
                .papers()
                .iter()
                .filter_map(|p| p.identity.doi.as_deref())
                .map(|doi| normalize_doi(doi).to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The exclusion keys in the library's `research/excluded.nix`, used to mark
/// results already excluded: a work is excluded when
/// [`exclusion_key`]`(work)` is in this set. Like [`known_dois`], returns an
/// empty set rather than an error when the file is missing or unreadable, so
/// a search still works with nothing marked.
#[deprecated(
    note = "matches one exact key; use `LibraryIndex::load(root).is_excluded(work)`, which matches every version of a paper and the paper from every provider"
)]
pub fn excluded_keys(root: &Path) -> HashSet<String> {
    Exclusions::load(&nix::excluded_path(root))
        .map(|exclusions| exclusions.entries().iter().map(|e| e.key.clone()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(deprecated)]
mod known_dois_tests {
    use super::*;
    use std::fs;

    #[test]
    fn normalize_doi_strips_url_prefix() {
        assert_eq!(
            normalize_doi("https://doi.org/10.1145/3474085.3475385"),
            "10.1145/3474085.3475385"
        );
        assert_eq!(
            normalize_doi("10.1145/3474085.3475385"),
            "10.1145/3474085.3475385"
        );
    }

    #[test]
    fn known_dois_normalizes_and_ignores_papers_without_a_doi() {
        let root = std::env::temp_dir().join(format!("pax-known-dois-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("research")).unwrap();
        fs::write(
            nix::papers_path(&root),
            r#"{
  withDoi = {
    doi = "https://doi.org/10.1145/3474085.3475385";
    title = "Has a DOI";
    authors = [ ];
    year = null;
    source_url = null;
    hash = null;
    tags = [ ];
    notes = null;
  };
  withoutDoi = {
    doi = null;
    title = "No DOI";
    authors = [ ];
    year = null;
    source_url = null;
    hash = null;
    tags = [ ];
    notes = null;
  };
}
"#,
        )
        .unwrap();

        let dois = known_dois(&root);
        assert_eq!(dois.len(), 1);
        assert!(dois.contains("10.1145/3474085.3475385"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn known_dois_is_empty_when_library_cannot_be_loaded() {
        let root =
            std::env::temp_dir().join(format!("pax-known-dois-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        assert!(known_dois(&root).is_empty());
    }
}

/// Searches the local library without querying any provider (§2.8 of `docs/mvp.md` at tag `1.0.0`).
/// Matches case-insensitively against title, authors, DOI, year, venue, tags, and
/// citation key. Returns an empty `Vec` (not an error) when the library can't be
/// loaded, same as `known_dois`.
pub fn search_local(query: &str, root: &Path) -> Vec<Paper> {
    let query = query.to_lowercase();
    Library::load(&nix::papers_path(root))
        .map(|library| {
            library
                .papers()
                .iter()
                .filter(|p| {
                    p.identity.title.to_lowercase().contains(&query)
                        || p.identity
                            .authors
                            .iter()
                            .any(|a| a.to_lowercase().contains(&query))
                        || p.identity
                            .doi
                            .as_deref()
                            .is_some_and(|d| d.to_lowercase().contains(&query))
                        || p.identity.year.is_some_and(|y| y.to_string() == query)
                        || p.identity
                            .venue
                            .as_deref()
                            .is_some_and(|v| v.to_lowercase().contains(&query))
                        || p.local
                            .tags
                            .iter()
                            .any(|t| t.to_lowercase().contains(&query))
                        || p.local.citation_key.to_lowercase().contains(&query)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Filter criteria for `pax list` (§2.7 of `docs/mvp.md` at tag `1.0.0`). All fields combine with AND;
/// a `None` field matches everything.
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub author: Option<String>,
    pub year: Option<i32>,
    pub tag: Option<String>,
}

/// Applies a [`ListFilter`] to an already-loaded set of papers. Author matching
/// is a case-insensitive substring match (like `search_local`); tag matching is
/// exact, since tags are a short controlled vocabulary the user themselves wrote.
pub fn filter_papers(papers: &[Paper], filter: &ListFilter) -> Vec<Paper> {
    papers
        .iter()
        .filter(|p| {
            filter.author.as_deref().is_none_or(|a| {
                let a = a.to_lowercase();
                p.identity
                    .authors
                    .iter()
                    .any(|au| au.to_lowercase().contains(&a))
            }) && filter.year.is_none_or(|y| p.identity.year == Some(y))
                && filter
                    .tag
                    .as_deref()
                    .is_none_or(|t| p.local.tags.iter().any(|tag| tag == t))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
#[allow(deprecated)]
mod excluded_keys_tests {
    use super::*;
    use std::fs;

    #[test]
    fn excluded_keys_reads_excluded_nix_and_matches_exclusion_key() {
        let root =
            std::env::temp_dir().join(format!("pax-excluded-keys-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("research")).unwrap();
        assert!(excluded_keys(&root).is_empty());

        fs::write(
            nix::excluded_path(&root),
            r#"{
  "10.1145/abc" = { title = "T"; reason = "R"; stage = "screening"; };
}
"#,
        )
        .unwrap();
        let keys = excluded_keys(&root);
        let work = CandidateWork {
            id: "openalex:W1".parse().unwrap(),
            title: "T".to_string(),
            authors: Vec::new(),
            publish_date: String::new(),
            doi: Some("https://doi.org/10.1145/ABC".to_string()),
            pdf_url: None,
            venue: None,
            abstract_text: None,
        };
        assert!(keys.contains(&exclusion_key(&work)));

        fs::remove_dir_all(&root).ok();
    }
}

#[cfg(test)]
mod exclude_paper_tests {
    use super::*;
    use std::fs;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    /// Polls a future that never waits: these tests only take the paths that
    /// need no network (a bare DOI with a title, or an early error).
    fn block_on<F: Future>(future: F) -> F::Output {
        match pin!(future)
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("exclude_paper unexpectedly waited on the network"),
        }
    }

    fn library(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("pax-exclude-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("research")).unwrap();
        fs::write(
            nix::papers_path(&root),
            r#"{
  lamport1978 = {
    doi = "10.1145/359545.359563";
    title = "Time, Clocks, and the Ordering of Events in a Distributed System";
    authors = [ "Leslie Lamport" ];
    year = 1978;
    source_url = null;
    hash = null;
    tags = [ ];
    notes = null;
  };
}
"#,
        )
        .unwrap();
        root
    }

    fn exclude(
        root: &Path,
        reference: &str,
        reason: &str,
        stage: ExclusionStage,
        title: Option<&str>,
    ) -> Result<Exclusion, PaxError> {
        block_on(exclude_paper(
            reference,
            reason,
            stage,
            title,
            root,
            &Config::default(),
        ))
    }

    #[test]
    fn a_doi_with_a_title_is_excluded_without_the_network_and_re_excluding_updates_it() {
        let root = library("doi");
        let first = exclude(
            &root,
            "https://doi.org/10.1000/ABC",
            "not empirical",
            ExclusionStage::Screening,
            Some("Some paper"),
        )
        .unwrap();
        assert_eq!(first.key, "10.1000/abc");

        exclude(
            &root,
            "10.1000/abc",
            "wrong domain",
            ExclusionStage::FullText,
            Some("Some paper"),
        )
        .unwrap();
        let exclusions = Exclusions::load(&nix::excluded_path(&root)).unwrap();
        assert_eq!(exclusions.entries().len(), 1);
        let entry = exclusions.get("10.1000/abc").unwrap();
        assert_eq!(entry.reason, "wrong domain");
        assert_eq!(entry.stage, ExclusionStage::FullText);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_given_year_is_recorded_without_the_network() {
        let root = library("year");
        let options = ExclusionOptions::new("not empirical", ExclusionStage::Screening)
            .with_title(Some("A grey-literature report".to_string()))
            .with_year(Some(2019));
        let excluded = block_on(exclude_paper_with(
            "10.1000/report",
            &options,
            &root,
            &Config::default(),
        ))
        .unwrap();
        assert_eq!(excluded.year, Some(2019));
        let exclusions = Exclusions::load(&nix::excluded_path(&root)).unwrap();
        assert_eq!(exclusions.get("10.1000/report").unwrap().year, Some(2019));

        // Without a year, a bare DOI with a title records none.
        let without = exclude(
            &root,
            "10.1000/other",
            "wrong domain",
            ExclusionStage::Screening,
            Some("Other paper"),
        )
        .unwrap();
        assert_eq!(without.year, None);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_criterion_and_round_are_recorded_and_re_excluding_without_them_clears_them() {
        let root = library("criterion");
        let options = ExclusionOptions::new("not empirical", ExclusionStage::Screening)
            .with_title(Some("Some paper".to_string()))
            .with_criterion(Some(" X3 ".to_string()))
            .with_round(Some("R1".to_string()));
        let excluded = block_on(exclude_paper_with(
            "10.1000/abc",
            &options,
            &root,
            &Config::default(),
        ))
        .unwrap();
        assert_eq!(excluded.criterion.as_deref(), Some("X3"));
        let entry = Exclusions::load(&nix::excluded_path(&root)).unwrap();
        let entry = entry.get("10.1000/abc").unwrap();
        assert_eq!(entry.criterion.as_deref(), Some("X3"));
        assert_eq!(entry.round.as_deref(), Some("R1"));

        let blank = options
            .with_criterion(Some("  ".to_string()))
            .with_round(None);
        block_on(exclude_paper_with(
            "10.1000/abc",
            &blank,
            &root,
            &Config::default(),
        ))
        .unwrap();
        let entry = Exclusions::load(&nix::excluded_path(&root)).unwrap();
        let entry = entry.get("10.1000/abc").unwrap();
        assert_eq!(
            (entry.criterion.as_deref(), entry.round.as_deref()),
            (None, None)
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_declared_paper_cannot_be_excluded() {
        let root = library("declared");
        let result = exclude(
            &root,
            "10.1145/359545.359563",
            "duplicate",
            ExclusionStage::Screening,
            Some("Time, Clocks"),
        );
        assert!(matches!(
            result,
            Err(PaxError::ExcludingDeclaredPaper(_, key)) if key == "lamport1978"
        ));
        assert!(!nix::excluded_path(&root).exists());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_empty_reason_is_refused() {
        let root = library("no-reason");
        assert!(matches!(
            exclude(&root, "10.1/x", "  ", ExclusionStage::Screening, Some("T")),
            Err(PaxError::MissingExclusionReason)
        ));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn free_text_is_not_a_reference() {
        let root = library("free-text");
        assert!(matches!(
            exclude(
                &root,
                "actor model",
                "R",
                ExclusionStage::Screening,
                Some("T")
            ),
            Err(PaxError::InvalidExclusionReference(_))
        ));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn titles_match_regardless_of_case_and_punctuation() {
        assert_eq!(
            normalized_title("Time, Clocks — and the  Ordering"),
            normalized_title("time clocks and the ordering")
        );
    }
}

#[cfg(test)]
mod excluded_add_tests {
    use super::*;
    use std::fs;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    /// Polls a future that never waits: these tests only take the paths that
    /// need no network (a refusal before resolving, or a local match).
    fn block_on<F: Future>(future: F) -> F::Output {
        match pin!(future)
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("unexpectedly waited on the network"),
        }
    }

    const PAPERS: &str = "{\n}\n";

    /// An empty library with three exclusions: by DOI, by version-less arXiv
    /// id, and by an opaque `provider:id`.
    fn library(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pax-excluded-add-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("research")).unwrap();
        fs::write(nix::papers_path(&root), PAPERS).unwrap();
        fs::write(
            nix::excluded_path(&root),
            r#"{
  "10.1/abc" = { title = "A"; reason = "not empirical"; stage = "screening"; };
  "arxiv:2301.01234" = { title = "B"; reason = "wrong domain"; stage = "full-text"; };
  "openalex:W123" = { title = "C"; reason = "duplicate"; stage = "screening"; };
}
"#,
        )
        .unwrap();
        root
    }

    fn add(root: &Path, reference: &str) -> Result<PaperRef, PaxError> {
        block_on(add_candidate(
            &reference.parse().unwrap(),
            root,
            &Config::default(),
        ))
    }

    fn unexclude(root: &Path, reference: &str) -> Result<Exclusion, PaxError> {
        block_on(unexclude_paper(reference, root, &Config::default()))
    }

    fn excluded_keys_in(root: &Path) -> Vec<String> {
        Exclusions::load(&nix::excluded_path(root))
            .unwrap()
            .entries()
            .iter()
            .map(|e| e.key.clone())
            .collect()
    }

    #[test]
    fn an_excluded_paper_is_refused_by_doi_arxiv_id_in_any_version_and_provider_key() {
        let root = library("refused");
        for (reference, expected) in [
            ("crossref:10.1/abc", "10.1/abc"),
            ("crossref:https://doi.org/10.1/ABC", "10.1/abc"),
            ("arxiv:2301.01234v3", "arxiv:2301.01234"),
            ("arxiv:2301.01234", "arxiv:2301.01234"),
            ("openalex:W123", "openalex:W123"),
        ] {
            match add(&root, reference) {
                Err(PaxError::AlreadyExcluded { key, .. }) => {
                    assert_eq!(key, expected, "{reference}")
                }
                other => panic!("{reference}: {other:?}"),
            }
        }
        assert_eq!(
            fs::read_to_string(nix::papers_path(&root)).unwrap(),
            PAPERS,
            "papers.nix is untouched"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_refusal_names_the_stage_reason_and_how_to_reverse_it() {
        let root = library("message");
        let error = add(&root, "arxiv:2301.01234v2").unwrap_err();
        assert_eq!(
            error.to_string(),
            "arxiv:2301.01234v2 was excluded at full-text (\"wrong domain\", key \
             arxiv:2301.01234); run `pax unexclude arxiv:2301.01234` first to reverse that \
             decision"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unexclude_removes_exactly_that_entry() {
        let root = library("unexclude");
        let removed = unexclude(&root, "arxiv:2301.01234v5").unwrap();
        assert_eq!(removed.key, "arxiv:2301.01234");
        assert_eq!(removed.reason, "wrong domain");
        assert_eq!(excluded_keys_in(&root), ["10.1/abc", "openalex:W123"]);

        assert_eq!(
            unexclude(&root, "https://doi.org/10.1/ABC").unwrap().key,
            "10.1/abc"
        );
        assert_eq!(
            unexclude(&root, "openalex:W123").unwrap().key,
            "openalex:W123"
        );
        assert!(excluded_keys_in(&root).is_empty());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unexcluding_what_isnt_excluded_is_not_excluded() {
        let root = library("not-excluded");
        for reference in ["10.1/other", "crossref:10.1/other"] {
            assert!(
                matches!(
                    unexclude(&root, reference),
                    Err(PaxError::NotExcluded(r)) if r == reference
                ),
                "{reference}"
            );
        }
        assert!(matches!(
            unexclude(&root, "actor model"),
            Err(PaxError::InvalidExclusionReference(_))
        ));
        assert_eq!(excluded_keys_in(&root).len(), 3);
        fs::remove_dir_all(&root).ok();
    }
}

#[cfg(test)]
mod local_filter_tests {
    use super::*;
    use std::fs;

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pax-local-filter-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("research")).unwrap();
        dir
    }

    fn two_papers_fixture() -> &'static str {
        r#"{
  turing1936 = {
    doi = "10.1112/plms/s2-42.1.230";
    title = "On Computable Numbers";
    authors = [ "Alan Turing" ];
    year = 1936;
    venue = "Proceedings of the London Mathematical Society";
    source_url = null;
    hash = null;
    tags = [ "computability" "logic" ];
    notes = null;
  };
  hewitt1973 = {
    doi = null;
    title = "A Universal Modular Actor Formalism";
    authors = [ "Carl Hewitt" "Peter Bishop" "Richard Steiger" ];
    year = 1973;
    venue = null;
    source_url = null;
    hash = null;
    tags = [ "concurrency" ];
    notes = null;
  };
}
"#
    }

    #[test]
    fn search_local_matches_across_documented_fields() {
        let root = scratch_dir("matches");
        fs::write(nix::papers_path(&root), two_papers_fixture()).unwrap();

        assert_eq!(search_local("computable", &root).len(), 1); // title
        assert_eq!(search_local("hewitt", &root).len(), 1); // author
        assert_eq!(search_local("10.1112", &root).len(), 1); // doi
        assert_eq!(search_local("1973", &root).len(), 1); // year
        assert_eq!(search_local("mathematical society", &root).len(), 1); // venue
        assert_eq!(search_local("concurrency", &root).len(), 1); // tag
        assert_eq!(search_local("turing1936", &root).len(), 1); // citation key
        assert_eq!(search_local("nonexistent", &root).len(), 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn search_local_is_case_insensitive() {
        let root = scratch_dir("case-insensitive");
        fs::write(nix::papers_path(&root), two_papers_fixture()).unwrap();
        assert_eq!(search_local("HEWITT", &root).len(), 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn filter_papers_combines_criteria_with_and() {
        let root = scratch_dir("filter-and");
        fs::write(nix::papers_path(&root), two_papers_fixture()).unwrap();
        let papers = Library::load(&nix::papers_path(&root))
            .unwrap()
            .papers()
            .to_vec();

        assert_eq!(
            filter_papers(&papers, &ListFilter::default()).len(),
            2,
            "no filters should pass everything through"
        );
        assert_eq!(
            filter_papers(
                &papers,
                &ListFilter {
                    author: Some("hewitt".to_string()),
                    ..Default::default()
                }
            )
            .len(),
            1
        );
        assert_eq!(
            filter_papers(
                &papers,
                &ListFilter {
                    year: Some(1936),
                    ..Default::default()
                }
            )
            .len(),
            1
        );
        assert_eq!(
            filter_papers(
                &papers,
                &ListFilter {
                    tag: Some("concurrency".to_string()),
                    ..Default::default()
                }
            )
            .len(),
            1
        );
        assert_eq!(
            filter_papers(
                &papers,
                &ListFilter {
                    author: Some("hewitt".to_string()),
                    year: Some(1936),
                    ..Default::default()
                }
            )
            .len(),
            0,
            "combined criteria must all match (AND, not OR)"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}

/// The result of resolving a `show` reference: a paper already declared in
/// the local library, a paper excluded in `excluded.nix`, or an unresolved
/// candidate from a provider.
pub enum ShowResult {
    Declared(Paper),
    /// An excluded paper, named by its alias.
    Excluded(Exclusion),
    Candidate(CandidateWork),
}

/// Resolves a `show` reference. A `CandidateId` always contains `:`
/// (`provider:native_id`); a library name — a citation key (by construction
/// of `citation_key::generate`) or an alias ([`paper::is_valid_alias`]) —
/// never does, so the colon's presence decides which address space
/// `reference` belongs to before either lookup is attempted, rather than
/// trying one and falling back to the other. A library name is a declared
/// paper's citation key or alias, or an excluded paper's alias.
pub async fn show_reference(
    reference: &str,
    root: &Path,
    config: &Config,
) -> Result<ShowResult, PaxError> {
    if !reference.contains(':') {
        let Ok(library) = Library::load(&nix::papers_path(root)) else {
            return Err(PaxError::NotFound(reference.to_string()));
        };
        let exclusions = Exclusions::load(&nix::excluded_path(root))?;
        return match names::lookup(reference, &library, &exclusions) {
            Some(names::Named::Declared(paper)) => Ok(ShowResult::Declared(paper.clone())),
            Some(names::Named::Excluded(exclusion)) => Ok(ShowResult::Excluded(exclusion.clone())),
            None => Err(PaxError::NotFound(reference.to_string())),
        };
    }

    let id: CandidateId = reference.parse()?;
    let work = resolve_candidate(&id, config).await?;
    Ok(ShowResult::Candidate(work))
}

/// Excludes a paper: records it in `research/excluded.nix` with `reason` and
/// `stage`, replacing any earlier exclusion of the same paper.
///
/// `reference` is a DOI (bare or `https://doi.org/…`) or a provider
/// reference (`openalex:W…`). The entry's title comes from the provider —
/// a DOI is looked up across providers, a reference resolved directly — and
/// a provider reference whose work has a DOI is keyed by that DOI, so the
/// exclusion matches the paper from any provider ([`exclusion_key`]); an
/// arXiv id is keyed without its version, so every version is excluded.
/// `title`, when given, overrides the resolved title and is the fallback if
/// resolution fails; a bare DOI with a `title` needs no network at all.
///
/// Refuses a paper already declared in `papers.nix` — matched by DOI, or
/// for a work without one, by normalized title and year — since a paper
/// can't be both included and excluded.
///
/// The entry records the paper's publication year when the provider gives
/// one. To set the year yourself, or to also record a criterion and round,
/// use [`exclude_paper_with`].
pub async fn exclude_paper(
    reference: &str,
    reason: &str,
    stage: ExclusionStage,
    title: Option<&str>,
    root: &Path,
    config: &Config,
) -> Result<Exclusion, PaxError> {
    let options = ExclusionOptions::new(reason, stage).with_title(title.map(str::to_string));
    exclude_paper_with(reference, &options, root, config).await
}

/// What [`exclude_paper_with`] records about an exclusion: the required
/// `reason` and `stage`, and optionally a `title` (as in [`exclude_paper`]),
/// a publication `year`, the protocol's `criterion` (e.g. `X3`) and the
/// screening `round` (e.g. `R1`). Criterion and round are free text pax
/// doesn't validate; blank ones count as unset. The `alias` (e.g. `P18`) must
/// be a valid alias ([`paper::is_valid_alias`]) no other paper holds as an
/// alias or citation key. Unlike the other fields, an alias left `None`
/// keeps the one an earlier exclusion of the paper recorded, and a blank one
/// clears it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExclusionOptions {
    pub reason: String,
    pub stage: ExclusionStage,
    pub title: Option<String>,
    /// Overrides the resolved work's year, and supplies one for a reference
    /// that can't be resolved. With the title, it's how `stats` matches an
    /// exclusion keyed by an opaque `provider:id` to its logged records.
    pub year: Option<i32>,
    pub criterion: Option<String>,
    pub round: Option<String>,
    pub alias: Option<String>,
}

impl ExclusionOptions {
    /// Options with a reason and stage, and no title, year, criterion,
    /// round or alias.
    pub fn new(reason: impl Into<String>, stage: ExclusionStage) -> Self {
        ExclusionOptions {
            reason: reason.into(),
            stage,
            title: None,
            year: None,
            criterion: None,
            round: None,
            alias: None,
        }
    }

    pub fn with_title(mut self, title: Option<String>) -> Self {
        self.title = title;
        self
    }

    pub fn with_year(mut self, year: Option<i32>) -> Self {
        self.year = year;
        self
    }

    pub fn with_criterion(mut self, criterion: Option<String>) -> Self {
        self.criterion = criterion;
        self
    }

    pub fn with_round(mut self, round: Option<String>) -> Self {
        self.round = round;
        self
    }

    pub fn with_alias(mut self, alias: Option<String>) -> Self {
        self.alias = alias;
        self
    }
}

/// [`exclude_paper`], recording everything in `options`: a given `year`
/// replaces the resolved one, and the criterion, round and alias are
/// recorded. Re-excluding a paper replaces its whole entry, so a year neither
/// given nor resolved again, or a criterion or round not given again, is
/// cleared. The alias is the exception: one not given again is kept, so a
/// corrected reason never loses the paper's ID; a blank one clears it.
pub async fn exclude_paper_with(
    reference: &str,
    options: &ExclusionOptions,
    root: &Path,
    config: &Config,
) -> Result<Exclusion, PaxError> {
    let reason = options.reason.trim();
    if reason.is_empty() {
        return Err(PaxError::MissingExclusionReason);
    }
    let stage = options.stage;
    let set = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let title = options
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let alias = set(&options.alias);
    // Before resolving, so a malformed alias costs no network.
    if let Some(alias) = alias.as_deref()
        && !paper::is_valid_alias(alias)
    {
        return Err(PaxError::InvalidAlias(alias.to_string()));
    }
    let library = Library::load(&nix::papers_path(root))?;

    let reference = reference.trim();
    let mut resolved = if doi_exclusion_key(reference).starts_with("10.") {
        resolve_doi_for_exclusion(reference, title, config).await?
    } else {
        let id: CandidateId = reference
            .parse()
            .map_err(|_| PaxError::InvalidExclusionReference(reference.to_string()))?;
        match resolve_candidate(&id, config).await {
            Ok(work) => ResolvedExclusion {
                key: exclusion_key(&work),
                title: title
                    .map(str::to_string)
                    .or_else(|| (!work.title.is_empty()).then(|| work.title.clone()))
                    .ok_or_else(|| {
                        PaxError::ExclusionTitleUnresolved(
                            reference.to_string(),
                            "the provider returned no title".to_string(),
                        )
                    })?,
                doi: work.doi.clone(),
                year: year_from_publish_date(&work.publish_date),
            },
            Err(e) => match title {
                Some(title) => ResolvedExclusion {
                    key: candidate_exclusion_key(&id),
                    title: title.to_string(),
                    doi: None,
                    year: None,
                },
                None => {
                    return Err(PaxError::ExclusionTitleUnresolved(
                        reference.to_string(),
                        e.to_string(),
                    ));
                }
            },
        }
    };
    if options.year.is_some() {
        resolved.year = options.year;
    }

    if let Some(paper) = declared_match(&library, &resolved) {
        return Err(PaxError::ExcludingDeclaredPaper(
            reference.to_string(),
            paper.local.citation_key.clone(),
        ));
    }

    let path = nix::excluded_path(root);
    let mut exclusions = Exclusions::load(&path)?;
    // Not given: a re-exclusion keeps the alias the entry already has.
    let alias = match options.alias {
        None => exclusions.get(&resolved.key).and_then(|e| e.alias.clone()),
        Some(_) => alias,
    };
    if options.alias.is_some()
        && let Some(alias) = &alias
    {
        let owner = NameHolder::Excluded(resolved.key.clone());
        names::check_alias(alias, &library, &exclusions, Some(&owner))?;
    }
    let exclusion = Exclusion::new(resolved.key, resolved.title, reason.to_string(), stage)
        .with_year(resolved.year)
        .with_criterion(set(&options.criterion))
        .with_round(set(&options.round))
        .with_alias(alias);
    exclusions.upsert(exclusion.clone());
    exclusions.save(&path)?;
    Ok(exclusion)
}

/// Reverses an exclusion: removes the paper's entry from
/// `research/excluded.nix` and returns it, so [`add_candidate`] accepts the
/// paper again.
///
/// `reference` is an entry's exclusion key (what
/// [`PaxError::AlreadyExcluded`] names), its alias, a DOI, or a provider
/// reference. It matches an entry by the same keys
/// [`LibraryIndex::is_excluded`] does: a DOI, an arXiv id in any version, or
/// a `provider:id` key. A provider reference that matches no entry by itself
/// is resolved, so a paper excluded under its DOI is found from any
/// provider's reference to it; a key, an alias or a DOI needs no network.
/// Only one entry is removed: the exact
/// key if there is one, else the first match. Fails with
/// [`PaxError::NotExcluded`] when nothing matches. The returned entry
/// carries the alias it freed, if it had one.
pub async fn unexclude_paper(
    reference: &str,
    root: &Path,
    config: &Config,
) -> Result<Exclusion, PaxError> {
    let reference = reference.trim();
    let path = nix::excluded_path(root);
    let mut exclusions = Exclusions::load(&path)?;

    let key = match unexclude_match(&exclusions, reference, root, config).await? {
        Some(exclusion) => exclusion.key.clone(),
        None => return Err(PaxError::NotExcluded(reference.to_string())),
    };
    let exclusion = exclusions
        .remove(&key)
        .expect("the matched entry is in exclusions");
    exclusions.save(&path)?;
    Ok(exclusion)
}

/// The entry [`unexclude_paper`] removes for `reference`, if any.
async fn unexclude_match<'a>(
    exclusions: &'a Exclusions,
    reference: &str,
    root: &Path,
    config: &Config,
) -> Result<Option<&'a Exclusion>, PaxError> {
    if let Some(exclusion) = exclusions.get(reference) {
        return Ok(Some(exclusion));
    }
    let is_doi = doi_exclusion_key(reference).starts_with("10.");
    if !is_doi && paper::is_valid_alias(reference) {
        let library = Library::load(&nix::papers_path(root))?;
        return Ok(match names::lookup(reference, &library, exclusions) {
            Some(names::Named::Excluded(exclusion)) => Some(exclusion),
            _ => None,
        });
    }
    if is_doi {
        return Ok(library_index::find_exclusion(
            exclusions,
            &identity::doi_id_keys(reference),
            &[doi_exclusion_key(reference)],
        ));
    }
    let id: CandidateId = reference
        .parse()
        .map_err(|_| PaxError::InvalidExclusionReference(reference.to_string()))?;
    let found = library_index::find_exclusion(
        exclusions,
        &identity::reference_id_keys(&id),
        &[candidate_exclusion_key(&id)],
    );
    // A Crossref id is a DOI, already matched; resolving learns nothing more.
    if found.is_some() || id.provider == ProviderId::Crossref {
        return Ok(found);
    }
    let work = resolve_candidate(&id, config).await?;
    let (id_keys, keys) = library_index::work_exclusion_keys(&work);
    Ok(library_index::find_exclusion(exclusions, &id_keys, &keys))
}

/// What [`exclude_paper`] learned about the paper it's excluding.
struct ResolvedExclusion {
    key: String,
    title: String,
    doi: Option<String>,
    year: Option<i32>,
}

/// A DOI's exclusion: keyed by the DOI itself; the title from `title` if
/// given, otherwise from the first provider that resolves the DOI.
async fn resolve_doi_for_exclusion(
    doi: &str,
    title: Option<&str>,
    config: &Config,
) -> Result<ResolvedExclusion, PaxError> {
    let key = doi_exclusion_key(doi);
    if let Some(title) = title {
        return Ok(ResolvedExclusion {
            key: key.clone(),
            title: title.to_string(),
            doi: Some(key),
            year: None,
        });
    }

    let mut results = search_by_doi(&key, config).await;
    let mut errors = Vec::new();
    for provider in [
        ProviderId::OpenAlex,
        ProviderId::Crossref,
        ProviderId::SemanticScholar,
    ] {
        match results.remove(&provider) {
            Some(Ok(works)) => {
                if let Some(work) = works.into_iter().find(|w| !w.title.is_empty()) {
                    return Ok(ResolvedExclusion {
                        key: key.clone(),
                        title: work.title,
                        doi: Some(key),
                        year: year_from_publish_date(&work.publish_date),
                    });
                }
            }
            Some(Err(e)) => errors.push(format!("{provider}: {e}")),
            None => {}
        }
    }
    let detail = if errors.is_empty() {
        "no provider returned a title".to_string()
    } else {
        errors.join("; ")
    };
    Err(PaxError::ExclusionTitleUnresolved(doi.to_string(), detail))
}

/// The declared paper `resolved` refers to, if any: same DOI when both have
/// one, otherwise same normalized title and the same year where both are
/// known.
fn declared_match<'a>(library: &'a Library, resolved: &ResolvedExclusion) -> Option<&'a Paper> {
    library.papers().iter().find(|paper| {
        match (resolved.doi.as_deref(), paper.identity.doi.as_deref()) {
            (Some(excluded), Some(declared)) => {
                doi_exclusion_key(excluded) == doi_exclusion_key(declared)
            }
            _ => {
                let same_title =
                    normalized_title(&paper.identity.title) == normalized_title(&resolved.title);
                let same_year = match (resolved.year, paper.identity.year) {
                    (Some(a), Some(b)) => a == b,
                    _ => true,
                };
                same_title && same_year
            }
        }
    })
}

/// Lowercased alphanumeric words, single-spaced — so case, punctuation, and
/// spacing differences between providers don't hide a match.
fn normalized_title(title: &str) -> String {
    title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Removes a declared paper from the local library at `root`. Only the
/// declaration/metadata is removed — Nix remains responsible for garbage
/// collecting any unused artifacts on its own (see `docs/mvp.md` at tag `1.0.0`).
///
/// `name` is the paper's citation key or alias; an excluded paper's alias is
/// refused with [`PaxError::ExcludedNotDeclared`]. Removing a paper frees
/// its alias; [`remove_paper_entry`] returns the removed paper, so a caller
/// can say which.
pub fn remove_paper(name: &str, root: &Path) -> Result<(), PaxError> {
    remove_paper_entry(name, root).map(|_| ())
}

/// [`remove_paper`], returning the paper it removed.
pub fn remove_paper_entry(name: &str, root: &Path) -> Result<Paper, PaxError> {
    let path = nix::papers_path(root);
    let mut library = Library::load(&path)?;
    let citation_key = names::declared_key(name, &library, root)?;
    let paper = library
        .find(&citation_key)
        .cloned()
        .expect("declared_key names a declared paper");
    library.remove(&citation_key);
    library.save(&path)?;
    Ok(paper)
}

/// What to change about a declared paper, passed to [`edit_paper`]. Every
/// field is opt-in — only a field that's `Some`/non-empty gets touched,
/// matching §2.10 of `docs/mvp.md` at tag `1.0.0`: "the original external metadata should not be
/// silently overwritten without user intent." Non-exhaustive, so fields can
/// be added without breaking callers: build one with [`PaperEdits::new`] and
/// the `with_*` methods.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct PaperEdits {
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
    pub notes: Option<String>,
    pub rename: Option<String>,
    pub title: Option<String>,
    /// A full replacement of the author list when given — not incremental
    /// add/remove like tags, since an author-list correction means the list
    /// was wrong, not that one name needs adding.
    pub authors: Option<Vec<String>>,
    pub year: Option<i32>,
    pub doi: Option<String>,
    /// Sets/replaces `Artifact.source_url` — the recovery path for a paper a
    /// provider declared with no open-access copy (`NoSourceUrl`/"has no PDF
    /// source recorded"): the user can find one by hand and record it here.
    /// Changing it invalidates any already-fetched `hash` (see
    /// `paper::apply_artifact_edits`), so the next `fetch`/`open`
    /// re-materializes against the new URL instead of keeping stale bytes.
    pub source_url: Option<String>,
    /// Sets the paper's alias (e.g. `P17`); an empty string clears it. It must
    /// be a valid alias ([`paper::is_valid_alias`]) that no other paper holds
    /// as an alias or citation key, in `papers.nix` or `excluded.nix`.
    pub alias: Option<String>,
}

impl PaperEdits {
    /// No edits; set the ones to make with the `with_*` methods.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_add_tags(mut self, add_tags: Vec<String>) -> Self {
        self.add_tags = add_tags;
        self
    }

    pub fn with_remove_tags(mut self, remove_tags: Vec<String>) -> Self {
        self.remove_tags = remove_tags;
        self
    }

    pub fn with_notes(mut self, notes: Option<String>) -> Self {
        self.notes = notes;
        self
    }

    pub fn with_rename(mut self, rename: Option<String>) -> Self {
        self.rename = rename;
        self
    }

    pub fn with_title(mut self, title: Option<String>) -> Self {
        self.title = title;
        self
    }

    pub fn with_authors(mut self, authors: Option<Vec<String>>) -> Self {
        self.authors = authors;
        self
    }

    pub fn with_year(mut self, year: Option<i32>) -> Self {
        self.year = year;
        self
    }

    pub fn with_doi(mut self, doi: Option<String>) -> Self {
        self.doi = doi;
        self
    }

    pub fn with_source_url(mut self, source_url: Option<String>) -> Self {
        self.source_url = source_url;
        self
    }

    pub fn with_alias(mut self, alias: Option<String>) -> Self {
        self.alias = alias;
        self
    }

    fn is_empty(&self) -> bool {
        self.add_tags.is_empty()
            && self.remove_tags.is_empty()
            && self.notes.is_none()
            && self.rename.is_none()
            && self.title.is_none()
            && self.authors.is_none()
            && self.year.is_none()
            && self.doi.is_none()
            && self.source_url.is_none()
            && self.alias.is_none()
    }
}

/// Edits a declared paper: tags, notes, citation-key rename, and Identity
/// corrections (title/authors/year/doi). A rename is checked for validity
/// (`library::is_valid_citation_key` — citation keys are unquoted Nix
/// identifiers, so an invalid one would corrupt `papers.nix`) and for
/// collision against every other declared paper before anything is written:
/// a citation key another paper holds is [`PaxError::CitationKeyExists`],
/// an alias another paper holds (declared or excluded) is
/// [`PaxError::NameTaken`]. A rename keeps the paper's alias. A new alias is
/// checked the same way ([`PaxError::InvalidAlias`], [`PaxError::NameTaken`]).
///
/// `name` is the paper's citation key or alias; an excluded paper's alias is
/// refused with [`PaxError::ExcludedNotDeclared`].
pub fn edit_paper(name: &str, root: &Path, edits: &PaperEdits) -> Result<(), PaxError> {
    if edits.is_empty() {
        return Err(PaxError::NoChangesSpecified);
    }
    if let Some(new_key) = &edits.rename
        && !library::is_valid_citation_key(new_key)
    {
        return Err(PaxError::InvalidCitationKey(new_key.clone()));
    }

    let path = nix::papers_path(root);
    let mut library = Library::load(&path)?;
    let citation_key = names::declared_key(name, &library, root)?;

    if let Some(new_key) = &edits.rename
        && *new_key != citation_key
        && library.find(new_key).is_some()
    {
        return Err(PaxError::CitationKeyExists(new_key.clone()));
    }

    let exclusions = Exclusions::load(&nix::excluded_path(root))?;
    let owner = NameHolder::Declared(citation_key.clone());
    if let Some(new_key) = &edits.rename
        && let Some(holder) = names::holder(new_key, &library, &exclusions, Some(&owner))
    {
        return Err(PaxError::NameTaken {
            name: new_key.clone(),
            holder,
        });
    }
    if let Some(alias) = edits.alias.as_deref().filter(|a| !a.is_empty()) {
        names::check_alias(alias, &library, &exclusions, Some(&owner))?;
    }

    let paper = library
        .find_mut(&citation_key)
        .expect("declared_key names a declared paper");
    paper::apply_local_edits(
        &mut paper.local,
        &edits.add_tags,
        &edits.remove_tags,
        edits.notes.as_deref(),
        edits.rename.as_deref(),
    );
    paper::apply_identity_corrections(
        &mut paper.identity,
        edits.title.as_deref(),
        edits.authors.as_deref(),
        edits.year,
        edits.doi.as_deref(),
    );
    paper::apply_artifact_edits(&mut paper.artifact, edits.source_url.as_deref());
    if let Some(alias) = &edits.alias {
        paper.local.alias = (!alias.is_empty()).then(|| alias.clone());
    }
    library.save(&path)?;
    Ok(())
}

#[cfg(test)]
mod edit_paper_tests {
    use super::*;
    use std::fs;

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pax-edit-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("research")).unwrap();
        dir
    }

    fn two_papers_fixture() -> &'static str {
        r#"{
  turing1936 = {
    doi = null;
    title = "On Computable Numbers";
    authors = [ "Alan Turing" ];
    year = 1936;
    venue = null;
    source_url = null;
    hash = null;
    tags = [ ];
    notes = null;
  };
  hewitt1973 = {
    doi = null;
    title = "A Universal Modular Actor Formalism";
    authors = [ "Carl Hewitt" ];
    year = 1973;
    venue = null;
    source_url = null;
    hash = null;
    tags = [ ];
    notes = null;
  };
}
"#
    }

    #[test]
    fn rename_to_an_existing_key_is_rejected() {
        let root = scratch_dir("rename-collision");
        fs::write(nix::papers_path(&root), two_papers_fixture()).unwrap();

        let result = edit_paper(
            "turing1936",
            &root,
            &PaperEdits {
                rename: Some("hewitt1973".to_string()),
                ..Default::default()
            },
        );
        assert!(matches!(result, Err(PaxError::CitationKeyExists(_))));

        // Untouched: still parses, and both original keys still resolve.
        let library = Library::load(&nix::papers_path(&root)).unwrap();
        assert!(library.find("turing1936").is_some());
        assert!(library.find("hewitt1973").is_some());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rename_to_an_invalid_key_is_rejected_without_touching_the_file() {
        let root = scratch_dir("rename-invalid");
        fs::write(nix::papers_path(&root), two_papers_fixture()).unwrap();

        let result = edit_paper(
            "turing1936",
            &root,
            &PaperEdits {
                rename: Some("bad key".to_string()),
                ..Default::default()
            },
        );
        assert!(matches!(result, Err(PaxError::InvalidCitationKey(_))));
        assert!(
            Library::load(&nix::papers_path(&root))
                .unwrap()
                .find("turing1936")
                .is_some()
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rename_succeeds_and_persists() {
        let root = scratch_dir("rename-success");
        fs::write(nix::papers_path(&root), two_papers_fixture()).unwrap();

        edit_paper(
            "turing1936",
            &root,
            &PaperEdits {
                rename: Some("turing36".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

        let library = Library::load(&nix::papers_path(&root)).unwrap();
        assert!(library.find("turing1936").is_none());
        assert!(library.find("turing36").is_some());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn no_edits_specified_is_an_error() {
        let root = scratch_dir("no-edits");
        fs::write(nix::papers_path(&root), two_papers_fixture()).unwrap();
        let result = edit_paper("turing1936", &root, &PaperEdits::default());
        assert!(matches!(result, Err(PaxError::NoChangesSpecified)));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn setting_a_source_url_persists_it_and_clears_any_existing_hash() {
        let root = scratch_dir("source-url");
        fs::write(
            nix::papers_path(&root),
            r#"{
  turing1936 = {
    doi = null;
    title = "On Computable Numbers";
    authors = [ "Alan Turing" ];
    year = 1936;
    venue = null;
    source_url = null;
    hash = "sha256-stale";
    tags = [ ];
    notes = null;
  };
}
"#,
        )
        .unwrap();

        edit_paper(
            "turing1936",
            &root,
            &PaperEdits {
                source_url: Some("https://example.org/turing.pdf".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

        let library = Library::load(&nix::papers_path(&root)).unwrap();
        let paper = library.find("turing1936").unwrap();
        assert_eq!(
            paper.artifact.source_url.as_deref(),
            Some("https://example.org/turing.pdf")
        );
        assert!(
            paper.artifact.hash.is_none(),
            "a changed source invalidates the old hash"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}

#[cfg(test)]
mod alias_resolution_tests {
    use super::*;
    use std::fs;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    /// Polls a future that never waits: a library name needs no network.
    fn block_on<F: Future>(future: F) -> F::Output {
        match pin!(future)
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("unexpectedly waited on the network"),
        }
    }

    /// `turing1936` declared as P17, `hewitt1973` declared without an alias,
    /// `10.1/abc` excluded as P18.
    fn library(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("pax-alias-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("research")).unwrap();
        fs::write(
            nix::papers_path(&root),
            r#"{
  turing1936 = {
    title = "On Computable Numbers";
    authors = [ "Alan Turing" ];
    alias = "P17";
  };
  hewitt1973 = { title = "A Universal Modular Actor Formalism"; authors = [ ]; };
}
"#,
        )
        .unwrap();
        fs::write(
            nix::excluded_path(&root),
            r#"{
  "10.1/abc" = { title = "Excluded"; reason = "not empirical"; stage = "screening"; alias = "P18"; };
}
"#,
        )
        .unwrap();
        root
    }

    fn papers(root: &Path) -> Library {
        Library::load(&nix::papers_path(root)).unwrap()
    }

    #[test]
    fn edit_accepts_an_alias() {
        let root = library("edit");
        let edits = PaperEdits {
            notes: Some("via alias".to_string()),
            ..Default::default()
        };
        edit_paper("P17", &root, &edits).unwrap();
        let library = papers(&root);
        let paper = library.find("turing1936").unwrap();
        assert_eq!(paper.local.notes.as_deref(), Some("via alias"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_rename_keeps_the_alias() {
        let root = library("rename");
        let edits = PaperEdits {
            rename: Some("turing36".to_string()),
            ..Default::default()
        };
        edit_paper("P17", &root, &edits).unwrap();
        let library = papers(&root);
        assert_eq!(
            library.find_alias("P17").unwrap().local.citation_key,
            "turing36"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn remove_accepts_an_alias_and_returns_the_paper_it_freed() {
        let root = library("remove");
        let removed = remove_paper_entry("P17", &root).unwrap();
        assert_eq!(removed.local.citation_key, "turing1936");
        assert_eq!(removed.local.alias.as_deref(), Some("P17"));
        assert!(papers(&root).find("turing1936").is_none());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn open_accepts_an_alias() {
        let root = library("open");
        assert!(matches!(
            resolve_artifact_path("P17", &root),
            Ok(ResolvedArtifact::NoSourceUrl)
        ));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn commands_that_need_a_declared_paper_refuse_an_excluded_alias() {
        let root = library("refuse");
        let excluded = |result: Result<(), PaxError>| {
            matches!(
                result,
                Err(PaxError::ExcludedNotDeclared { name, key }) if name == "P18" && key == "10.1/abc"
            )
        };
        let edits = PaperEdits {
            notes: Some("x".to_string()),
            ..Default::default()
        };
        assert!(excluded(edit_paper("P18", &root, &edits)));
        assert!(excluded(remove_paper("P18", &root)));
        assert!(excluded(fetch_paper("P18", &root).map(|_| ())));
        assert!(excluded(resolve_artifact_path("P18", &root).map(|_| ())));
        let error = remove_paper("P18", &root).unwrap_err();
        assert_eq!(
            error.to_string(),
            "P18 is excluded, not in the library (excluded as 10.1/abc)"
        );
        assert!(matches!(
            remove_paper("P19", &root),
            Err(PaxError::NoSuchPaper(_))
        ));
        assert_eq!(papers(&root).papers().len(), 2);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn show_resolves_a_declared_and_an_excluded_alias() {
        let root = library("show");
        let config = Config::default();
        assert!(matches!(
            block_on(show_reference("P17", &root, &config)),
            Ok(ShowResult::Declared(paper)) if paper.local.citation_key == "turing1936"
        ));
        assert!(matches!(
            block_on(show_reference("P18", &root, &config)),
            Ok(ShowResult::Excluded(exclusion)) if exclusion.key == "10.1/abc"
        ));
        assert!(matches!(
            block_on(show_reference("P19", &root, &config)),
            Err(PaxError::NotFound(_))
        ));
        fs::remove_dir_all(&root).ok();
    }

    fn set_alias(root: &Path, name: &str, alias: &str) -> Result<(), PaxError> {
        let edits = PaperEdits {
            alias: Some(alias.to_string()),
            ..Default::default()
        };
        edit_paper(name, root, &edits)
    }

    fn rename(root: &Path, name: &str, new_key: &str) -> Result<(), PaxError> {
        let edits = PaperEdits {
            rename: Some(new_key.to_string()),
            ..Default::default()
        };
        edit_paper(name, root, &edits)
    }

    fn taken_by(result: Result<(), PaxError>) -> Option<NameHolder> {
        match result {
            Err(PaxError::NameTaken { holder, .. }) => Some(holder),
            _ => None,
        }
    }

    #[test]
    fn edit_sets_and_an_empty_alias_clears_it() {
        let root = library("set-clear");
        set_alias(&root, "hewitt1973", "P20").unwrap();
        assert_eq!(
            papers(&root).find_alias("P20").unwrap().local.citation_key,
            "hewitt1973"
        );
        set_alias(&root, "P20", "").unwrap();
        assert_eq!(papers(&root).find("hewitt1973").unwrap().local.alias, None);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_alias_held_by_another_paper_in_either_file_is_refused() {
        let root = library("alias-taken");
        let declared = |key: &str| Some(NameHolder::Declared(key.to_string()));
        assert_eq!(
            taken_by(set_alias(&root, "hewitt1973", "P17")),
            declared("turing1936")
        );
        assert_eq!(
            taken_by(set_alias(&root, "hewitt1973", "turing1936")),
            declared("turing1936")
        );
        assert_eq!(
            taken_by(set_alias(&root, "hewitt1973", "P18")),
            Some(NameHolder::Excluded("10.1/abc".to_string()))
        );
        assert!(matches!(
            set_alias(&root, "hewitt1973", "P 20"),
            Err(PaxError::InvalidAlias(_))
        ));
        // Setting a paper's own alias again is fine.
        set_alias(&root, "turing1936", "P17").unwrap();
        assert_eq!(papers(&root).find("hewitt1973").unwrap().local.alias, None);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_rename_to_an_alias_another_paper_holds_is_refused() {
        let root = library("rename-taken");
        assert_eq!(
            taken_by(rename(&root, "hewitt1973", "P17")),
            Some(NameHolder::Declared("turing1936".to_string()))
        );
        assert_eq!(
            taken_by(rename(&root, "hewitt1973", "P18")),
            Some(NameHolder::Excluded("10.1/abc".to_string()))
        );
        assert!(papers(&root).find("hewitt1973").is_some());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_removed_papers_alias_can_be_taken_again() {
        let root = library("retake");
        remove_paper("P17", &root).unwrap();
        set_alias(&root, "hewitt1973", "P17").unwrap();
        assert_eq!(
            papers(&root).find_alias("P17").unwrap().local.citation_key,
            "hewitt1973"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// Excludes `doi` under `alias`, offline: a DOI with a title.
    fn exclude_as(root: &Path, doi: &str, alias: &str) -> Result<Exclusion, PaxError> {
        let options = ExclusionOptions::new("off topic", ExclusionStage::FullText)
            .with_title(Some("A title".to_string()))
            .with_alias(Some(alias.to_string()));
        block_on(exclude_paper_with(doi, &options, root, &Config::default()))
    }

    fn exclusion_alias(root: &Path, key: &str) -> Option<String> {
        Exclusions::load(&nix::excluded_path(root))
            .unwrap()
            .get(key)?
            .alias
            .clone()
    }

    #[test]
    fn exclude_sets_an_alias_and_re_excluding_changes_it() {
        let root = library("exclude-alias");
        let exclusion = exclude_as(&root, "10.1/new", "P20").unwrap();
        assert_eq!(exclusion.alias.as_deref(), Some("P20"));
        assert_eq!(exclusion_alias(&root, "10.1/new").as_deref(), Some("P20"));

        // The paper's own alias isn't taken from itself.
        exclude_as(&root, "10.1/abc", "P18").unwrap();
        exclude_as(&root, "10.1/abc", "P21").unwrap();
        assert_eq!(exclusion_alias(&root, "10.1/abc").as_deref(), Some("P21"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn re_excluding_without_an_alias_keeps_it_and_a_blank_one_clears_it() {
        let root = library("exclude-keep-alias");
        exclude_as(&root, "10.1/new", "P20").unwrap();
        let options = ExclusionOptions::new("not empirical", ExclusionStage::Screening)
            .with_title(Some("A title".to_string()));
        let exclusion = block_on(exclude_paper_with(
            "10.1/new",
            &options,
            &root,
            &Config::default(),
        ))
        .unwrap();
        assert_eq!(exclusion.reason, "not empirical");
        assert_eq!(exclusion.alias.as_deref(), Some("P20"));
        assert_eq!(exclusion_alias(&root, "10.1/new").as_deref(), Some("P20"));

        exclude_as(&root, "10.1/new", "").unwrap();
        assert_eq!(exclusion_alias(&root, "10.1/new"), None);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn exclude_refuses_an_invalid_alias_or_one_another_paper_holds() {
        let root = library("exclude-taken");
        assert!(matches!(
            exclude_as(&root, "10.1/new", "P:20"),
            Err(PaxError::InvalidAlias(_))
        ));
        let taken = |alias| match exclude_as(&root, "10.1/new", alias) {
            Err(PaxError::NameTaken { holder, .. }) => Some(holder),
            _ => None,
        };
        assert_eq!(
            taken("P17"),
            Some(NameHolder::Declared("turing1936".to_string()))
        );
        assert_eq!(
            taken("hewitt1973"),
            Some(NameHolder::Declared("hewitt1973".to_string()))
        );
        assert_eq!(
            taken("P18"),
            Some(NameHolder::Excluded("10.1/abc".to_string()))
        );
        assert_eq!(exclusion_alias(&root, "10.1/new"), None);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_freed_alias_can_be_taken_on_the_other_side() {
        let root = library("move");
        // Declared to excluded: remove, then exclude under the same alias.
        remove_paper("P17", &root).unwrap();
        exclude_as(&root, "10.1/turing", "P17").unwrap();
        // Excluded to declared: unexclude, then give a declared paper the alias.
        block_on(unexclude_paper("P18", &root, &Config::default())).unwrap();
        set_alias(&root, "hewitt1973", "P18").unwrap();
        assert!(matches!(
            block_on(show_reference("P17", &root, &Config::default())),
            Ok(ShowResult::Excluded(exclusion)) if exclusion.key == "10.1/turing"
        ));
        assert!(matches!(
            block_on(show_reference("P18", &root, &Config::default())),
            Ok(ShowResult::Declared(paper)) if paper.local.citation_key == "hewitt1973"
        ));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unexclude_accepts_an_alias_and_returns_the_entry_it_freed() {
        let root = library("unexclude");
        let config = Config::default();
        let removed = block_on(unexclude_paper("P18", &root, &config)).unwrap();
        assert_eq!(removed.key, "10.1/abc");
        assert_eq!(removed.alias.as_deref(), Some("P18"));
        assert!(
            Exclusions::load(&nix::excluded_path(&root))
                .unwrap()
                .entries()
                .is_empty()
        );
        // A declared paper's alias isn't an exclusion.
        assert!(matches!(
            block_on(unexclude_paper("P17", &root, &config)),
            Err(PaxError::NotExcluded(_))
        ));
        fs::remove_dir_all(&root).ok();
    }
}

/// The result of `fetch_paper`: whether an artifact was newly materialized or
/// was already fetched (and so didn't need a network call).
pub enum FetchOutcome {
    AlreadyFetched { hash: String },
    Fetched { hash: String },
}

/// Materializes a declared paper's artifact through Nix and records its
/// content hash. Idempotent: a paper that already has a hash is reported as
/// already fetched rather than re-hitting the network — verifying a *stale*
/// hash is `check`'s job, not `fetch`'s (see §2.6 of `docs/mvp.md` at tag `1.0.0`).
///
/// `name` is the paper's citation key or alias; an excluded paper's alias is
/// refused with [`PaxError::ExcludedNotDeclared`].
pub fn fetch_paper(name: &str, root: &Path) -> Result<FetchOutcome, PaxError> {
    let path = nix::papers_path(root);
    let mut library = Library::load(&path)?;
    let citation_key = names::declared_key(name, &library, root)?;
    let citation_key = citation_key.as_str();
    let paper = library
        .find_mut(citation_key)
        .expect("declared_key names a declared paper");

    if let Some(hash) = &paper.artifact.hash {
        return Ok(FetchOutcome::AlreadyFetched { hash: hash.clone() });
    }
    let source_url = paper
        .artifact
        .source_url
        .clone()
        .ok_or_else(|| PaxError::NoSourceUrl(citation_key.to_string()))?;

    let hash = nix::prefetch_file(&source_url, citation_key)?;
    paper.artifact.hash = Some(hash.clone());
    library.save(&path)?;
    Ok(FetchOutcome::Fetched { hash })
}

pub struct SyncReport {
    pub citation_key: String,
    pub result: Result<FetchOutcome, PaxError>,
}

/// Materializes every declared paper that doesn't have a hash yet, by
/// calling `fetch_paper` once per citation key — batch `fetch`, not a
/// distinct fetching mechanism. One paper's failure (network error, no
/// `source_url`) doesn't stop the rest, mirroring `check_library`'s
/// per-paper error isolation.
pub fn sync_library(root: &Path) -> Result<Vec<SyncReport>, PaxError> {
    let path = nix::papers_path(root);
    let library = Library::load(&path)?;
    let keys: Vec<String> = library
        .papers()
        .iter()
        .map(|p| p.local.citation_key.clone())
        .collect();
    Ok(keys
        .into_iter()
        .map(|citation_key| {
            let result = fetch_paper(&citation_key, root);
            SyncReport {
                citation_key,
                result,
            }
        })
        .collect())
}

#[cfg(test)]
mod sync_tests {
    use super::*;
    use std::fs;

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pax-sync-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("research")).unwrap();
        dir
    }

    #[test]
    fn empty_library_syncs_to_an_empty_report() {
        let root = scratch_dir("empty");
        fs::write(nix::papers_path(&root), "{\n}\n").unwrap();

        let reports = sync_library(&root).unwrap();
        assert!(reports.is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn already_fetched_papers_are_reported_without_touching_nix() {
        let root = scratch_dir("already-fetched");
        fs::write(
            nix::papers_path(&root),
            r#"{
  turing1936 = {
    doi = null;
    title = "On Computable Numbers";
    authors = [ "Alan Turing" ];
    year = 1936;
    source_url = "https://example.org/turing.pdf";
    hash = "sha256-abc123";
    tags = [ ];
    notes = null;
  };
}
"#,
        )
        .unwrap();

        let reports = sync_library(&root).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].citation_key, "turing1936");
        match &reports[0].result {
            Ok(FetchOutcome::AlreadyFetched { hash }) => assert_eq!(hash, "sha256-abc123"),
            _ => panic!("expected AlreadyFetched, got a different result"),
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn paper_with_no_source_url_reports_an_error_without_stopping_others() {
        let root = scratch_dir("no-source");
        fs::write(
            nix::papers_path(&root),
            r#"{
  noSource = {
    doi = null;
    title = "No Source Paper";
    authors = [ ];
    year = null;
    source_url = null;
    hash = null;
    tags = [ ];
    notes = null;
  };
  alreadyFetched = {
    doi = null;
    title = "Already Fetched Paper";
    authors = [ ];
    year = null;
    source_url = "https://example.org/paper.pdf";
    hash = "sha256-def456";
    tags = [ ];
    notes = null;
  };
}
"#,
        )
        .unwrap();

        let reports = sync_library(&root).unwrap();
        assert_eq!(reports.len(), 2);
        let no_source = reports
            .iter()
            .find(|r| r.citation_key == "noSource")
            .unwrap();
        assert!(matches!(no_source.result, Err(PaxError::NoSourceUrl(_))));
        let already_fetched = reports
            .iter()
            .find(|r| r.citation_key == "alreadyFetched")
            .unwrap();
        assert!(matches!(
            already_fetched.result,
            Ok(FetchOutcome::AlreadyFetched { .. })
        ));
        fs::remove_dir_all(&root).unwrap();
    }
}

/// The outcome of checking a single declared paper against `nix`.
pub enum CheckStatus {
    /// No hash recorded yet — `fetch` hasn't run for this paper.
    NotFetched,
    /// The recorded hash still matches what the source URL resolves to.
    Reproducible,
    /// The source URL now resolves to different content than what was recorded.
    Mismatch { expected: String, actual: String },
    /// The source URL couldn't be resolved at all (network failure, 404, etc.).
    Error(String),
}

pub struct CheckReport {
    pub citation_key: String,
    pub status: CheckStatus,
}

fn classify(expected: &str, fetched: Result<String, PaxError>) -> CheckStatus {
    match fetched {
        Ok(actual) if actual == expected => CheckStatus::Reproducible,
        Ok(actual) => CheckStatus::Mismatch {
            expected: expected.to_string(),
            actual,
        },
        Err(e) => CheckStatus::Error(e.to_string()),
    }
}

/// Verifies every declared paper's artifact still reproduces from its
/// recorded hash, without materializing or writing anything — that's
/// `fetch`'s job. A paper with no hash yet is reported `NotFetched` rather
/// than as an error; one paper's network failure doesn't stop the rest of
/// the scan (mirrors `search_all`'s per-provider error isolation).
pub fn check_library(root: &Path) -> Result<Vec<CheckReport>, PaxError> {
    let path = nix::papers_path(root);
    let library = Library::load(&path)?;
    Ok(library
        .papers()
        .iter()
        .map(|paper| {
            let status = match (&paper.artifact.hash, &paper.artifact.source_url) {
                (Some(expected), Some(url)) => {
                    classify(expected, nix::prefetch_file(url, &paper.local.citation_key))
                }
                _ => CheckStatus::NotFetched,
            };
            CheckReport {
                citation_key: paper.local.citation_key.clone(),
                status,
            }
        })
        .collect())
}

#[cfg(test)]
mod check_tests {
    use super::*;

    #[test]
    fn classify_matching_hash_is_reproducible() {
        let status = classify("sha256-abc", Ok("sha256-abc".to_string()));
        assert!(matches!(status, CheckStatus::Reproducible));
    }

    #[test]
    fn classify_differing_hash_is_a_mismatch() {
        let status = classify("sha256-abc", Ok("sha256-xyz".to_string()));
        match status {
            CheckStatus::Mismatch { expected, actual } => {
                assert_eq!(expected, "sha256-abc");
                assert_eq!(actual, "sha256-xyz");
            }
            _ => panic!("expected Mismatch"),
        }
    }

    #[test]
    fn classify_fetch_failure_is_an_error() {
        let status = classify(
            "sha256-abc",
            Err(PaxError::Fetch("connection refused".to_string())),
        );
        match status {
            CheckStatus::Error(message) => assert_eq!(message, "fetch failed: connection refused"),
            _ => panic!("expected Error"),
        }
    }
}

/// Where a declared paper's artifact stands relative to being openable.
pub enum ResolvedArtifact {
    /// Materialized; here's its Nix store path.
    Path(PathBuf),
    /// Has a `source_url` but no `hash` yet — recoverable by fetching.
    NotFetched,
    /// No `source_url` at all — nothing to fetch, unrecoverable automatically.
    NoSourceUrl,
}

/// Resolves a declared paper's materialized artifact to its Nix store path.
/// A paper with no hash yet is reported as `NotFetched` rather than
/// attempting a Nix build (which would fail on the `null` hash with an
/// opaque evaluation error); one with no `source_url` at all is
/// `NoSourceUrl`, since there's nothing to fetch even automatically.
///
/// `name` is the paper's citation key or alias; an excluded paper's alias is
/// refused with [`PaxError::ExcludedNotDeclared`].
pub fn resolve_artifact_path(name: &str, root: &Path) -> Result<ResolvedArtifact, PaxError> {
    let path = nix::papers_path(root);
    let library = Library::load(&path)?;
    let citation_key = names::declared_key(name, &library, root)?;
    let citation_key = citation_key.as_str();
    let paper = library
        .find(citation_key)
        .expect("declared_key names a declared paper");

    if paper.artifact.hash.is_none() {
        return Ok(if paper.artifact.source_url.is_some() {
            ResolvedArtifact::NotFetched
        } else {
            ResolvedArtifact::NoSourceUrl
        });
    }
    Ok(ResolvedArtifact::Path(nix::build_package(
        root,
        citation_key,
    )?))
}

#[cfg(test)]
mod resolve_artifact_path_tests {
    use super::*;
    use std::fs;

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pax-open-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("research")).unwrap();
        dir
    }

    #[test]
    fn no_hash_but_has_source_url_is_not_fetched() {
        let root = scratch_dir("not-fetched");
        fs::write(
            nix::papers_path(&root),
            r#"{
  turing1936 = {
    doi = null;
    title = "On Computable Numbers";
    authors = [ "Alan Turing" ];
    year = 1936;
    source_url = "https://example.org/turing.pdf";
    hash = null;
    tags = [ ];
    notes = null;
  };
}
"#,
        )
        .unwrap();

        let result = resolve_artifact_path("turing1936", &root).unwrap();
        assert!(matches!(result, ResolvedArtifact::NotFetched));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn no_hash_and_no_source_url_is_no_source_url() {
        let root = scratch_dir("no-source");
        fs::write(
            nix::papers_path(&root),
            r#"{
  turing1936 = {
    doi = null;
    title = "On Computable Numbers";
    authors = [ "Alan Turing" ];
    year = 1936;
    source_url = null;
    hash = null;
    tags = [ ];
    notes = null;
  };
}
"#,
        )
        .unwrap();

        let result = resolve_artifact_path("turing1936", &root).unwrap();
        assert!(matches!(result, ResolvedArtifact::NoSourceUrl));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unknown_citation_key_is_an_error() {
        let root = scratch_dir("unknown-key");
        fs::write(nix::papers_path(&root), "{\n}\n").unwrap();

        let result = resolve_artifact_path("nonexistent", &root);
        assert!(matches!(result, Err(PaxError::NoSuchPaper(_))));
        fs::remove_dir_all(&root).unwrap();
    }
}

#[cfg(test)]
mod send_tests {
    use super::*;

    fn assert_send<T: Send>(_: T) {}

    /// Async clients on a multi-threaded runtime (an MCP server, a
    /// `tokio::spawn`ed task) need every public future to be `Send`. These
    /// futures are only built, never polled, so nothing touches the network.
    #[test]
    fn public_futures_are_send() {
        let config = Config::default();
        let options = SearchOptions::default();
        let root = Path::new(".");
        let id: CandidateId = "openalex:W1".parse().unwrap();
        assert_send(search_all("q", &config));
        assert_send(search_by_author("a", &config));
        assert_send(search_all_paged("q", &options, &config));
        assert_send(search_by_author_paged("a", &options, &config));
        assert_send(search_by_doi("10.1/a", &config));
        assert_send(resolve_candidate(&id, &config));
        assert_send(add_candidate(&id, root, &config));
        assert_send(show_reference("k", root, &config));
        assert_send(exclude_paper(
            "openalex:W1",
            "r",
            ExclusionStage::Screening,
            None,
            root,
            &config,
        ));
        assert_send(exclude_paper_with(
            "openalex:W1",
            &ExclusionOptions::new("r", ExclusionStage::Screening),
            root,
            &config,
        ));
        assert_send(unexclude_paper("openalex:W1", root, &config));
        assert_send(references("k", &options, root, &config));
        assert_send(cited_by("k", &options, root, &config));
        assert_send(search_all_logged("q", &options, root, &config));
        assert_send(search_by_author_logged("a", &options, root, &config));
        assert_send(references_logged("k", &options, root, &config));
        assert_send(cited_by_logged("k", &options, root, &config));
    }
}
