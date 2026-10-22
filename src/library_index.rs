//! What a library already knows about a result: whether the paper is
//! declared in `papers.nix`, and whether it's excluded in `excluded.nix`.
//!
//! Both checks match the paper, not one record of it: a result counts when
//! any of its DOI or arXiv keys (see [`crate::identity`]) matches, so every
//! arXiv version of a paper, and the same paper from any provider, gets the
//! same answer. Title+year keys aren't used: on their own they don't prove
//! a result is the declared or excluded paper.

use std::collections::HashSet;
use std::path::Path;

use crate::exclusions::{Exclusions, candidate_exclusion_key, exclusion_key};
use crate::identity::{self, IdentityKey};
use crate::library::Library;
use crate::nix;
use crate::provider::{CandidateId, CandidateWork};

/// The identity keys of a library's declared and excluded papers, built once
/// per command and asked about each result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct LibraryIndex {
    /// DOI and arXiv keys of every paper in `papers.nix`.
    declared: HashSet<IdentityKey>,
    /// DOI and arXiv keys of every entry in `excluded.nix`.
    excluded: HashSet<IdentityKey>,
    /// The raw exclusion keys, for entries keyed by an opaque `provider:id`
    /// (e.g. `openalex:W…`) that carries no DOI or arXiv id.
    excluded_ids: HashSet<String>,
}

impl LibraryIndex {
    /// Indexes an already-loaded library and its exclusions.
    pub fn new(library: &Library, exclusions: &Exclusions) -> Self {
        let declared = library
            .papers()
            .iter()
            .flat_map(identity::paper_id_keys)
            .collect();
        let excluded = exclusions
            .entries()
            .iter()
            .flat_map(|entry| exclusion_id_keys(&entry.key))
            .collect();
        let excluded_ids = exclusions
            .entries()
            .iter()
            .map(|entry| entry.key.clone())
            .collect();
        LibraryIndex {
            declared,
            excluded,
            excluded_ids,
        }
    }

    /// Loads the library at `root` (`research/papers.nix` and
    /// `research/excluded.nix`). A file that's missing or can't be read
    /// counts as empty rather than an error, so a search still works before
    /// `init`, just with nothing flagged. Use [`LibraryIndex::new`] to handle
    /// load errors yourself.
    pub fn load(root: &Path) -> Self {
        let library =
            Library::load(&nix::papers_path(root)).unwrap_or_else(|_| Library::new(Vec::new()));
        let exclusions = Exclusions::load(&nix::excluded_path(root)).unwrap_or_default();
        Self::new(&library, &exclusions)
    }

    /// Whether `work` is a paper declared in the library: it shares a DOI or
    /// a version-less arXiv id with one (a declared paper's arXiv id comes
    /// from an arXiv DOI or an arxiv.org `source_url`). A paper added as a
    /// preprint so matches its published version when that carries the
    /// arXiv id, and vice versa.
    pub fn contains(&self, work: &CandidateWork) -> bool {
        identity::work_id_keys(work)
            .iter()
            .any(|key| self.declared.contains(key))
    }

    /// Whether `work` is an excluded paper: it shares a DOI or a
    /// version-less arXiv id with an entry in `excluded.nix`, or its
    /// exclusion key ([`exclusion_key`], [`candidate_exclusion_key`]) is an
    /// entry's key. Excluding any version of an arXiv paper excludes all of
    /// them, and excluding a DOI excludes the paper from every provider.
    pub fn is_excluded(&self, work: &CandidateWork) -> bool {
        identity::work_id_keys(work)
            .iter()
            .any(|key| self.excluded.contains(key))
            || self.excluded_ids.contains(&exclusion_key(work))
            || self
                .excluded_ids
                .contains(&candidate_exclusion_key(&work.id))
    }
}

