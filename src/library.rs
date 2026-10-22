//! Reads and writes `research/papers.nix`.
//!
//! `papers.nix` is Nix syntax, but it's a schema PAX itself invented and
//! fully controls (a flat attrset of `citation_key = { ...fields... };`
//! records — see `templates/papers.nix.template`). `research/flake.nix`
//! only ever reads whichever fields it needs (`url`, `hash`) via
//! `builtins.mapAttrs`, so extra fields are harmless to it. That means this
//! module only needs a reader/writer for one fixed, PAX-defined shape — not
//! a general Nix-language parser.
//!
//! The generic attrset tokenizer/parser and the field writers are shared
//! with `research/excluded.nix` and live in `nix_attrs`; this module only
//! maps entries onto [`Paper`]s and back.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::error::PaxError;
use crate::nix_attrs::{
    self, Value, take_int_opt, take_string, take_string_list, take_string_opt, write_field_int_opt,
    write_field_string, write_field_string_list, write_field_string_opt,
};
use crate::paper::{Artifact, Identity, Local, Paper};

#[derive(Debug, Clone, PartialEq)]
pub struct Library {
    papers: Vec<Paper>,
}

impl Library {
    pub fn new(papers: Vec<Paper>) -> Self {
        Library { papers }
    }

    pub fn papers(&self) -> &[Paper] {
        &self.papers
    }

    pub fn insert(&mut self, paper: Paper) {
        self.papers.push(paper);
    }

    pub fn find(&self, citation_key: &str) -> Option<&Paper> {
        self.papers
            .iter()
            .find(|p| p.local.citation_key == citation_key)
    }

    pub fn find_mut(&mut self, citation_key: &str) -> Option<&mut Paper> {
        self.papers
            .iter_mut()
            .find(|p| p.local.citation_key == citation_key)
    }

    pub fn remove(&mut self, citation_key: &str) -> bool {
        let len_before = self.papers.len();
        self.papers.retain(|p| p.local.citation_key != citation_key);
        self.papers.len() != len_before
    }

    pub fn load(path: &Path) -> Result<Self, PaxError> {
        let text = std::fs::read_to_string(path)?;
        let papers = nix_attrs::parse_entries(&text)?
            .into_iter()
            .map(|(citation_key, fields)| paper_from_fields(citation_key, fields))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Library { papers })
    }

    /// Written atomically (see [`nix_attrs::write_atomically`]), so a
    /// concurrent reader — another `pax`/`lazypax` process, or `nix build`
    /// evaluating `papers.nix` mid-save — never sees a torn file.
    pub fn save(&self, path: &Path) -> Result<(), PaxError> {
        let mut papers = self.papers.clone();
        papers.sort_by(|a, b| a.local.citation_key.cmp(&b.local.citation_key));

        let mut out = String::from("{\n");
        for paper in &papers {
            write_entry(&mut out, paper);
        }
        out.push_str("}\n");
        nix_attrs::write_atomically(path, &out)
    }
}

/// Whether `key` is a valid citation key per this file's own tokenizer grammar
/// — `(alpha|_)(alnum|_|-)*`. Citation keys are written as bare Nix attribute
/// names (see `write_entry`), not quoted strings, so an invalid one would
/// produce a `papers.nix` this same file can no longer parse back.
pub(crate) fn is_valid_citation_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

fn write_entry(out: &mut String, paper: &Paper) {
    let _ = writeln!(out, "  {} = {{", paper.local.citation_key);
    write_field_string_opt(out, "doi", paper.identity.doi.as_deref());
    write_field_string(out, "title", &paper.identity.title);
    write_field_string_list(out, "authors", &paper.identity.authors);
    write_field_int_opt(out, "year", paper.identity.year);
    write_field_string_opt(out, "venue", paper.identity.venue.as_deref());
    write_field_string_opt(out, "source_url", paper.artifact.source_url.as_deref());
    write_field_string_opt(out, "hash", paper.artifact.hash.as_deref());
    write_field_string_list(out, "tags", &paper.local.tags);
    write_field_string_opt(out, "notes", paper.local.notes.as_deref());
    out.push_str("  };\n");
}

