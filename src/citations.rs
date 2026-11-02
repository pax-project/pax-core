//! Citation graph lookups: a seed paper's references (what it cites) and the
//! papers citing it, from the providers that expose real citation edges —
//! OpenAlex and Semantic Scholar — and the papers those providers recommend
//! as related to it ([`related`]), resolved and merged the same way. Results
//! stay per provider: each list is that provider's own, with its own total.

use std::collections::HashMap;
use std::path::Path;

use thiserror::Error;

use crate::error::PaxError;
use crate::exclusions::Exclusions;
use crate::library::Library;
use crate::merge::MergedWork;
use crate::names::{self, Named};
use crate::paper::Paper;
use crate::provider::{
    CandidateId, OpenAlexProvider, ProviderError, ProviderId, SearchOptions, SearchPage,
    SemanticScholarProvider,
};
use crate::{Config, nix, normalize_doi, resolve_candidate};

/// Why one provider's citation list couldn't be fetched.
#[derive(Debug, Clone, Error)]
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

impl CitationError {
    /// Why this error leaves the provider's list missing.
    pub fn reason(&self) -> IncompleteReason {
        match self {
            CitationError::Unresolvable { .. } | CitationError::SeedNotFound { .. } => {
                IncompleteReason::Unresolved
            }
            CitationError::Provider(ProviderError::Withheld { .. }) => IncompleteReason::Withheld,
            // `references_page`/`citations_page` report a missing seed as
            // `NotFound`, which `seed_error` names `SeedNotFound`.
            CitationError::Provider(ProviderError::NotFound) => IncompleteReason::Unresolved,
            CitationError::Provider(_) => IncompleteReason::Failed,
        }
    }
}

/// Why one provider's citation list for a seed is incomplete, so a caller
/// knows not to treat it as the paper's whole list — and, for a review, to
/// fall back to the paper's own bibliography.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum IncompleteReason {
    /// The provider holds the list back, e.g. at the publisher's request.
    Withheld,
    /// The request failed: network, rate limit, an unreadable response.
    Failed,
    /// The seed can't be looked up in this provider: no id, DOI or arXiv id
    /// is known for it there, or the provider has no record of it.
    Unresolved,
    /// The limit stopped retrieval before the provider's total (see
    /// [`SearchPage::is_complete`]). The list returned is good, just short —
    /// the only reason that isn't a failure.
    Truncated,
}

impl IncompleteReason {
    /// The reason's name, as the CLI prints it and the search log records
    /// it: `withheld`, `failed`, `unresolved` or `truncated`.
    pub fn as_str(self) -> &'static str {
        match self {
            IncompleteReason::Withheld => "withheld",
            IncompleteReason::Failed => "failed",
            IncompleteReason::Unresolved => "unresolved",
            IncompleteReason::Truncated => "truncated",
        }
    }

    /// Whether the provider returned no list at all, rather than a short
    /// one. Every reason but [`IncompleteReason::Truncated`].
    pub fn is_missing(self) -> bool {
        self != IncompleteReason::Truncated
    }
}

impl std::fmt::Display for IncompleteReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One provider whose citation list for a seed is incomplete, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Incomplete {
    pub provider: ProviderId,
    pub reason: IncompleteReason,
}

/// Each citation provider's list, by provider. A provider's own failure —
/// including not knowing the seed — doesn't fail the others.
pub type CitationResults = HashMap<ProviderId, Result<SearchPage, CitationError>>;

/// Why one provider's result is incomplete, if it is: the error's
/// [`CitationError::reason`], or [`IncompleteReason::Truncated`] for a list
/// retrieval stopped short of the provider's total.
pub fn incomplete_reason(result: &Result<SearchPage, CitationError>) -> Option<IncompleteReason> {
    match result {
        Ok(page) => (!page.is_complete()).then_some(IncompleteReason::Truncated),
        Err(e) => Some(e.reason()),
    }
}

