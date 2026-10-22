//! Reads and writes `research/excluded.nix`: the papers a library has
//! decided against, each with the reason and the screening stage it was
//! excluded at.
//!
//! It's a separate file from `papers.nix`, so `papers.nix` and
//! `research/flake.nix` are untouched: the flake never imports it, and a
//! library without the file simply has no exclusions. Same attrset subset
//! as `papers.nix` (see `nix_attrs`), keyed by an exclusion key — a
//! normalized DOI, or `provider:id` for a work without one:
//!
//! ```nix
//! {
//!   "10.1145/123456" = { title = "Some paper"; reason = "not empirical"; stage = "full-text"; };
//!   "arxiv:2301.01234" = { title = "..."; reason = "..."; stage = "screening"; };
//! }
//! ```

use std::collections::HashMap;
use std::fmt;
use std::fmt::Write as _;
use std::path::Path;
use std::str::FromStr;

use crate::error::PaxError;
use crate::nix_attrs::{self, Value, take_string, write_field_string};
use crate::provider::CandidateWork;

/// When in the screening process a paper was excluded. PRISMA 2020 reports
/// the two in separate boxes: title/abstract screening, and full-text
/// eligibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ExclusionStage {
    /// Excluded from its title and abstract.
    Screening,
    /// Excluded after reading the full text.
    FullText,
}

impl ExclusionStage {
    fn as_str(self) -> &'static str {
        match self {
            ExclusionStage::Screening => "screening",
            ExclusionStage::FullText => "full-text",
        }
    }
}

impl fmt::Display for ExclusionStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownExclusionStage(pub String);

impl fmt::Display for UnknownExclusionStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown exclusion stage {:?} (expected \"screening\" or \"full-text\")",
            self.0
        )
    }
}

impl std::error::Error for UnknownExclusionStage {}

impl FromStr for ExclusionStage {
    type Err = UnknownExclusionStage;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "screening" => Ok(ExclusionStage::Screening),
            "full-text" => Ok(ExclusionStage::FullText),
            other => Err(UnknownExclusionStage(other.to_string())),
        }
    }
}

/// One excluded paper.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Exclusion {
    /// The exclusion key: see [`exclusion_key`].
    pub key: String,
    pub title: String,
    pub reason: String,
    pub stage: ExclusionStage,
}

impl Exclusion {
    pub fn new(key: String, title: String, reason: String, stage: ExclusionStage) -> Self {
        Exclusion {
            key,
            title,
            reason,
            stage,
        }
    }
}

/// The contents of `research/excluded.nix`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Exclusions {
    entries: Vec<Exclusion>,
}

impl Exclusions {
    pub fn entries(&self) -> &[Exclusion] {
        &self.entries
    }

    pub fn get(&self, key: &str) -> Option<&Exclusion> {
        self.entries.iter().find(|e| e.key == key)
    }

    /// Adds `exclusion`, replacing any existing entry with the same key —
    /// re-excluding a paper updates its reason and stage.
    pub fn upsert(&mut self, exclusion: Exclusion) {
        match self.entries.iter_mut().find(|e| e.key == exclusion.key) {
            Some(existing) => *existing = exclusion,
            None => self.entries.push(exclusion),
        }
    }

    /// Loads `path`, or returns no exclusions if the file doesn't exist.
    pub fn load(path: &Path) -> Result<Self, PaxError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        let entries = nix_attrs::parse_entries(&text)?
            .into_iter()
            .map(|(key, fields)| exclusion_from_fields(key, fields))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Exclusions { entries })
    }

    /// Written atomically, sorted by key so diffs stay small.
    pub fn save(&self, path: &Path) -> Result<(), PaxError> {
        let mut entries = self.entries.clone();
        entries.sort_by(|a, b| a.key.cmp(&b.key));

        let mut out = String::from("{\n");
        for entry in &entries {
            let _ = writeln!(out, "  {} = {{", nix_attrs::format_key(&entry.key));
            write_field_string(&mut out, "title", &entry.title);
            write_field_string(&mut out, "reason", &entry.reason);
            write_field_string(&mut out, "stage", entry.stage.as_str());
            out.push_str("  };\n");
        }
        out.push_str("}\n");
        nix_attrs::write_atomically(path, &out)
    }
}

