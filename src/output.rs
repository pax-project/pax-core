//! Serializable result shapes: what a command returns, as one JSON
//! document. The `pax` CLI prints them with `--json`, and any other client
//! (e.g. an MCP server) can return the same shapes, so every client's
//! structured output agrees field for field.
//!
//! Each shape is built from the library's own results with its `new` (or a
//! `From` impl); none is meant to be built field by field, so every one is
//! non-exhaustive and fields can be added in a minor release. With the
//! `schemars` feature, each also derives `schemars::JsonSchema`.

use std::collections::HashMap;
use std::fmt::Display;

use serde::Serialize;

use crate::citations::{CITATION_PROVIDERS, SeedMergeReport, SeedResults};
use crate::exclusions::Exclusion;
use crate::library_index::LibraryIndex;
use crate::merge::{MERGE_ORDER, MergeReport, MergedWork};
use crate::paper::Paper;
use crate::provider::{CandidateWork, ProviderError, ProviderId, SearchPage};
use crate::search_log::LogOutcome;

/// One provider's record of a paper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Candidate {
    /// The reference `show` and `add` accept, e.g. `openalex:W2072794470`.
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub publish_date: String,
    pub doi: Option<String>,
    pub venue: Option<String>,
    /// Whether the provider found an open-access PDF.
    pub pdf_available: bool,
    /// Whether the paper is already declared in the library (matched by DOI
    /// or arXiv id, so any provider's record of it counts).
    pub in_library: bool,
    /// Whether the paper is excluded in `research/excluded.nix`.
    pub excluded: bool,
}

impl Candidate {
    /// `work`, flagged against `index`.
    pub fn new(work: &CandidateWork, index: &LibraryIndex) -> Self {
        Candidate::flagged(work, index.contains(work), index.is_excluded(work))
    }

    /// `work`, flagged as given: a merged row is flagged if any of its
    /// records is.
    pub(crate) fn flagged(work: &CandidateWork, in_library: bool, excluded: bool) -> Self {
        Candidate {
            id: work.id.to_string(),
            title: work.title.clone(),
            authors: work.authors.clone(),
            publish_date: work.publish_date.clone(),
            doi: work.doi.clone(),
            venue: work.venue.clone(),
            pdf_available: work.pdf_url.is_some(),
            in_library,
            excluded,
        }
    }
}

/// One provider's record of a paper in full, as `show` gives it: the
/// [`Candidate`] fields, plus its PDF source and abstract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct CandidateDetails {
    #[serde(flatten)]
    pub candidate: Candidate,
    /// The open-access PDF the provider found, if any.
    pub pdf_url: Option<String>,
    /// The record's abstract, if the provider has one.
    #[serde(rename = "abstract")]
    pub abstract_text: Option<String>,
}

impl CandidateDetails {
    /// `work`, flagged against `index`.
    pub fn new(work: &CandidateWork, index: &LibraryIndex) -> Self {
        CandidateDetails {
            candidate: Candidate::new(work, index),
            pdf_url: work.pdf_url.clone(),
            abstract_text: work.abstract_text.clone(),
        }
    }
}

/// A paper declared in the library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct DeclaredPaper {
    pub citation_key: String,
    /// The paper's alias (e.g. the review's study ID `P17`), accepted
    /// wherever its citation key is, when it has one.
    pub alias: Option<String>,
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub doi: Option<String>,
    pub venue: Option<String>,
    pub tags: Vec<String>,
    pub notes: Option<String>,
    /// Whether the PDF has been fetched (its Nix hash is recorded).
    pub fetched: bool,
    /// Where the PDF is downloaded from; null when no open-access copy was
    /// found, or the PDF was added from a local file.
    pub source_url: Option<String>,
    /// The PDF's recorded Nix hash, once fetched.
    pub hash: Option<String>,
}

impl From<&Paper> for DeclaredPaper {
    fn from(paper: &Paper) -> Self {
        DeclaredPaper {
            citation_key: paper.local.citation_key.clone(),
            alias: paper.local.alias.clone(),
            title: paper.identity.title.clone(),
            authors: paper.identity.authors.clone(),
            year: paper.identity.year,
            doi: paper.identity.doi.clone(),
            venue: paper.identity.venue.clone(),
            tags: paper.local.tags.clone(),
            notes: paper.local.notes.clone(),
            fetched: paper.artifact.hash.is_some(),
            source_url: paper.artifact.source_url.clone(),
            hash: paper.artifact.hash.clone(),
        }
    }
}