/// Every provider in `results` whose list is incomplete, with why, in
/// [`CITATION_PROVIDERS`] order. Empty when every list is whole.
pub fn incomplete_lists(results: &CitationResults) -> Vec<Incomplete> {
    CITATION_PROVIDERS
        .iter()
        .filter_map(|&provider| {
            let reason = incomplete_reason(results.get(&provider)?)?;
            Some(Incomplete { provider, reason })
        })
        .collect()
}

/// The providers [`references`], [`cited_by`] and [`related`] ask, in display
/// order. Crossref isn't one: its reference lists are publisher-deposited
/// and often incomplete, and it has no forward citations or related works.
pub const CITATION_PROVIDERS: [ProviderId; 2] = [ProviderId::OpenAlex, ProviderId::SemanticScholar];

/// The works `seed` cites, per provider, paged up to `options.limit` with
/// `options.filters` applied. `seed` is a `provider:id` reference (any
/// provider) or a library name in the library at `root`: a declared paper's
/// citation key or alias, or an excluded paper's alias. It's resolved to
/// each provider's own id, via its DOI or arXiv id when needed.
///
/// Errors only when `seed` itself is invalid: a malformed reference, or a
/// name not in the library.
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

/// The works each of `seeds` cites, per seed and per provider, as
/// [`references`] for each one; see [`SeedResults`] for how they're merged
/// and attributed. Seeds are looked up one after another, as `search_all`
/// asks providers, since Semantic Scholar's unauthenticated rate limit is
/// tight and a snowballing round can have dozens of seeds. `options` apply
/// per seed per provider.
///
/// Never fails as a whole: an invalid seed fails only itself, with its error
/// in its [`SeedResult`]. A seed that's the same paper as an earlier one
/// (the same string, or sharing a provider id, DOI or arXiv id with it) is
/// resolved once: its lists are the earlier seed's (see
/// [`SeedResult::same_as`]).
pub async fn references_many<S: AsRef<str>>(
    seeds: &[S],
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> SeedResults {
    citation_lists_many(seeds, Direction::References, options, root, config).await
}

/// The works citing each of `seeds`; otherwise as [`references_many`].
pub async fn cited_by_many<S: AsRef<str>>(
    seeds: &[S],
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> SeedResults {
    citation_lists_many(seeds, Direction::CitedBy, options, root, config).await
}

/// The papers the providers themselves recommend as related to `seed`, per
/// provider: OpenAlex's `related_works` (chosen from shared concepts) and
/// Semantic Scholar's Recommendations API. Not citation edges, so they reach
/// papers outside the seed's citation neighbourhood, as a diverse snowballing
/// start set wants; deterministic for a given provider state. `seed`,
/// `options` and the errors are as for [`references`]; filters apply
/// server-side in OpenAlex and, for year and type, after retrieval in
/// Semantic Scholar.
///
/// Neither provider reports a total, so every page's `total` is `None`, and
/// [`incomplete_reason`] gives [`IncompleteReason::Truncated`] for every list
/// returned: a related list is never presented as complete. Only an error
/// means a provider's list is missing.
pub async fn related(
    seed: &str,
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> Result<CitationResults, PaxError> {
    citation_lists(seed, Direction::Related, options, root, config).await
}

/// The related works of each of `seeds`, one seed after another; otherwise
/// as [`related`] for each, merged and attributed as [`references_many`]'s
/// are (see [`SeedResults`]).
///
/// Each seed's recommendations are fetched on their own, so every record
/// can be attributed to the seed it was recommended for, and each seed logs
/// what it would alone. Semantic Scholar's multi-paper endpoint, which
/// recommends for a set of papers taken together, isn't used: its list
/// belongs to no one seed.
pub async fn related_many<S: AsRef<str>>(
    seeds: &[S],
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> SeedResults {
    citation_lists_many(seeds, Direction::Related, options, root, config).await
}

/// One seed's part in a multi-seed lookup.
#[derive(Debug)]
#[non_exhaustive]
pub struct SeedResult {
    /// The seed as given: a `provider:id` reference or a citation key.
    pub seed: String,
    /// The index of an earlier seed that is the same paper, whose lists
    /// these are, copied rather than fetched again. `None` for a seed looked
    /// up in its own right.
    pub same_as: Option<usize>,
    /// Each citation provider's list for the seed, as [`references`] returns
    /// it; an error when the seed itself is invalid (a malformed reference,
    /// or a citation key not in the library).
    pub results: Result<CitationResults, PaxError>,
}

impl SeedResult {
    /// The seed's incomplete provider lists, with why ([`incomplete_lists`]).
    /// Empty for an invalid seed, whose `results` error says why instead.
    pub fn incomplete(&self) -> Vec<Incomplete> {
        self.results
            .as_ref()
            .map(incomplete_lists)
            .unwrap_or_default()
    }
}

/// Every seed's lists from [`references_many`], [`cited_by_many`] or
/// [`related_many`], in the order the seeds were given.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct SeedResults {
    pub seeds: Vec<SeedResult>,
}

impl SeedResults {
    /// One list merged across every seed and provider: seeds in order, and
    /// within a seed providers in [`crate::merge::MERGE_ORDER`], as
    /// [`crate::merge_pages`] takes them. Each merged work records which seed
    /// and provider each of its records came from. A seed that's the
    /// [`same_as`](SeedResult::same_as) an earlier one contributes its copy,
    /// so its records count, and show up as duplicates, as they would had it
    /// been looked up on its own.
    pub fn merge(&self) -> SeedMergeReport {
        let mut sources = Vec::new();
        let mut works = Vec::new();
        for (seed, result) in self.seeds.iter().enumerate() {
            let Ok(results) = &result.results else {
                continue;
            };
            for &provider in crate::merge::MERGE_ORDER {
                if let Some(Ok(page)) = results.get(&provider) {
                    sources.extend(page.works.iter().map(|_| RecordSource { seed, provider }));
                    works.extend(page.works.iter().cloned());
                }
            }
        }
        let (report, positions) = crate::merge::merge_indexed(works);
        SeedMergeReport {
            before: report.before,
            after: report.after,
            unmatched: report.unmatched,
            works: report
                .works
                .into_iter()
                .zip(positions)
                .map(|(merged, positions)| SeedMergedWork {
                    merged,
                    sources: positions.into_iter().map(|i| sources[i]).collect(),
                })
                .collect(),
        }
    }

    /// Whether any seed had any provider return a list.
    pub fn any_listed(&self) -> bool {
        self.seeds.iter().any(|seed| {
            seed.results
                .as_ref()
                .is_ok_and(|results| results.values().any(Result::is_ok))
        })
    }
}

/// Where one record of a merged work came from: the seed (an index into
/// [`SeedResults::seeds`]) whose list held it, and the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecordSource {
    pub seed: usize,
    pub provider: ProviderId,
}

/// One paper in a list merged across seeds.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SeedMergedWork {
    /// The merged records, as [`crate::merge_pages`] would group them.
    pub merged: MergedWork,
    /// Where each record came from, in [`MergedWork::records`] order.
    pub sources: Vec<RecordSource>,
}

impl SeedMergedWork {
    /// The seeds whose lists returned this paper, without repeats, in seed
    /// order.
    pub fn seeds(&self) -> Vec<usize> {
        let mut seeds: Vec<usize> = self.sources.iter().map(|source| source.seed).collect();
        seeds.sort_unstable();
        seeds.dedup();
        seeds
    }

    /// The providers that returned this paper, for any seed, without
    /// repeats, in [`crate::merge::MERGE_ORDER`].
    pub fn providers(&self) -> Vec<ProviderId> {
        crate::merge::MERGE_ORDER
            .iter()
            .copied()
            .filter(|provider| self.sources.iter().any(|s| s.provider == *provider))
            .collect()
    }
}

/// The outcome of [`SeedResults::merge`]: one [`SeedMergedWork`] per
/// distinct paper, plus the same counts as [`crate::MergeReport`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct SeedMergeReport {
    /// One entry per distinct paper, in order of first appearance.
    pub works: Vec<SeedMergedWork>,
    /// Records before merging, across every seed and provider.
    pub before: usize,
    /// Records after merging (`works.len()`).
    pub after: usize,
    /// Records with no identity key, kept as their own rows.
    pub unmatched: usize,
}

