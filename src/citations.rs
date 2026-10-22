//! Citation graph lookups: a seed paper's references (what it cites) and the
//! papers citing it, from the providers that expose real citation edges —
//! OpenAlex and Semantic Scholar. Results stay per provider: each list is
//! that provider's own, with its own total.

use std::collections::HashMap;
use std::path::Path;

use thiserror::Error;

use crate::error::PaxError;
use crate::library::Library;
use crate::paper::Paper;
use crate::provider::{
    CandidateId, OpenAlexProvider, ProviderError, ProviderId, SearchOptions, SearchPage,
    SemanticScholarProvider,
};
use crate::{Config, nix, normalize_doi, resolve_candidate};

/// Why one provider's citation list couldn't be fetched.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CitationError {
    /// The seed had no identifier this provider can look a paper up by.
    /// `lookup_error` says why, when fetching the seed from its own provider
    /// to learn its DOI failed.
    #[error(
        "can't look the seed up in {provider}: no {provider} id, DOI, or arXiv id is known{}",
        lookup_error.as_ref().map(|e| format!(" (fetching the seed's DOI failed: {e})")).unwrap_or_default()
    )]
    Unresolvable {
        provider: ProviderId,
        lookup_error: Option<SeedLookupError>,
    },
    /// The provider has no record of the seed under `key`.
    #[error("{provider} has no record of the seed (looked up as {key})")]
    SeedNotFound { provider: ProviderId, key: String },
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

/// Fetching a `provider:id` seed from its own provider, to learn its DOI,
/// failed. `error` tells a provider with no such work
/// ([`ProviderError::NotFound`]) from a request that failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{provider}: {error}")]
pub struct SeedLookupError {
    /// The seed's own provider, the one it was fetched from.
    pub provider: ProviderId,
    pub error: ProviderError,
}

/// Each citation provider's list, by provider. A provider's own failure —
/// including not knowing the seed — doesn't fail the others.
pub type CitationResults = HashMap<ProviderId, Result<SearchPage, CitationError>>;

/// The providers [`references`] and [`cited_by`] ask, in display order.
/// Crossref isn't one: its reference lists are publisher-deposited and often
/// incomplete, and it has no forward citations.
pub const CITATION_PROVIDERS: [ProviderId; 2] = [ProviderId::OpenAlex, ProviderId::SemanticScholar];