/// Declared papers, in library order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct PaperList {
    /// Empty when the library is, or when no paper matches a filter.
    pub papers: Vec<DeclaredPaper>,
}

impl PaperList {
    pub fn new(papers: &[Paper]) -> Self {
        PaperList {
            papers: papers.iter().map(DeclaredPaper::from).collect(),
        }
    }
}

/// An exclusion as recorded in `research/excluded.nix`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Excluded {
    /// The key it's recorded under: the paper's DOI when known (so it's
    /// excluded from every provider), else its provider reference.
    pub key: String,
    pub title: String,
    pub reason: String,
    /// `screening` or `full-text`.
    pub stage: String,
    /// The paper's publication year, when known.
    pub year: Option<i32>,
    /// The protocol's exclusion criterion, when recorded.
    pub criterion: Option<String>,
    /// The screening round, when recorded.
    pub round: Option<String>,
    /// The paper's alias (e.g. the review's study ID `P18`), when it has
    /// one.
    pub alias: Option<String>,
}

impl From<&Exclusion> for Excluded {
    fn from(exclusion: &Exclusion) -> Self {
        Excluded {
            key: exclusion.key.clone(),
            title: exclusion.title.clone(),
            reason: exclusion.reason.clone(),
            stage: exclusion.stage.to_string(),
            year: exclusion.year,
            criterion: exclusion.criterion.clone(),
            round: exclusion.round.clone(),
            alias: exclusion.alias.clone(),
        }
    }
}

/// What `show` found. Exactly one field is set: `declared` for a paper in
/// the library, `excluded` for an excluded paper named by its alias,
/// `candidate` for a provider reference, `records` for a DOI looked up at
/// every provider (a client that offers that; the `pax` CLI's `show` doesn't).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Shown {
    pub declared: Option<DeclaredPaper>,
    /// The paper's exclusion record: it was excluded from the review, so it
    /// isn't in the library.
    pub excluded: Option<Excluded>,
    pub candidate: Option<CandidateDetails>,
}

impl Shown {
    /// [`crate::show_reference`]'s result; a candidate is flagged against
    /// `index`.
    pub fn new(shown: &crate::ShowResult, index: &LibraryIndex) -> Self {
        let none = Shown {
            declared: None,
            excluded: None,
            candidate: None,
        };
        match shown {
            crate::ShowResult::Declared(paper) => Shown {
                declared: Some(paper.into()),
                ..none
            },
            crate::ShowResult::Excluded(exclusion) => Shown {
                excluded: Some(exclusion.into()),
                ..none
            },
            crate::ShowResult::Candidate(work) => Shown {
                candidate: Some(CandidateDetails::new(work, index)),
                ..none
            },
        }
    }
}

/// One paper in a result list, merged across every provider that returned
/// it. The flattened fields are its representative record (the published
/// version, if any); `in_library` and `excluded` hold if any record is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct MergedHit {
    #[serde(flatten)]
    pub work: Candidate,
    /// The providers that returned this paper, without repeats.
    pub providers: Vec<String>,
    /// Every merged record's reference, representative (`id`) first; any of
    /// them works with `show` and `add`.
    pub ids: Vec<String>,
    /// The paper's abstract ([`MergedWork::abstract_text`]). Only present
    /// when abstracts were asked for; then null when no record has one.
    #[serde(rename = "abstract", skip_serializing_if = "Option::is_none")]
    pub abstract_text: Option<Option<String>>,
    /// Kept by a language filter although no record carries a language to
    /// check it against.
    pub language_unknown: bool,
}

