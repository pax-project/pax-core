//! `pax-core`: the paper-discovery, declaration, and library domain logic for
//! PAX. This is the primary artifact of this repository — any CLI or other
//! adapter (including the `pax` binary in `src/bin/pax/`, or a future
//! `lazypax` TUI) is a client of this library, not the other way around.

pub mod bibtex;
mod citation_key;
pub mod error;
pub mod github;
pub mod library;
pub mod nix;
pub mod paper;
mod process;
pub mod provider;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub use error::PaxError;
pub use github::publish_to_github_release;
pub use library::Library;
pub use nix::init_library;
use paper::year_from_publish_date;
pub use paper::{Artifact, Identity, Local, Paper, PaperRef};
pub use provider::{
    ArxivProvider, CandidateId, CandidateWork, CrossrefProvider, OpenAlexProvider, Provider,
    ProviderError, ProviderId, SemanticScholarProvider,
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

/// Searches every configured provider and aggregates results by provider.
/// A failing provider doesn't fail the whole search — its error is reported
/// alongside whatever providers did succeed, so callers (e.g. the CLI) can
/// decide how to surface it.
pub async fn search_all(
    query: &str,
    config: &Config,
) -> HashMap<ProviderId, Result<Vec<CandidateWork>, ProviderError>> {
    let mut results = HashMap::new();

    let openalex = OpenAlexProvider::new();
    results.insert(ProviderId::OpenAlex, openalex.search(query).await);

    match CrossrefProvider::new() {
        Ok(crossref) => {
            results.insert(ProviderId::Crossref, crossref.search(query).await);
        }
        Err(e) => {
            results.insert(ProviderId::Crossref, Err(e));
        }
    }

    match SemanticScholarProvider::new(config.semantic_scholar_api_key.as_deref()) {
        Ok(semantic_scholar) => {
            results.insert(
                ProviderId::SemanticScholar,
                semantic_scholar.search(query).await,
            );
        }
        Err(e) => {
            results.insert(ProviderId::SemanticScholar, Err(e));
        }
    }

    match ArxivProvider::new(config.arxiv_contact.as_deref()) {
        Ok(arxiv) => {
            results.insert(ProviderId::ArXiv, arxiv.search(query).await);
        }
        Err(e) => {
            results.insert(ProviderId::ArXiv, Err(e));
        }
    }

    results
}

/// Same shape as [`search_all`], but scoped to an author name — each provider's
/// `search_by_author` decides whether that's a true field-scoped query or (for a
/// provider with no such endpoint) a fallback to plain full-text search.
pub async fn search_by_author(
    author: &str,
    config: &Config,
) -> HashMap<ProviderId, Result<Vec<CandidateWork>, ProviderError>> {
    let mut results = HashMap::new();

    let openalex = OpenAlexProvider::new();
    results.insert(
        ProviderId::OpenAlex,
        openalex.search_by_author(author).await,
    );

    match CrossrefProvider::new() {
        Ok(crossref) => {
            results.insert(
                ProviderId::Crossref,
                crossref.search_by_author(author).await,
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
                semantic_scholar.search_by_author(author).await,
            );
        }
        Err(e) => {
            results.insert(ProviderId::SemanticScholar, Err(e));
        }
    }

    match ArxivProvider::new(config.arxiv_contact.as_deref()) {
        Ok(arxiv) => {
            results.insert(ProviderId::ArXiv, arxiv.search_by_author(author).await);
        }
        Err(e) => {
            results.insert(ProviderId::ArXiv, Err(e));
        }
    }

    results
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
    }
}