impl SeedMergeReport {
    /// Records removed as duplicates (`before - after`).
    pub fn duplicates(&self) -> usize {
        self.before - self.after
    }
}

async fn citation_lists_many<S: AsRef<str>>(
    seeds: &[S],
    direction: Direction,
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> SeedResults {
    let mut out = SeedResults::default();
    // The ids of each seed looked up in its own right, by its index.
    let mut looked_up: Vec<(usize, SeedIds)> = Vec::new();
    for seed in seeds {
        let seed = seed.as_ref();
        // The same string as a seed already looked up needs no resolving.
        let repeat = looked_up
            .iter()
            .map(|(index, _)| *index)
            .find(|&index| out.seeds[index].seed == seed);
        let (same_as, results) = match repeat {
            Some(index) => (Some(index), copy_results(&out.seeds[index])),
            None => match SeedIds::resolve(seed, root, config).await {
                Err(e) => (None, Err(e)),
                Ok(ids) => match looked_up.iter().find(|(_, other)| ids.same_paper(other)) {
                    Some(&(index, _)) => (Some(index), copy_results(&out.seeds[index])),
                    None => {
                        let results = lists_for(&ids, direction, options, config).await;
                        looked_up.push((out.seeds.len(), ids));
                        (None, Ok(results))
                    }
                },
            },
        };
        out.seeds.push(SeedResult {
            seed: seed.to_string(),
            same_as,
            results,
        });
    }
    out
}

/// A copy of a looked-up seed's lists, for a later seed that's the same
/// paper.
fn copy_results(seed: &SeedResult) -> Result<CitationResults, PaxError> {
    Ok(seed
        .results
        .as_ref()
        .expect("only a seed with lists is looked up")
        .clone())
}

/// Which of a seed's lists to fetch.
#[derive(Clone, Copy)]
enum Direction {
    References,
    CitedBy,
    /// The providers' related works: not a citation edge, but resolved and
    /// merged the same way.
    Related,
}

async fn citation_lists(
    seed: &str,
    direction: Direction,
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> Result<CitationResults, PaxError> {
    let ids = SeedIds::resolve(seed, root, config).await?;
    Ok(lists_for(&ids, direction, options, config).await)
}

/// Each citation provider's list for the seed `ids` describes.
async fn lists_for(
    ids: &SeedIds,
    direction: Direction,
    options: &SearchOptions,
    config: &Config,
) -> CitationResults {
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
                Direction::Related => provider.related_page(&key, options).await,
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
                        Direction::Related => provider.recommendations_page(&key, options).await,
                    };
                    seed_error(ProviderId::SemanticScholar, key, page)
                }
            }
        }
    };
    results.insert(ProviderId::SemanticScholar, semantic_scholar);

    results
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
    /// Reads `seed` as a library name when it has no `:` (a citation key or
    /// alias never has one — see `show_reference`), and as a `provider:id`
    /// reference otherwise. An excluded paper's alias resolves through its
    /// exclusion key, a DOI or a `provider:id`. A reference from one provider
    /// is fetched to learn its DOI, so the other provider can look it up; if
    /// that fetch fails, the other provider just reports the seed as
    /// unresolvable.
    async fn resolve(seed: &str, root: &Path, config: &Config) -> Result<Self, PaxError> {
        let reference = if seed.contains(':') {
            seed.to_string()
        } else {
            let library = Library::load(&nix::papers_path(root))
                .map_err(|_| PaxError::NoSuchPaper(seed.to_string()))?;
            let exclusions = Exclusions::load(&nix::excluded_path(root))?;
            match names::lookup(seed, &library, &exclusions) {
                Some(Named::Declared(paper)) => return Ok(Self::from_paper(paper)),
                // An excluded paper's alias: its exclusion key is a DOI or a
                // `provider:id` reference, resolved like one given directly.
                Some(Named::Excluded(exclusion)) if exclusion.key.starts_with("10.") => {
                    return Ok(Self::from_doi(Some(&exclusion.key)));
                }
                Some(Named::Excluded(exclusion)) => exclusion.key.clone(),
                None => return Err(PaxError::NoSuchPaper(seed.to_string())),
            }
        };

        let id: CandidateId = reference.parse()?;
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
            // dblp has no citation graph: the seed is fetched below for its DOI.
            ProviderId::Dblp => SeedIds::default(),
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

    /// Whether `other` is the same paper: they share a provider id, a DOI
    /// (case-insensitively) or an arXiv id.
    fn same_paper(&self, other: &SeedIds) -> bool {
        let shared = |a: &Option<String>, b: &Option<String>| {
            a.as_deref()
                .zip(b.as_deref())
                .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
        };
        shared(&self.openalex, &other.openalex)
            || shared(&self.semantic_scholar, &other.semantic_scholar)
            || shared(&self.doi, &other.doi)
            || shared(&self.arxiv, &other.arxiv)
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
    use crate::provider::CandidateWork;

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
                alias: None,
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

    fn work(
        provider: ProviderId,
        native_id: &str,
        title: &str,
        doi: Option<&str>,
    ) -> CandidateWork {
        CandidateWork {
            id: CandidateId {
                provider,
                native_id: native_id.to_string(),
            },
            title: title.to_string(),
            authors: Vec::new(),
            publish_date: "2017-06-12".to_string(),
            doi: doi.map(str::to_string),
            pdf_url: None,
            venue: None,
            abstract_text: None,
        }
    }

    #[test]
    fn merging_two_providers_reference_lists_collapses_shared_papers() {
        let mut results: CitationResults = HashMap::new();
        results.insert(
            ProviderId::SemanticScholar,
            Ok(SearchPage::new(
                vec![
                    work(
                        ProviderId::SemanticScholar,
                        "s1",
                        "Attention Is All You Need",
                        Some("10.48550/arXiv.1706.03762"),
                    ),
                    work(
                        ProviderId::SemanticScholar,
                        "s2",
                        "Deep Residual Learning",
                        Some("10.1109/CVPR.2016.90"),
                    ),
                    work(ProviderId::SemanticScholar, "s3", "", None),
                ],
                Some(3),
                None,
            )),
        );
        results.insert(
            ProviderId::OpenAlex,
            Ok(SearchPage::new(
                vec![
                    work(
                        ProviderId::OpenAlex,
                        "W1",
                        "Deep residual learning",
                        Some("https://doi.org/10.1109/cvpr.2016.90"),
                    ),
                    work(
                        ProviderId::OpenAlex,
                        "W2",
                        "Attention is all you need",
                        Some("https://doi.org/10.48550/arxiv.1706.03762"),
                    ),
                ],
                Some(2),
                None,
            )),
        );

        let report = crate::merge_pages(&results);
        assert_eq!(
            (
                report.before,
                report.after,
                report.duplicates(),
                report.unmatched
            ),
            (5, 3, 2, 1)
        );
        // OpenAlex comes first in provider order, so its records lead.
        assert_eq!(report.works[0].work.id.native_id, "W1");
        assert_eq!(
            report.works[0].providers(),
            [ProviderId::OpenAlex, ProviderId::SemanticScholar]
        );
        assert_eq!(
            report.works[1].providers(),
            [ProviderId::OpenAlex, ProviderId::SemanticScholar]
        );
    }

    #[test]
    fn merging_skips_a_provider_that_failed() {
        let mut results: CitationResults = HashMap::new();
        results.insert(
            ProviderId::OpenAlex,
            Ok(SearchPage::new(
                vec![work(ProviderId::OpenAlex, "W1", "A Study", Some("10.1/a"))],
                Some(1),
                None,
            )),
        );
        results.insert(
            ProviderId::SemanticScholar,
            Err(CitationError::SeedNotFound {
                provider: ProviderId::SemanticScholar,
                key: "DOI:10.1/seed".to_string(),
            }),
        );
        let report = crate::merge_pages(&results);
        assert_eq!((report.before, report.after), (1, 1));
    }

    #[test]
    fn each_citation_error_has_a_reason() {
        let cases = [
            (
                CitationError::Unresolvable {
                    provider: ProviderId::OpenAlex,
                    lookup_error: None,
                },
                IncompleteReason::Unresolved,
            ),
            (
                CitationError::SeedNotFound {
                    provider: ProviderId::OpenAlex,
                    key: "W1".to_string(),
                },
                IncompleteReason::Unresolved,
            ),
            (
                CitationError::Provider(ProviderError::Withheld {
                    message: "withheld".to_string(),
                }),
                IncompleteReason::Withheld,
            ),
            (
                CitationError::Provider(ProviderError::Request("429".to_string())),
                IncompleteReason::Failed,
            ),
        ];
        for (error, reason) in cases {
            assert_eq!(error.reason(), reason, "{error}");
        }
    }

    #[test]
    fn incomplete_lists_name_each_provider_and_reason_in_order() {
        let a = work(ProviderId::OpenAlex, "W1", "A Study", Some("10.1/a"));
        let mut results: CitationResults = HashMap::new();
        // Retrieved one of 40: the limit cut it short.
        results.insert(
            ProviderId::OpenAlex,
            Ok(SearchPage::new(vec![a.clone()], Some(40), None)),
        );
        results.insert(
            ProviderId::SemanticScholar,
            Err(CitationError::Provider(ProviderError::Withheld {
                message: "withheld".to_string(),
            })),
        );
        assert_eq!(
            incomplete_lists(&results),
            [
                Incomplete {
                    provider: ProviderId::OpenAlex,
                    reason: IncompleteReason::Truncated,
                },
                Incomplete {
                    provider: ProviderId::SemanticScholar,
                    reason: IncompleteReason::Withheld,
                },
            ]
        );

        results.insert(
            ProviderId::OpenAlex,
            Ok(SearchPage::new(vec![a], Some(1), None)),
        );
        results.remove(&ProviderId::SemanticScholar);
        assert!(incomplete_lists(&results).is_empty());
    }

    #[test]
    fn only_truncated_leaves_a_list() {
        assert!(!IncompleteReason::Truncated.is_missing());
        for reason in [
            IncompleteReason::Withheld,
            IncompleteReason::Failed,
            IncompleteReason::Unresolved,
        ] {
            assert!(reason.is_missing(), "{reason}");
        }
    }

    #[test]
    fn a_related_list_without_a_total_is_never_complete() {
        let a = work(ProviderId::OpenAlex, "W1", "A Study", Some("10.1/a"));
        let related: Result<SearchPage, CitationError> = Ok(SearchPage::new(vec![a], None, None));
        assert_eq!(
            incomplete_reason(&related),
            Some(IncompleteReason::Truncated)
        );
        let empty: Result<SearchPage, CitationError> = Ok(SearchPage::new(Vec::new(), None, None));
        assert_eq!(incomplete_reason(&empty), Some(IncompleteReason::Truncated));
    }

    #[test]
    fn related_works_of_seeds_resolve_like_citation_lists() {
        let root = std::env::temp_dir().join(format!("pax-related-many-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        nix::init_library(&root).unwrap();
        // No DOI and no arXiv source: neither provider can be asked, so no
        // network is needed.
        std::fs::write(
            nix::papers_path(&root),
            r#"{
  hoare1978 = {
    doi = null;
    title = "Communicating Sequential Processes";
    authors = [ "C. A. R. Hoare" ];
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
        let config = Config::default();
        let options = SearchOptions::default();
        let seeds = ["hoare1978", "nosuchkey2000", "hoare1978"];
        let future = std::pin::pin!(related_many(&seeds, &options, &root, &config));
        let poll = future.poll(&mut std::task::Context::from_waker(std::task::Waker::noop()));
        let std::task::Poll::Ready(results) = poll else {
            panic!("an offline lookup should finish at once");
        };
        let reasons: Vec<_> = results.seeds[0]
            .incomplete()
            .into_iter()
            .map(|incomplete| (incomplete.provider, incomplete.reason))
            .collect();
        assert_eq!(
            reasons,
            [
                (ProviderId::OpenAlex, IncompleteReason::Unresolved),
                (ProviderId::SemanticScholar, IncompleteReason::Unresolved),
            ]
        );
        assert!(matches!(
            results.seeds[1].results,
            Err(PaxError::NoSuchPaper(_))
        ));
        assert_eq!(results.seeds[2].same_as, Some(0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn aliases_resolve_as_seeds_declared_and_excluded() {
        let root = std::env::temp_dir().join(format!("pax-alias-seeds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        nix::init_library(&root).unwrap();
        std::fs::write(
            nix::papers_path(&root),
            r#"{
  dijkstra1968 = {
    doi = "10.1145/363095.363143";
    title = "Go To Statement Considered Harmful";
    authors = [ ];
    alias = "P17";
  };
}
"#,
        )
        .unwrap();
        std::fs::write(
            nix::excluded_path(&root),
            r#"{
  "10.1/abc" = { title = "T"; reason = "R"; stage = "screening"; alias = "P18"; };
}
"#,
        )
        .unwrap();
        let config = Config::default();
        let resolve = |seed: &'static str| {
            let future = std::pin::pin!(SeedIds::resolve(seed, &root, &config));
            match future.poll(&mut std::task::Context::from_waker(std::task::Waker::noop())) {
                std::task::Poll::Ready(result) => result,
                std::task::Poll::Pending => panic!("{seed}: a library name needs no network"),
            }
        };
        assert_eq!(
            resolve("P17").unwrap().doi.as_deref(),
            Some("10.1145/363095.363143")
        );
        assert_eq!(resolve("P18").unwrap().doi.as_deref(), Some("10.1/abc"));
        assert!(matches!(resolve("P19"), Err(PaxError::NoSuchPaper(_))));
        let _ = std::fs::remove_dir_all(&root);
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

    #[test]
    fn seeds_sharing_an_id_doi_or_arxiv_id_are_the_same_paper() {
        let doi = SeedIds::from_doi(Some("10.1145/359545.359563"));
        let openalex = SeedIds {
            openalex: Some("W1".to_string()),
            doi: Some("10.1145/359545.359563".to_uppercase()),
            ..SeedIds::default()
        };
        assert!(doi.same_paper(&openalex));
        let arxiv = SeedIds {
            arxiv: Some("1706.03762".to_string()),
            ..SeedIds::default()
        };
        assert!(arxiv.same_paper(&SeedIds::from_doi(Some("10.48550/arXiv.1706.03762"))));
        assert!(!doi.same_paper(&arxiv));
        // Knowing nothing is never a match.
        assert!(!SeedIds::default().same_paper(&SeedIds::default()));
    }

    fn page(works: Vec<CandidateWork>) -> Result<SearchPage, CitationError> {
        let total = works.len() as u64;
        Ok(SearchPage::new(works, Some(total), None))
    }

    fn seed_result(seed: &str, results: Result<CitationResults, PaxError>) -> SeedResult {
        SeedResult {
            seed: seed.to_string(),
            same_as: None,
            results,
        }
    }

    #[test]
    fn a_merged_hit_names_every_seed_and_provider_that_returned_it() {
        let shared = |provider, id| work(provider, id, "Shared", Some("10.1/shared"));
        let results = SeedResults {
            seeds: vec![
                seed_result(
                    "a2020",
                    Ok(HashMap::from([
                        (
                            ProviderId::OpenAlex,
                            page(vec![
                                shared(ProviderId::OpenAlex, "W1"),
                                work(ProviderId::OpenAlex, "W2", "Only A", Some("10.1/a")),
                            ]),
                        ),
                        (
                            ProviderId::SemanticScholar,
                            Err(CitationError::Provider(ProviderError::Withheld {
                                message: "withheld".to_string(),
                            })),
                        ),
                    ])),
                ),
                seed_result("nosuchkey", Err(PaxError::NoSuchPaper("nosuchkey".into()))),
                seed_result(
                    "openalex:W9",
                    Ok(HashMap::from([(
                        ProviderId::SemanticScholar,
                        page(vec![shared(ProviderId::SemanticScholar, "s1")]),
                    )])),
                ),
            ],
        };
        let report = results.merge();
        assert_eq!(
            (report.before, report.after, report.duplicates()),
            (3, 2, 1)
        );
        let hit = &report.works[0];
        assert_eq!(hit.merged.work.id.native_id, "W1");
        assert_eq!(hit.seeds(), [0, 2]);
        assert_eq!(
            hit.providers(),
            [ProviderId::OpenAlex, ProviderId::SemanticScholar]
        );
        assert_eq!(
            hit.sources,
            [
                RecordSource {
                    seed: 0,
                    provider: ProviderId::OpenAlex
                },
                RecordSource {
                    seed: 2,
                    provider: ProviderId::SemanticScholar
                },
            ]
        );
        assert_eq!(report.works[1].seeds(), [0]);
        assert!(results.any_listed());
        assert_eq!(
            results.seeds[0].incomplete(),
            [Incomplete {
                provider: ProviderId::SemanticScholar,
                reason: IncompleteReason::Withheld,
            }]
        );
        assert!(results.seeds[1].incomplete().is_empty());
    }

    #[test]
    fn an_invalid_seed_fails_only_itself_and_a_repeat_is_looked_up_once() {
        let root = std::env::temp_dir().join(format!("pax-citations-many-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        nix::init_library(&root).unwrap();
        // No DOI and no arXiv source: neither provider can be asked, so no
        // network is needed.
        std::fs::write(
            nix::papers_path(&root),
            r#"{
  hoare1978 = {
    doi = null;
    title = "Communicating Sequential Processes";
    authors = [ "C. A. R. Hoare" ];
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
        let config = Config::default();
        let options = SearchOptions::default();
        let seeds = ["hoare1978", "nosuchkey2000", "hoare1978", "bad:"];
        let future = std::pin::pin!(references_many(&seeds, &options, &root, &config));
        let poll = future.poll(&mut std::task::Context::from_waker(std::task::Waker::noop()));
        let std::task::Poll::Ready(results) = poll else {
            panic!("an offline lookup should finish at once");
        };
        let names: Vec<&str> = results.seeds.iter().map(|s| s.seed.as_str()).collect();
        assert_eq!(names, seeds);
        assert!(results.seeds[0].results.is_ok());
        assert!(matches!(
            results.seeds[1].results,
            Err(PaxError::NoSuchPaper(_))
        ));
        assert_eq!(results.seeds[2].same_as, Some(0));
        assert!(results.seeds[2].results.is_ok());
        assert!(matches!(
            results.seeds[3].results,
            Err(PaxError::InvalidReference(_))
        ));
        let reasons: Vec<_> = results.seeds[0]
            .incomplete()
            .into_iter()
            .map(|incomplete| incomplete.reason)
            .collect();
        assert_eq!(
            reasons,
            [IncompleteReason::Unresolved, IncompleteReason::Unresolved]
        );
        assert!(!results.any_listed());
        let _ = std::fs::remove_dir_all(&root);
    }
}