impl MergedHit {
    /// `merged`, flagged against `index`; `language_unknown` tells whether a
    /// language filter kept a record without a language to check. With
    /// `abstracts`, it carries the group's abstract.
    pub fn new(
        merged: &MergedWork,
        index: &LibraryIndex,
        language_unknown: &dyn Fn(&CandidateWork) -> bool,
        abstracts: bool,
    ) -> Self {
        MergedHit {
            work: Candidate::flagged(
                &merged.work,
                merged.records().any(|work| index.contains(work)),
                merged.records().any(|work| index.is_excluded(work)),
            ),
            providers: merged.providers().iter().map(ToString::to_string).collect(),
            ids: merged.ids().iter().map(ToString::to_string).collect(),
            abstract_text: abstracts.then(|| merged.abstract_text().map(str::to_string)),
            language_unknown: merged.records().all(language_unknown),
        }
    }
}

/// How one provider applied one requested filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct FilterReport {
    /// `year`, `type`, or `language`.
    pub filter: String,
    /// `server-side` (the provider's `total` reflects it), `post-retrieval`
    /// (applied to the records fetched; `total` counts records before it),
    /// or `not applied` (the provider has no such data, so its results
    /// ignore the filter).
    pub applied: String,
}

/// What one provider returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ProviderReport {
    pub provider: String,
    /// Records retrieved (0 when the provider failed).
    pub retrieved: usize,
    /// Records the provider reports matching in total; null when it doesn't
    /// report one.
    pub total: Option<u64>,
    /// Whether every matching record was retrieved; false when `total` is
    /// unknown.
    pub complete: bool,
    /// The most this provider serves for one query, however far it's paged.
    pub max_retrievable: Option<u64>,
    /// Records fetched before any post-retrieval filter dropped some.
    pub scanned: u64,
    pub filters: Vec<FilterReport>,
    /// Retrieved records the language filter kept without a language to
    /// check.
    pub language_unknown: usize,
    /// Set when this provider failed; the other providers' results still
    /// stand.
    pub error: Option<String>,
}

impl ProviderReport {
    pub fn new<E: Display>(provider: ProviderId, result: &Result<SearchPage, E>) -> Self {
        match result {
            Ok(page) => ProviderReport {
                provider: provider.to_string(),
                retrieved: page.retrieved(),
                total: page.total,
                complete: page.is_complete(),
                max_retrievable: page.max_retrievable,
                scanned: page.scanned,
                filters: page
                    .filters
                    .iter()
                    .map(|filter| FilterReport {
                        filter: filter.kind.to_string(),
                        applied: filter.application.to_string(),
                    })
                    .collect(),
                language_unknown: page.language_unknown.len(),
                error: None,
            },
            Err(e) => ProviderReport {
                provider: provider.to_string(),
                retrieved: 0,
                total: None,
                complete: false,
                max_retrievable: None,
                scanned: 0,
                filters: Vec::new(),
                language_unknown: 0,
                error: Some(e.to_string()),
            },
        }
    }
}

/// Record counts before and after merging duplicates across providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct MergeCounts {
    /// Records retrieved from every provider.
    pub before: usize,
    /// Distinct papers (`results.len()`).
    pub after: usize,
    /// Records merged into another (`before - after`).
    pub duplicates: usize,
    /// Records with no DOI, arXiv id, or title and year, kept as their own
    /// rows since they can't be matched.
    pub unmatched: usize,
}

impl From<&MergeReport> for MergeCounts {
    fn from(report: &MergeReport) -> Self {
        MergeCounts {
            before: report.before,
            after: report.after,
            duplicates: report.duplicates(),
            unmatched: report.unmatched,
        }
    }
}

impl From<&SeedMergeReport> for MergeCounts {
    fn from(report: &SeedMergeReport) -> Self {
        MergeCounts {
            before: report.before,
            after: report.after,
            duplicates: report.duplicates(),
            unmatched: report.unmatched,
        }
    }
}

/// Whether a search was recorded in the library's search log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SearchLogStatus {
    pub logged: bool,
    /// The log file, when logged.
    pub path: Option<String>,
    /// Why it wasn't logged.
    pub reason: Option<String>,
}

impl From<&LogOutcome> for SearchLogStatus {
    fn from(log: &LogOutcome) -> Self {
        let not_logged = |reason: String| SearchLogStatus {
            logged: false,
            path: None,
            reason: Some(reason),
        };
        match log {
            LogOutcome::Written(path) => SearchLogStatus {
                logged: true,
                path: Some(path.display().to_string()),
                reason: None,
            },
            LogOutcome::NoLibrary => {
                not_logged("not in a pax library, so nothing was logged".to_string())
            }
            LogOutcome::Failed(e) => not_logged(format!(
                "writing the search log failed: {e}; the results still stand"
            )),
            LogOutcome::Nothing => {
                not_logged("every seed was invalid, so no provider was asked".to_string())
            }
        }
    }
}