/// The works `seed` cites, per provider, paged up to `options.limit` with
/// `options.filters` applied. `seed` is a `provider:id` reference (any
/// provider) or the citation key of a paper in the library at `root`; it's
/// resolved to each provider's own id, via its DOI or arXiv id when needed.
///
/// Errors only when `seed` itself is invalid: a malformed reference, or a
/// citation key not in the library.
pub async fn references(
    seed: &str,
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> Result<CitationResults, PaxError> {
    citation_lists(seed, Direction::References, options, root, config).await
}

/// The works citing `seed`, per provider; otherwise as [`references`].
pub async fn cited_by(
    seed: &str,
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> Result<CitationResults, PaxError> {
    citation_lists(seed, Direction::CitedBy, options, root, config).await
}

#[derive(Clone, Copy)]
enum Direction {
    References,
    CitedBy,
}

async fn citation_lists(
    seed: &str,
    direction: Direction,
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> Result<CitationResults, PaxError> {
    let ids = SeedIds::resolve(seed, root, config).await?;
    let mut results = HashMap::new();

    let openalex = match ids.openalex_key() {
        None => Err(CitationError::Unresolvable {
            lookup_error: ids.lookup_error.clone(),
            provider: ProviderId::OpenAlex,
        }),
        Some(key) => {
            let provider = OpenAlexProvider::new();
            let page = match direction {
                Direction::References => provider.references_page(&key, options).await,
                Direction::CitedBy => provider.citations_page(&key, options).await,
            };
            seed_error(ProviderId::OpenAlex, key, page)
        }
    };
    results.insert(ProviderId::OpenAlex, openalex);

    let semantic_scholar = match ids.semantic_scholar_key() {
        None => Err(CitationError::Unresolvable {
            lookup_error: ids.lookup_error.clone(),
            provider: ProviderId::SemanticScholar,
        }),
        Some(key) => {
            match SemanticScholarProvider::new(config.semantic_scholar_api_key.as_deref()) {
                Err(e) => Err(e.into()),
                Ok(provider) => {
                    let page = match direction {
                        Direction::References => provider.references_page(&key, options).await,
                        Direction::CitedBy => provider.citations_page(&key, options).await,
                    };
                    seed_error(ProviderId::SemanticScholar, key, page)
                }
            }
        }
    };
    results.insert(ProviderId::SemanticScholar, semantic_scholar);

    Ok(results)
}

/// Names a provider's `NotFound` as the seed missing there under `key`.
fn seed_error(
    provider: ProviderId,
    key: String,
    page: Result<SearchPage, ProviderError>,
) -> Result<SearchPage, CitationError> {
    page.map_err(|e| match e {
        ProviderError::NotFound => CitationError::SeedNotFound { provider, key },
        e => e.into(),
    })
}

/// What's known about the seed that a citation provider can look it up by.
#[derive(Debug, Default, PartialEq, Eq)]
struct SeedIds {
    openalex: Option<String>,
    semantic_scholar: Option<String>,
    doi: Option<String>,
    /// Without a version suffix, e.g. `1706.03762`.
    arxiv: Option<String>,
    /// Why fetching a `provider:id` seed to learn its DOI failed, if it did.
    lookup_error: Option<SeedLookupError>,
}

impl SeedIds {
    /// Reads `seed` as a citation key when it has no `:` (a citation key
    /// never does — see `show_reference`), and as a `provider:id` reference
    /// otherwise. A reference from one provider is fetched to learn its DOI,
    /// so the other provider can look it up; if that fetch fails, the other
    /// provider just reports the seed as unresolvable.
    async fn resolve(seed: &str, root: &Path, config: &Config) -> Result<Self, PaxError> {
        if !seed.contains(':') {
            let library = Library::load(&nix::papers_path(root))
                .map_err(|_| PaxError::NoSuchPaper(seed.to_string()))?;
            let paper = library
                .find(seed)
                .ok_or_else(|| PaxError::NoSuchPaper(seed.to_string()))?;
            return Ok(Self::from_paper(paper));
        }

        let id: CandidateId = seed.parse()?;
        let mut ids = match id.provider {
            ProviderId::Crossref => return Ok(Self::from_doi(Some(&id.native_id))),
            ProviderId::OpenAlex => SeedIds {
                openalex: Some(id.native_id.clone()),
                ..SeedIds::default()
            },
            ProviderId::SemanticScholar => SeedIds {
                semantic_scholar: Some(id.native_id.clone()),
                ..SeedIds::default()
            },
            ProviderId::ArXiv => SeedIds {
                arxiv: Some(strip_arxiv_version(&id.native_id).to_string()),
                ..SeedIds::default()
            },
        };
        match resolve_candidate(&id, config).await {
            Ok(work) => ids.doi = work.doi.map(|doi| normalize_doi(&doi).to_string()),
            Err(error) => {
                ids.lookup_error = Some(SeedLookupError {
                    provider: id.provider,
                    error,
                })
            }
        }
        Ok(ids)
    }

    /// From a DOI alone, recognizing arXiv's own DOIs (`10.48550/arXiv.<id>`).
    fn from_doi(doi: Option<&str>) -> Self {
        let doi = doi.map(|doi| normalize_doi(doi).to_string());
        let arxiv = doi
            .as_deref()
            .and_then(arxiv_id_from_doi)
            .map(str::to_string);
        SeedIds {
            doi,
            arxiv,
            ..SeedIds::default()
        }
    }

    /// From a declared paper: its DOI, and its arXiv id when the DOI is
    /// arXiv's own or the PDF source is on arXiv — a paper added from arXiv
    /// often has no DOI at all.
    fn from_paper(paper: &Paper) -> Self {
        let mut ids = Self::from_doi(paper.identity.doi.as_deref());
        if ids.arxiv.is_none() {
            ids.arxiv = paper
                .artifact
                .source_url
                .as_deref()
                .and_then(arxiv_id_from_url)
                .map(str::to_string);
        }
        ids
    }

    /// The key `OpenAlexProvider` looks the seed up by.
    fn openalex_key(&self) -> Option<String> {
        self.openalex
            .clone()
            .or_else(|| self.doi.as_ref().map(|doi| format!("doi:{doi}")))
            .or_else(|| {
                self.arxiv
                    .as_ref()
                    .map(|arxiv| format!("doi:{ARXIV_DOI_PREFIX}{arxiv}"))
            })
    }

    /// The key `SemanticScholarProvider` looks the seed up by.
    fn semantic_scholar_key(&self) -> Option<String> {
        self.semantic_scholar
            .clone()
            .or_else(|| self.doi.as_ref().map(|doi| format!("DOI:{doi}")))
            .or_else(|| self.arxiv.as_ref().map(|arxiv| format!("ARXIV:{arxiv}")))
    }
}

/// The DOI prefix arXiv registers for every paper, e.g.
/// `10.48550/arXiv.1706.03762`.
const ARXIV_DOI_PREFIX: &str = "10.48550/arXiv.";

fn arxiv_id_from_doi(doi: &str) -> Option<&str> {
    let prefix = doi.get(..ARXIV_DOI_PREFIX.len())?;
    prefix
        .eq_ignore_ascii_case(ARXIV_DOI_PREFIX)
        .then(|| &doi[ARXIV_DOI_PREFIX.len()..])
        .filter(|id| !id.is_empty())
}

/// The arXiv id in an `arxiv.org/abs/…` or `arxiv.org/pdf/…` URL, without
/// version or `.pdf` suffix.
fn arxiv_id_from_url(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once("arxiv.org/")?;
    let id = rest
        .strip_prefix("abs/")
        .or_else(|| rest.strip_prefix("pdf/"))?;
    let id = id.split(['?', '#']).next()?;
    let id = id.strip_suffix(".pdf").unwrap_or(id);
    Some(strip_arxiv_version(id)).filter(|id| !id.is_empty())
}

/// `1706.03762v7` → `1706.03762`: the providers index the paper, not a
/// version of it.
fn strip_arxiv_version(id: &str) -> &str {
    match id.rsplit_once('v') {
        Some((base, version))
            if !base.is_empty()
                && !version.is_empty()
                && version.chars().all(|c| c.is_ascii_digit()) =>
        {
            base
        }
        _ => id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_doi_seed_is_looked_up_by_doi_everywhere() {
        let ids = SeedIds::from_doi(Some("https://doi.org/10.1145/359545.359563"));
        assert_eq!(
            ids.openalex_key().as_deref(),
            Some("doi:10.1145/359545.359563")
        );
        assert_eq!(
            ids.semantic_scholar_key().as_deref(),
            Some("DOI:10.1145/359545.359563")
        );
    }

    #[test]
    fn a_provider_s_own_id_wins_over_the_doi() {
        let ids = SeedIds {
            openalex: Some("W3137220996".to_string()),
            doi: Some("10.1145/359545.359563".to_string()),
            ..SeedIds::default()
        };
        assert_eq!(ids.openalex_key().as_deref(), Some("W3137220996"));
        assert_eq!(
            ids.semantic_scholar_key().as_deref(),
            Some("DOI:10.1145/359545.359563")
        );
    }

    #[test]
    fn an_arxiv_seed_without_a_doi_uses_arxiv_s_own_doi_and_id() {
        let ids = SeedIds {
            arxiv: Some("1706.03762".to_string()),
            ..SeedIds::default()
        };
        assert_eq!(
            ids.openalex_key().as_deref(),
            Some("doi:10.48550/arXiv.1706.03762")
        );
        assert_eq!(
            ids.semantic_scholar_key().as_deref(),
            Some("ARXIV:1706.03762")
        );
    }

    #[test]
    fn an_arxiv_doi_also_yields_the_arxiv_id() {
        let ids = SeedIds::from_doi(Some("10.48550/ARXIV.1706.03762"));
        assert_eq!(ids.arxiv.as_deref(), Some("1706.03762"));
        assert_eq!(SeedIds::from_doi(Some("10.1145/x")).arxiv, None);
    }

    #[test]
    fn a_seed_with_nothing_to_look_up_by_is_unresolvable() {
        let ids = SeedIds::from_doi(None);
        assert_eq!(ids.openalex_key(), None);
        assert_eq!(ids.semantic_scholar_key(), None);
    }

    #[test]
    fn a_declared_paper_without_a_doi_falls_back_to_its_arxiv_source() {
        let paper = Paper {
            identity: crate::paper::Identity {
                doi: None,
                title: "Attention Is All You Need".to_string(),
                authors: Vec::new(),
                year: Some(2017),
                venue: None,
            },
            artifact: crate::paper::Artifact {
                source_url: Some("https://arxiv.org/pdf/1706.03762v7".to_string()),
                hash: None,
            },
            local: crate::paper::Local {
                citation_key: "vaswani2017".to_string(),
                tags: Vec::new(),
                notes: None,
            },
        };
        let ids = SeedIds::from_paper(&paper);
        assert_eq!(
            ids.semantic_scholar_key().as_deref(),
            Some("ARXIV:1706.03762")
        );
    }

    #[test]
    fn arxiv_ids_are_read_from_abs_and_pdf_urls() {
        for url in [
            "https://arxiv.org/abs/1706.03762",
            "https://arxiv.org/pdf/1706.03762v7",
            "http://arxiv.org/pdf/1706.03762v7.pdf",
        ] {
            assert_eq!(arxiv_id_from_url(url), Some("1706.03762"), "{url}");
        }
        assert_eq!(
            arxiv_id_from_url("https://arxiv.org/abs/cs/0112017v1"),
            Some("cs/0112017")
        );
        assert_eq!(arxiv_id_from_url("https://example.org/paper.pdf"), None);
        assert_eq!(arxiv_id_from_url("https://arxiv.org/list/cs.DC"), None);
    }

    #[test]
    fn arxiv_versions_are_stripped() {
        assert_eq!(strip_arxiv_version("1706.03762v7"), "1706.03762");
        assert_eq!(strip_arxiv_version("1706.03762"), "1706.03762");
        assert_eq!(strip_arxiv_version("cs/0112017v1"), "cs/0112017");
        assert_eq!(strip_arxiv_version("solv-int/9901001"), "solv-int/9901001");
    }

    #[test]
    fn not_found_names_the_provider_and_key() {
        let error = seed_error(
            ProviderId::SemanticScholar,
            "DOI:10.1/x".to_string(),
            Err(ProviderError::NotFound),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "semanticscholar has no record of the seed (looked up as DOI:10.1/x)"
        );
    }

    #[test]
    fn a_citation_key_missing_from_the_library_is_an_error() {
        let root = std::env::temp_dir().join(format!("pax-citations-{}", std::process::id()));
        let config = Config::default();
        let result = std::pin::pin!(SeedIds::resolve("nosuchkey2000", &root, &config));
        let poll = result.poll(&mut std::task::Context::from_waker(std::task::Waker::noop()));
        assert!(matches!(
            poll,
            std::task::Poll::Ready(Err(PaxError::NoSuchPaper(_)))
        ));
    }
}