/// The DOI and arXiv keys an exclusion key names: a `provider:id` key's own
/// (an arXiv id, or a Crossref DOI), else the key read as a DOI. Empty for
/// another provider's opaque id.
fn exclusion_id_keys(key: &str) -> Vec<IdentityKey> {
    match key.parse::<CandidateId>() {
        Ok(id) => identity::reference_id_keys(&id),
        Err(_) => identity::doi_id_keys(key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exclusions::{Exclusion, ExclusionStage};
    use crate::paper::{Artifact, Identity, Local, Paper};
    use crate::provider::ProviderId;

    fn work(provider: ProviderId, native_id: &str, doi: Option<&str>) -> CandidateWork {
        CandidateWork {
            id: CandidateId {
                provider,
                native_id: native_id.to_string(),
            },
            title: "Attention Is All You Need".to_string(),
            authors: Vec::new(),
            publish_date: "2017-06-12".to_string(),
            doi: doi.map(str::to_string),
            pdf_url: None,
            venue: None,
            abstract_text: None,
        }
    }

    fn excluding(keys: &[&str]) -> LibraryIndex {
        let mut exclusions = Exclusions::default();
        for key in keys {
            exclusions.upsert(Exclusion::new(
                key.to_string(),
                "T".to_string(),
                "R".to_string(),
                ExclusionStage::Screening,
            ));
        }
        LibraryIndex::new(&Library::new(Vec::new()), &exclusions)
    }

    fn declaring(doi: Option<&str>, source_url: Option<&str>) -> LibraryIndex {
        let paper = Paper {
            identity: Identity {
                doi: doi.map(str::to_string),
                title: "Attention Is All You Need".to_string(),
                ..Identity::default()
            },
            artifact: Artifact {
                source_url: source_url.map(str::to_string),
                hash: None,
            },
            local: Local {
                citation_key: "vaswani2017".to_string(),
                ..Local::default()
            },
        };
        LibraryIndex::new(&Library::new(vec![paper]), &Exclusions::default())
    }

    #[test]
    fn an_excluded_arxiv_v1_flags_its_v3() {
        let index = excluding(&["arxiv:1706.03762"]);
        assert!(index.is_excluded(&work(ProviderId::ArXiv, "1706.03762v3", None)));
        assert!(!index.is_excluded(&work(ProviderId::ArXiv, "1706.03763v1", None)));
    }

    #[test]
    fn an_excluded_arxiv_paper_is_flagged_from_another_provider_by_its_arxiv_doi() {
        let index = excluding(&["arxiv:1706.03762"]);
        assert!(index.is_excluded(&work(
            ProviderId::OpenAlex,
            "W2963403868",
            Some("https://doi.org/10.48550/arXiv.1706.03762"),
        )));
    }

    #[test]
    fn excluded_by_doi_flags_the_same_paper_from_another_provider() {
        let index = excluding(&["10.1145/3474085.3475385"]);
        assert!(index.is_excluded(&work(
            ProviderId::SemanticScholar,
            "abc123",
            Some("10.1145/3474085.3475385"),
        )));
        assert!(index.is_excluded(&work(
            ProviderId::OpenAlex,
            "W1",
            Some("https://doi.org/10.1145/3474085.3475385"),
        )));
        assert!(!index.is_excluded(&work(ProviderId::OpenAlex, "W2", Some("10.1145/other"))));
    }

    #[test]
    fn an_opaque_provider_id_exclusion_flags_that_record() {
        let index = excluding(&["openalex:W123"]);
        assert!(index.is_excluded(&work(ProviderId::OpenAlex, "W123", None)));
        assert!(!index.is_excluded(&work(ProviderId::OpenAlex, "W124", None)));
    }

    #[test]
    fn a_paper_added_as_a_preprint_is_in_the_library_as_its_published_version() {
        let index = declaring(
            Some("10.48550/arXiv.1706.03762"),
            Some("https://arxiv.org/pdf/1706.03762v1"),
        );
        let mut published = work(ProviderId::OpenAlex, "W1", Some("10.5555/3295222.3295349"));
        published.venue = Some("NeurIPS".to_string());
        published.pdf_url = Some("https://arxiv.org/pdf/1706.03762v7".to_string());
        assert!(index.contains(&published));
        assert!(index.contains(&work(ProviderId::ArXiv, "1706.03762v5", None)));
    }

    #[test]
    fn a_paper_added_from_an_arxiv_source_without_a_doi_matches_by_arxiv_id() {
        let index = declaring(None, Some("https://arxiv.org/abs/1706.03762"));
        assert!(index.contains(&work(
            ProviderId::SemanticScholar,
            "s1",
            Some("10.48550/arXiv.1706.03762"),
        )));
    }

    #[test]
    fn in_library_still_matches_by_doi() {
        let index = declaring(Some("https://doi.org/10.1145/ABC"), None);
        assert!(index.contains(&work(
            ProviderId::Crossref,
            "10.1145/abc",
            Some("10.1145/abc")
        )));
        assert!(!index.contains(&work(ProviderId::Crossref, "10.1145/x", Some("10.1145/x"))));
    }

    #[test]
    fn a_title_and_year_alone_match_neither_flag() {
        let index = declaring(Some("10.1/a"), None);
        assert!(!index.contains(&work(ProviderId::OpenAlex, "W1", None)));
    }

    #[test]
    fn load_reads_papers_nix_and_excluded_nix() {
        let root = std::env::temp_dir().join(format!("pax-index-load-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("research")).unwrap();
        std::fs::write(
            nix::papers_path(&root),
            r#"{
  vaswani2017 = {
    doi = null;
    title = "Attention Is All You Need";
    authors = [ ];
    year = 2017;
    source_url = "https://arxiv.org/pdf/1706.03762v7";
    hash = null;
    tags = [ ];
    notes = null;
  };
}
"#,
        )
        .unwrap();
        std::fs::write(
            nix::excluded_path(&root),
            r#"{ "10.1145/abc" = { title = "T"; reason = "R"; stage = "screening"; }; }"#,
        )
        .unwrap();

        let index = LibraryIndex::load(&root);
        assert!(index.contains(&work(ProviderId::ArXiv, "1706.03762v2", None)));
        assert!(index.is_excluded(&work(ProviderId::OpenAlex, "W1", Some("10.1145/ABC"))));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_library_indexes_nothing() {
        let root = std::env::temp_dir().join(format!("pax-index-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(LibraryIndex::load(&root), LibraryIndex::default());
    }
}