/// Per-provider counts and one list merged across providers: a search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SearchResults {
    /// The providers asked, in [`crate::merge::MERGE_ORDER`].
    pub providers: Vec<ProviderReport>,
    pub merge: MergeCounts,
    pub results: Vec<MergedHit>,
    pub log: SearchLogStatus,
}

impl SearchResults {
    /// A search's per-provider `results`, merged and flagged against
    /// `index`, with each paper's abstract when `abstracts` were asked for.
    pub fn new<E: Display>(
        results: &HashMap<ProviderId, Result<SearchPage, E>>,
        index: &LibraryIndex,
        log: &LogOutcome,
        abstracts: bool,
    ) -> Self {
        let language_unknown = |work: &CandidateWork| {
            results
                .values()
                .flatten()
                .any(|page| page.is_language_unknown(work))
        };
        let merged = crate::merge_pages(results);
        SearchResults {
            providers: MERGE_ORDER
                .iter()
                .filter_map(|&provider| {
                    Some(ProviderReport::new(provider, results.get(&provider)?))
                })
                .collect(),
            merge: (&merged).into(),
            results: merged
                .works
                .iter()
                .map(|work| MergedHit::new(work, index, &language_unknown, abstracts))
                .collect(),
            log: log.into(),
        }
    }
}

/// What one provider returned for a DOI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct DoiLookup {
    pub provider: String,
    /// Whether the provider has a record for the DOI.
    pub found: bool,
    /// Set when the lookup failed (not when the provider just has no
    /// record); the other providers' records still stand.
    pub error: Option<String>,
}

/// Every provider's record of a DOI, merged as a search's results are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct DoiRecords {
    /// The DOI looked up, normalized.
    pub doi: String,
    /// The providers that resolve DOIs (arXiv doesn't).
    pub providers: Vec<DoiLookup>,
    /// Normally one paper; more if the providers' records don't merge.
    pub results: Vec<MergedHit>,
}

impl DoiRecords {
    /// [`crate::search_by_doi`]'s `results` for `doi`, merged and flagged
    /// against `index`.
    pub fn new(
        doi: &str,
        results: &HashMap<ProviderId, Result<Vec<CandidateWork>, ProviderError>>,
        index: &LibraryIndex,
    ) -> Self {
        let providers = MERGE_ORDER
            .iter()
            .filter_map(|provider| {
                let result = results.get(provider)?;
                Some(DoiLookup {
                    provider: provider.to_string(),
                    found: result.as_ref().is_ok_and(|works| !works.is_empty()),
                    error: match result {
                        Err(ProviderError::NotFound) | Ok(_) => None,
                        Err(e) => Some(e.to_string()),
                    },
                })
            })
            .collect();
        let merged = crate::merge_works(
            MERGE_ORDER
                .iter()
                .filter_map(|provider| results.get(provider)?.as_ref().ok())
                .flatten()
                .cloned(),
        );
        DoiRecords {
            doi: crate::normalize_doi(doi.trim()).to_string(),
            providers,
            results: merged
                .works
                .iter()
                .map(|work| MergedHit::new(work, index, &|_| false, false))
                .collect(),
        }
    }
}

/// Which lists a multi-seed lookup returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Lists {
    /// `refs` / `cited-by`: citation lists, which can be whole.
    Citations,
    /// `related`: recommendations, which have no total, so are never whole.
    Related,
}

/// One provider whose list for a seed is incomplete, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct IncompleteList {
    pub provider: String,
    /// `withheld`, `failed`, `unresolved`, or `truncated` (the limit stopped
    /// retrieval before the provider's total; the list is good, just
    /// short). Related lists report only the missing ones (`failed`,
    /// `unresolved`), since they have no total.
    pub reason: String,
}

