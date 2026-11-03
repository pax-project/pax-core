//! Serializable result shapes: what a command returns, as one JSON
//! document. The `pax` CLI prints them with `--json`, and any other client
//! (e.g. an MCP server) can return the same shapes, so every client's
//! structured output agrees field for field.
//!
//! Each shape is built from the library's own results with its `new` (or a
//! `From` impl); none is meant to be built field by field, so every one is
//! non-exhaustive and fields can be added in a minor release. With the
//! `schemars` feature, each also derives `schemars::JsonSchema`.

use serde::Serialize;

use crate::exclusions::Exclusion;
use crate::library_index::LibraryIndex;
use crate::paper::Paper;
use crate::provider::CandidateWork;

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