fn exclusion_from_fields(
    key: String,
    mut fields: HashMap<String, Value>,
) -> Result<Exclusion, PaxError> {
    let title = take_string(&mut fields, "title")?;
    let reason = take_string(&mut fields, "reason")?;
    let stage =
        take_string(&mut fields, "stage")?
            .parse()
            .map_err(|e: UnknownExclusionStage| {
                PaxError::LibraryParse(format!("{e} in excluded entry {key:?}"))
            })?;

    if let Some(unknown) = fields.keys().next() {
        return Err(PaxError::LibraryParse(format!(
            "unrecognized field {unknown:?} in excluded entry {key:?}"
        )));
    }

    Ok(Exclusion {
        key,
        title,
        reason,
        stage,
    })
}

/// The key a DOI is excluded under: the DOI without a `https://doi.org/`
/// prefix, lowercased (DOIs are case-insensitive).
pub fn doi_exclusion_key(doi: &str) -> String {
    crate::normalize_doi(doi.trim()).to_ascii_lowercase()
}

/// The key `work` is excluded under, and the one to look up when flagging
/// it: its DOI when it has one, so an exclusion matches the same paper from
/// any provider; otherwise its `provider:id`.
pub fn exclusion_key(work: &CandidateWork) -> String {
    match work.doi.as_deref() {
        Some(doi) if !doi.trim().is_empty() => doi_exclusion_key(doi),
        _ => work.id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_path(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pax-exclusions-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("excluded.nix")
    }

    fn sample() -> Exclusions {
        let mut exclusions = Exclusions::default();
        exclusions.upsert(Exclusion::new(
            "10.1145/123456".to_string(),
            "A \"quoted\" title".to_string(),
            "not empirical".to_string(),
            ExclusionStage::FullText,
        ));
        exclusions.upsert(Exclusion::new(
            "arxiv:2301.01234".to_string(),
            "Some preprint".to_string(),
            "wrong domain".to_string(),
            ExclusionStage::Screening,
        ));
        exclusions
    }

    #[test]
    fn exclusions_round_trip_through_excluded_nix() {
        let path = scratch_path("round-trip");
        let original = sample();
        original.save(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#""10.1145/123456" = {"#));
        assert!(text.contains(r#"stage = "full-text";"#));

        let loaded = Exclusions::load(&path).unwrap();
        // Saved sorted by key, which is also `sample`'s insertion order.
        assert_eq!(loaded, original);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_missing_file_means_no_exclusions() {
        let path = scratch_path("missing");
        assert!(Exclusions::load(&path).unwrap().entries().is_empty());
    }

    #[test]
    fn upsert_replaces_an_existing_key() {
        let mut exclusions = sample();
        exclusions.upsert(Exclusion::new(
            "10.1145/123456".to_string(),
            "A \"quoted\" title".to_string(),
            "duplicate study".to_string(),
            ExclusionStage::Screening,
        ));
        assert_eq!(exclusions.entries().len(), 2);
        let updated = exclusions.get("10.1145/123456").unwrap();
        assert_eq!(updated.reason, "duplicate study");
        assert_eq!(updated.stage, ExclusionStage::Screening);
    }

    #[test]
    fn an_unknown_stage_is_a_parse_error() {
        let path = scratch_path("bad-stage");
        std::fs::write(
            &path,
            r#"{ "10.1/x" = { title = "T"; reason = "R"; stage = "maybe"; }; }"#,
        )
        .unwrap();
        assert!(matches!(
            Exclusions::load(&path),
            Err(PaxError::LibraryParse(_))
        ));
    }

    #[test]
    fn a_missing_reason_is_a_parse_error() {
        let path = scratch_path("no-reason");
        std::fs::write(
            &path,
            r#"{ "10.1/x" = { title = "T"; stage = "screening"; }; }"#,
        )
        .unwrap();
        assert!(Exclusions::load(&path).is_err());
    }

    #[test]
    fn doi_keys_are_normalized() {
        assert_eq!(
            doi_exclusion_key(" https://doi.org/10.1145/ABC.Def "),
            "10.1145/abc.def"
        );
    }
}