/// Resolves a candidate and declares it in the local library at `root`
/// (`research/papers.nix`) — metadata only. `Artifact.source_url`/`hash`
/// stay unset; fetching bytes and computing a Nix hash is `fetch`'s job,
/// not `add`'s (lazy materialization, see pax-project/.github's mvp.md).
pub async fn add_candidate(
    id: &CandidateId,
    root: &Path,
    config: &Config,
) -> Result<PaperRef, PaxError> {
    let work = resolve_candidate(id, config).await?;
    let path = nix::papers_path(root);
    let mut library = Library::load(&path)?;

    let existing_keys: Vec<&str> = library
        .papers()
        .iter()
        .map(|p| p.local.citation_key.as_str())
        .collect();
    let citation_key = citation_key::generate(&work, &existing_keys);

    library.insert(Paper {
        identity: Identity {
            doi: work.doi.clone(),
            title: work.title.clone(),
            authors: work.authors.clone(),
            year: year_from_publish_date(&work.publish_date),
            venue: work.venue.clone(),
        },
        artifact: Artifact {
            source_url: work.pdf_url.clone(),
            hash: None,
        },
        local: Local {
            citation_key: citation_key.clone(),
            tags: Vec::new(),
            notes: None,
        },
    });
    library.save(&path)?;

    Ok(PaperRef(citation_key))
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

#[cfg(test)]
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

/// Searches the local library without querying any provider (pax-project/.github's mvp.md §2.8).
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

/// Filter criteria for `pax list` (pax-project/.github's mvp.md §2.7). All fields combine with AND;
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

/// The result of resolving a `show` reference: either a paper already
/// declared in the local library, or an unresolved candidate from a
/// provider.
pub enum ShowResult {
    Declared(Paper),
    Candidate(CandidateWork),
}

/// Resolves a `show` reference. A `CandidateId` always contains `:`
/// (`provider:native_id`); a citation key, by construction of
/// `citation_key::generate`, never does — so the colon's presence decides
/// which address space `reference` belongs to before either lookup is
/// attempted, rather than trying one and falling back to the other.
pub async fn show_reference(
    reference: &str,
    root: &Path,
    config: &Config,
) -> Result<ShowResult, PaxError> {
    if !reference.contains(':') {
        let path = nix::papers_path(root);
        if let Ok(library) = Library::load(&path)
            && let Some(paper) = library.find(reference)
        {
            return Ok(ShowResult::Declared(paper.clone()));
        }
        return Err(PaxError::NotFound(reference.to_string()));
    }

    let id: CandidateId = reference.parse()?;
    let work = resolve_candidate(&id, config).await?;
    Ok(ShowResult::Candidate(work))
}

/// Removes a declared paper from the local library at `root`. Only the
/// declaration/metadata is removed — Nix remains responsible for garbage
/// collecting any unused artifacts on its own (see pax-project/.github's mvp.md).
pub fn remove_paper(citation_key: &str, root: &Path) -> Result<(), PaxError> {
    let path = nix::papers_path(root);
    let mut library = Library::load(&path)?;
    if !library.remove(citation_key) {
        return Err(PaxError::NoSuchPaper(citation_key.to_string()));
    }
    library.save(&path)?;
    Ok(())
}

/// What to change about a declared paper, passed to [`edit_paper`]. Every
/// field is opt-in — only a field that's `Some`/non-empty gets touched,
/// matching pax-project/.github's mvp.md §2.10: "the original external metadata should not be
/// silently overwritten without user intent."
#[derive(Debug, Clone, Default)]
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
}

impl PaperEdits {
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
    }
}

/// Edits a declared paper: tags, notes, citation-key rename, and Identity
/// corrections (title/authors/year/doi). A rename is checked for validity
/// (`library::is_valid_citation_key` — citation keys are unquoted Nix
/// identifiers, so an invalid one would corrupt `papers.nix`) and for
/// collision against every other declared paper before anything is written.
pub fn edit_paper(citation_key: &str, root: &Path, edits: &PaperEdits) -> Result<(), PaxError> {
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

    if let Some(new_key) = &edits.rename
        && new_key != citation_key
        && library.find(new_key).is_some()
    {
        return Err(PaxError::CitationKeyExists(new_key.clone()));
    }

    let paper = library
        .find_mut(citation_key)
        .ok_or_else(|| PaxError::NoSuchPaper(citation_key.to_string()))?;
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

/// The result of `fetch_paper`: whether an artifact was newly materialized or
/// was already fetched (and so didn't need a network call).
pub enum FetchOutcome {
    AlreadyFetched { hash: String },
    Fetched { hash: String },
}

/// Materializes a declared paper's artifact through Nix and records its
/// content hash. Idempotent: a paper that already has a hash is reported as
/// already fetched rather than re-hitting the network — verifying a *stale*
/// hash is `check`'s job, not `fetch`'s (see pax-project/.github's mvp.md §2.6).
pub fn fetch_paper(citation_key: &str, root: &Path) -> Result<FetchOutcome, PaxError> {
    let path = nix::papers_path(root);
    let mut library = Library::load(&path)?;
    let paper = library
        .find_mut(citation_key)
        .ok_or_else(|| PaxError::NoSuchPaper(citation_key.to_string()))?;

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
pub fn resolve_artifact_path(
    citation_key: &str,
    root: &Path,
) -> Result<ResolvedArtifact, PaxError> {
    let path = nix::papers_path(root);
    let library = Library::load(&path)?;
    let paper = library
        .find(citation_key)
        .ok_or_else(|| PaxError::NoSuchPaper(citation_key.to_string()))?;

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