/// One seed's part in a `refs`, `cited-by` or `related` lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SeedReport {
    /// The seed as given.
    pub seed: String,
    /// An earlier seed that is the same paper, whose lists these are.
    pub same_as: Option<String>,
    /// What each citation provider returned for this seed.
    pub providers: Vec<ProviderReport>,
    /// This seed's provider lists that are incomplete, with why.
    pub incomplete: Vec<IncompleteList>,
    /// Whether every provider returned this seed's whole list; for related
    /// works, whether no provider's list is missing.
    pub complete: bool,
    /// Set when the seed itself is invalid (a malformed reference, or a
    /// name not in the library); no provider was asked for it, and the
    /// other seeds still answer.
    pub error: Option<String>,
}

impl SeedReport {
    fn new(results: &SeedResults, seed: &crate::SeedResult, lists: Lists) -> Self {
        let same_as = seed.same_as.map(|i| results.seeds[i].seed.clone());
        match &seed.results {
            Ok(lists_by_provider) => {
                let incomplete: Vec<IncompleteList> = crate::incomplete_lists(lists_by_provider)
                    .into_iter()
                    .filter(|list| lists == Lists::Citations || list.reason.is_missing())
                    .map(|list| IncompleteList {
                        provider: list.provider.to_string(),
                        reason: list.reason.to_string(),
                    })
                    .collect();
                SeedReport {
                    seed: seed.seed.clone(),
                    same_as,
                    providers: CITATION_PROVIDERS
                        .iter()
                        .filter_map(|&provider| {
                            Some(ProviderReport::new(
                                provider,
                                lists_by_provider.get(&provider)?,
                            ))
                        })
                        .collect(),
                    complete: incomplete.is_empty(),
                    incomplete,
                    error: None,
                }
            }
            Err(e) => SeedReport {
                seed: seed.seed.clone(),
                same_as,
                providers: Vec::new(),
                incomplete: Vec::new(),
                complete: false,
                error: Some(e.to_string()),
            },
        }
    }
}

/// One paper in a list merged across seeds and providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct CitationHit {
    #[serde(flatten)]
    pub hit: MergedHit,
    /// The seeds whose lists returned this paper, as given, in seed order.
    pub seeds: Vec<String>,
}

/// Per-seed reports and one list merged across every seed and provider: a
/// `refs`, `cited-by` or `related` lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct CitationResults {
    pub seeds: Vec<SeedReport>,
    /// Whether every seed's lists are complete.
    pub complete: bool,
    pub merge: MergeCounts,
    pub results: Vec<CitationHit>,
    pub log: SearchLogStatus,
}