fn paper_from_fields(
    citation_key: String,
    mut fields: HashMap<String, Value>,
) -> Result<Paper, PaxError> {
    // Keys may be quoted in the file, but a citation key must still be a
    // valid one: it is written back bare (see `write_entry`).
    if !is_valid_citation_key(&citation_key) {
        return Err(PaxError::LibraryParse(format!(
            "invalid citation key {citation_key:?}"
        )));
    }
    let doi = take_string_opt(&mut fields, "doi")?;
    let title = take_string(&mut fields, "title")?;
    let authors = take_string_list(&mut fields, "authors")?;
    let year = take_int_opt(&mut fields, "year")?;
    let venue = take_string_opt(&mut fields, "venue")?;
    let source_url = take_string_opt(&mut fields, "source_url")?;
    let hash = take_string_opt(&mut fields, "hash")?;
    let tags = take_string_list(&mut fields, "tags")?;
    let notes = take_string_opt(&mut fields, "notes")?;

    if let Some(unknown) = fields.keys().next() {
        return Err(PaxError::LibraryParse(format!(
            "unrecognized field {unknown:?} in entry {citation_key:?}"
        )));
    }

    Ok(Paper {
        identity: Identity {
            doi,
            title,
            authors,
            year,
            venue,
        },
        artifact: Artifact { source_url, hash },
        local: Local {
            citation_key,
            tags,
            notes,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_papers() -> Vec<Paper> {
        vec![
            Paper {
                identity: Identity {
                    doi: Some("10.1112/plms/s2-42.1.230".to_string()),
                    title: "On Computable Numbers".to_string(),
                    authors: vec!["Alan M. Turing".to_string()],
                    year: Some(1936),
                    venue: Some("Proceedings of the London Mathematical Society".to_string()),
                },
                artifact: Artifact {
                    source_url: Some("https://example.org/turing1936.pdf".to_string()),
                    hash: Some("sha256-abc123".to_string()),
                },
                local: Local {
                    citation_key: "turing1936".to_string(),
                    tags: vec!["computability".to_string(), "logic".to_string()],
                    notes: Some("foundational".to_string()),
                },
            },
            Paper {
                identity: Identity {
                    doi: None,
                    title: "A paper with no DOI or notes".to_string(),
                    authors: vec![],
                    year: None,
                    venue: None,
                },
                artifact: Artifact {
                    source_url: None,
                    hash: None,
                },
                local: Local {
                    citation_key: "anon2020".to_string(),
                    tags: vec![],
                    notes: None,
                },
            },
        ]
    }

    #[test]
    fn is_valid_citation_key_accepts_tokenizer_identifiers() {
        assert!(is_valid_citation_key("turing1936"));
        assert!(is_valid_citation_key("_foo"));
        assert!(is_valid_citation_key("a-b_c"));
    }

    #[test]
    fn is_valid_citation_key_rejects_non_identifiers() {
        assert!(!is_valid_citation_key(""));
        assert!(!is_valid_citation_key("1abc"));
        assert!(!is_valid_citation_key("has space"));
        assert!(!is_valid_citation_key("\"quoted\""));
    }

    #[test]
    fn save_never_leaves_a_reader_seeing_a_truncated_file() {
        // Regression guard for the torn-read class of bug: a concurrent
        // `Library::load` running at any point during `save` must see
        // either the pre-save content in full or the post-save content in
        // full, never a partial/empty file. `save` itself is synchronous
        // (no real concurrency to interleave here), so this instead checks
        // the property that makes that guarantee hold: no temp file is
        // left behind, and the final path always parses back correctly
        // immediately after `save` returns — i.e. the swap really is a
        // single atomic step, not two observable ones.
        let dir =
            std::env::temp_dir().join(format!("pax-library-test-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("papers.nix");

        Library::new(sample_papers()).save(&path).unwrap();
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            entries,
            vec![std::ffi::OsString::from("papers.nix")],
            "no stray temp file left behind"
        );
        assert_eq!(Library::load(&path).unwrap().papers().len(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn library_round_trips_through_papers_nix() {
        let dir = std::env::temp_dir().join(format!("pax-library-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("papers.nix");

        let mut expected = sample_papers();
        expected.sort_by(|a, b| a.local.citation_key.cmp(&b.local.citation_key));
        let library = Library::new(expected.clone());

        library.save(&path).unwrap();
        let loaded = Library::load(&path).unwrap();

        assert_eq!(loaded.papers(), expected.as_slice());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_returns_matching_paper_or_none() {
        let library = Library::new(sample_papers());

        assert_eq!(
            library
                .find("turing1936")
                .map(|p| p.identity.title.as_str()),
            Some("On Computable Numbers")
        );
        assert!(library.find("nonexistent-key").is_none());
    }

    #[test]
    fn find_mut_allows_in_place_mutation() {
        let mut library = Library::new(sample_papers());

        library.find_mut("turing1936").unwrap().local.notes = Some("edited".to_string());
        assert_eq!(
            library.find("turing1936").unwrap().local.notes,
            Some("edited".to_string())
        );
        assert!(library.find_mut("nonexistent-key").is_none());
    }

    #[test]
    fn remove_deletes_matching_paper_and_reports_whether_found() {
        let mut library = Library::new(sample_papers());

        assert!(!library.remove("nonexistent-key"));
        assert_eq!(library.papers().len(), 2);

        assert!(library.remove("turing1936"));
        assert_eq!(library.papers().len(), 1);
        assert!(library.find("turing1936").is_none());
    }

    #[test]
    fn empty_library_round_trips() {
        let dir =
            std::env::temp_dir().join(format!("pax-library-test-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("papers.nix");

        Library::new(vec![]).save(&path).unwrap();
        let loaded = Library::load(&path).unwrap();
        assert!(loaded.papers().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    fn load_text(name: &str, text: &str) -> Result<Library, PaxError> {
        let dir =
            std::env::temp_dir().join(format!("pax-library-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("papers.nix");
        std::fs::write(&path, text).unwrap();
        let loaded = Library::load(&path);
        std::fs::remove_dir_all(&dir).ok();
        loaded
    }

    #[test]
    fn a_quoted_valid_citation_key_loads() {
        let library = load_text("quoted-key", r#"{ "turing1936" = { title = "T"; }; }"#).unwrap();
        assert!(library.find("turing1936").is_some());
    }

    #[test]
    fn a_quoted_invalid_citation_key_is_rejected() {
        assert!(matches!(
            load_text("bad-key", r#"{ "10.1145/x" = { title = "T"; }; }"#),
            Err(PaxError::LibraryParse(_))
        ));
    }
}