impl CitationResults {
    /// A multi-seed lookup's `results`, merged across seeds and providers
    /// and flagged against `index`, with each paper's abstract when
    /// `abstracts` were asked for.
    pub fn new(
        results: &SeedResults,
        index: &LibraryIndex,
        log: &LogOutcome,
        lists: Lists,
        abstracts: bool,
    ) -> Self {
        let seeds: Vec<SeedReport> = results
            .seeds
            .iter()
            .map(|seed| SeedReport::new(results, seed, lists))
            .collect();
        let language_unknown = |work: &CandidateWork| {
            results
                .seeds
                .iter()
                .filter_map(|seed| seed.results.as_ref().ok())
                .flat_map(|lists| lists.values().flatten())
                .any(|page| page.is_language_unknown(work))
        };
        let merged = results.merge();
        let hits = merged
            .works
            .iter()
            .map(|work| {
                let mut names: Vec<String> = Vec::new();
                for i in work.seeds() {
                    let name = &results.seeds[i].seed;
                    if !names.contains(name) {
                        names.push(name.clone());
                    }
                }
                CitationHit {
                    hit: MergedHit::new(&work.merged, index, &language_unknown, abstracts),
                    seeds: names,
                }
            })
            .collect();
        CitationResults {
            complete: seeds.iter().all(|seed| seed.complete),
            seeds,
            merge: (&merged).into(),
            results: hits,
            log: log.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paper::{Artifact, Identity, Local};

    fn lamport() -> Paper {
        Paper {
            identity: Identity {
                doi: Some("10.1145/359545.359563".into()),
                title: "Time, Clocks".into(),
                authors: vec!["Leslie Lamport".into()],
                year: Some(1978),
                venue: None,
            },
            artifact: Artifact {
                source_url: None,
                hash: Some("sha256-x".into()),
            },
            local: Local::new("lamport1978")
                .with_tags(vec!["core".into()])
                .with_alias(Some("P1".into())),
        }
    }

    #[test]
    fn a_declared_paper_serializes_every_field() {
        let json = serde_json::to_value(DeclaredPaper::from(&lamport())).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "citation_key": "lamport1978",
                "alias": "P1",
                "title": "Time, Clocks",
                "authors": ["Leslie Lamport"],
                "year": 1978,
                "doi": "10.1145/359545.359563",
                "venue": null,
                "tags": ["core"],
                "notes": null,
                "fetched": true,
                "source_url": null,
                "hash": "sha256-x"
            })
        );
    }

    fn empty_index() -> LibraryIndex {
        LibraryIndex::new(
            &crate::Library::new(Vec::new()),
            &crate::Exclusions::default(),
        )
    }

    fn work(id: &str, doi: &str) -> CandidateWork {
        CandidateWork {
            id: id.parse().unwrap(),
            title: "A Study".into(),
            authors: Vec::new(),
            publish_date: "2020".into(),
            doi: Some(doi.into()),
            pdf_url: None,
            venue: None,
            abstract_text: Some("We study.".into()),
        }
    }

    #[test]
    fn search_results_report_every_provider_and_merge_duplicates() {
        let results: HashMap<ProviderId, Result<SearchPage, ProviderError>> = HashMap::from([
            (
                ProviderId::OpenAlex,
                Ok(SearchPage::new(
                    vec![work("openalex:W1", "10.1/a")],
                    Some(1),
                    None,
                )),
            ),
            (
                ProviderId::Crossref,
                Ok(SearchPage::new(
                    vec![work("crossref:10.1/a", "10.1/a")],
                    Some(9),
                    None,
                )),
            ),
            (
                ProviderId::ArXiv,
                Err(ProviderError::Request("timed out".into())),
            ),
        ]);
        let search = SearchResults::new(&results, &empty_index(), &LogOutcome::NoLibrary, false);
        let json = serde_json::to_value(&search).unwrap();
        assert_eq!(json["providers"][0]["provider"], "openalex");
        assert_eq!(json["providers"][0]["complete"], true);
        assert_eq!(json["providers"][1]["complete"], false);
        assert_eq!(json["providers"][2]["error"], "request failed: timed out");
        assert_eq!(
            json["merge"],
            serde_json::json!({ "before": 2, "after": 1, "duplicates": 1, "unmatched": 0 })
        );
        assert_eq!(
            json["results"][0]["ids"],
            serde_json::json!(["openalex:W1", "crossref:10.1/a"])
        );
        assert!(json["results"][0].get("abstract").is_none());
        assert_eq!(json["log"]["logged"], false);

        let search = SearchResults::new(&results, &empty_index(), &LogOutcome::NoLibrary, true);
        let json = serde_json::to_value(&search).unwrap();
        assert_eq!(json["results"][0]["abstract"], "We study.");
    }

    #[test]
    fn a_shown_candidate_flattens_its_details() {
        let work = CandidateWork {
            id: "openalex:W1".parse().unwrap(),
            title: "A Study".into(),
            authors: Vec::new(),
            publish_date: "2020-01-01".into(),
            doi: None,
            pdf_url: Some("https://example.org/a.pdf".into()),
            venue: None,
            abstract_text: Some("We study.".into()),
        };
        let index = LibraryIndex::new(
            &crate::Library::new(Vec::new()),
            &crate::Exclusions::default(),
        );
        let shown = Shown::new(&crate::ShowResult::Candidate(work), &index);
        let json = serde_json::to_value(shown).unwrap();
        assert_eq!(json["declared"], serde_json::Value::Null);
        assert_eq!(json["excluded"], serde_json::Value::Null);
        assert_eq!(json["candidate"]["id"], "openalex:W1");
        assert_eq!(json["candidate"]["pdf_available"], true);
        assert_eq!(json["candidate"]["pdf_url"], "https://example.org/a.pdf");
        assert_eq!(json["candidate"]["abstract"], "We study.");
        assert_eq!(json["candidate"]["in_library"], false);
    }
}
