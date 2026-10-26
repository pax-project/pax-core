//! The review's PRISMA 2020 flow-diagram counts, computed from the files a
//! library already holds: the search log (`research/search-log.jsonl`), the
//! exclusions (`research/excluded.nix`) and the declared papers
//! (`research/papers.nix`). Nothing new is stored; [`stats`] reads the three
//! files and counts.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::Serialize;

use crate::error::PaxError;
use crate::exclusions::{ExclusionStage, Exclusions};
use crate::identity::{self, IdentityKey, parse_identity_key};
use crate::library::Library;
use crate::nix;
use crate::search_log::{SEARCH_LOG_VERSION, SearchCommand, SearchLogEntry, search_log_path};

/// A library's PRISMA 2020 flow-diagram counts. See [`stats`] for where each
/// one comes from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Stats {
    /// Records identified, per source.
    pub identified: Identified,
    /// Records removed as duplicates, across the whole log:
    /// `identified.total - records_screened`.
    pub duplicates_removed: usize,
    /// Distinct papers among every record identified: the records left after
    /// duplicates are removed.
    pub records_screened: usize,
    /// Papers excluded at title/abstract screening.
    pub excluded_screening: usize,
    /// Papers left to read in full: `records_screened - excluded_screening`.
    pub reports_sought: usize,
    /// Declared papers with no fetchable artifact: no `source_url` or no
    /// `hash` recorded.
    pub not_retrieved: usize,
    /// Papers read in full: `reports_sought - not_retrieved`.
    pub reports_assessed: usize,
    /// Papers excluded after reading the full text, by reason.
    pub excluded_full_text: FullTextExclusions,
    /// Declared papers with a fetchable artifact. A declared paper without
    /// one is only counted in `not_retrieved`.
    pub included: usize,
    /// Log lines where the provider failed, so retrieved no records; they
    /// count nowhere else.
    pub skipped_log_lines: usize,
}

/// Records identified, per provider (by its log tag, e.g. `openalex`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Identified {
    /// From database searches (`search`, `search --author`).
    pub databases: BTreeMap<String, usize>,
    /// From citation searching (`refs`, `cited-by`): PRISMA's "other
    /// methods".
    pub citation_searching: BTreeMap<String, usize>,
    /// Every record identified, from both.
    pub total: usize,
}

/// Full-text exclusions, grouped by reason.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct FullTextExclusions {
    pub total: usize,
    /// One entry per distinct reason (exact text), sorted by reason.
    pub reasons: Vec<ReasonCount>,
}

/// How many papers were excluded for one reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct ReasonCount {
    pub reason: String,
    pub count: usize,
}

/// The PRISMA 2020 flow-diagram counts of the library at `root`:
///
/// - **identified**: each answering provider's retrieved records, summed per
///   provider — `search` / `search --author` lines as databases, `refs` /
///   `cited-by` lines as citation searching. A line whose provider failed
///   retrieved nothing and is only counted in `skipped_log_lines`.
/// - **duplicates removed**: identified records minus distinct papers across
///   the *whole* log, not per command. Two records are the same paper when
///   they share an identity key, by [`identity::group`]'s rules (a
///   title+year match never joins two different non-arXiv DOIs); a record
///   with no key is a paper of its own.
/// - **excluded at screening / full text**: `excluded.nix` entries by
///   `stage`, the full-text ones grouped by `reason`.
/// - **not retrieved**: declared papers with no `source_url` or no `hash` —
///   those `check_library` reports as not fetched. Decided from
///   `papers.nix` alone, with no network: a recorded artifact whose source
///   has since gone away isn't counted (that's `check_library`'s job).
/// - **reports sought / assessed**: what's left after screening exclusions,
///   then after the reports not retrieved (never below zero).
/// - **included**: the declared papers with a fetchable artifact, so a paper
///   is never both included and not retrieved.
///
/// The boxes balance, as a PRISMA diagram's must, when every declared or
/// excluded paper was found by a logged search; a paper added or excluded
/// without one is still counted in its box.
///
/// A missing search log or `excluded.nix` counts as empty. A missing
/// `papers.nix` means there's no library: the error is its I/O error. A log
/// line that isn't a readable entry is a [`PaxError::SearchLogParse`] naming
/// it.
pub fn stats(root: &Path) -> Result<Stats, PaxError> {
    let library = Library::load(&nix::papers_path(root))?;
    let exclusions = Exclusions::load(&nix::excluded_path(root))?;
    let entries = read_log(&search_log_path(root))?;

    let mut stats = Stats::default();
    let mut records: Vec<Vec<IdentityKey>> = Vec::new();
    for (line, entry) in entries {
        let Some(keys) = entry.keys else {
            stats.skipped_log_lines += 1;
            continue;
        };
        let column = match entry.command {
            SearchCommand::Refs | SearchCommand::CitedBy => {
                &mut stats.identified.citation_searching
            }
            _ => &mut stats.identified.databases,
        };
        *column.entry(entry.provider).or_default() += keys.len();
        for record in keys {
            let parsed = record
                .iter()
                .map(|key| {
                    parse_identity_key(key).ok_or_else(|| PaxError::SearchLogParse {
                        line,
                        message: format!("{key:?} is not an identity key"),
                    })
                })
                .collect::<Result<_, _>>()?;
            records.push(parsed);
        }
    }
    stats.identified.total = records.len();
    stats.records_screened = distinct_papers(records);
    stats.duplicates_removed = stats.identified.total - stats.records_screened;

    let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
    for exclusion in exclusions.entries() {
        match exclusion.stage {
            ExclusionStage::Screening => stats.excluded_screening += 1,
            ExclusionStage::FullText => *reasons.entry(&exclusion.reason).or_default() += 1,
        }
    }
    stats.excluded_full_text = FullTextExclusions {
        total: reasons.values().sum(),
        reasons: reasons
            .into_iter()
            .map(|(reason, count)| ReasonCount {
                reason: reason.to_string(),
                count,
            })
            .collect(),
    };

    let (retrieved, not_retrieved): (Vec<_>, Vec<_>) = library
        .papers()
        .iter()
        .partition(|paper| paper.artifact.source_url.is_some() && paper.artifact.hash.is_some());
    stats.included = retrieved.len();
    stats.not_retrieved = not_retrieved.len();
    stats.reports_sought = stats
        .records_screened
        .saturating_sub(stats.excluded_screening);
    stats.reports_assessed = stats.reports_sought.saturating_sub(stats.not_retrieved);
    Ok(stats)
}

/// The log's entries, each with its line number (from 1); blank lines are
/// skipped. A missing log is empty.
fn read_log(path: &Path) -> Result<Vec<(usize, SearchLogEntry)>, PaxError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    text.lines()
        .enumerate()
        .map(|(index, text)| (index + 1, text))
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(line, text)| {
            let entry: SearchLogEntry =
                serde_json::from_str(text).map_err(|e| PaxError::SearchLogParse {
                    line,
                    message: e.to_string(),
                })?;
            if entry.v > SEARCH_LOG_VERSION {
                return Err(PaxError::SearchLogParse {
                    line,
                    message: format!(
                        "schema version {} is newer than this pax reads ({SEARCH_LOG_VERSION})",
                        entry.v
                    ),
                });
            }
            Ok((line, entry))
        })
        .collect()
}

/// How many distinct papers `records` hold, by [`identity::group`].
fn distinct_papers(records: Vec<Vec<IdentityKey>>) -> usize {
    // A record with no key is a paper of its own; of the rest, records with
    // identical keys always group together, so group each key set once — a
    // review's log repeats the same records across many searches.
    let (keyless, keyed): (Vec<_>, Vec<_>) = records.into_iter().partition(Vec::is_empty);
    let mut seen = HashSet::new();
    let unique: Vec<Vec<IdentityKey>> = keyed
        .into_iter()
        .filter(|keys| seen.insert(keys.clone()))
        .collect();
    let groups: HashSet<usize> = identity::group(&unique).into_iter().collect();
    keyless.len() + groups.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn scratch_library(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("pax-stats-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        nix::init_library(&root).unwrap();
        root
    }

    /// A log line: `keys` is `None` for a failed provider.
    fn line(command: &str, provider: &str, keys: Option<&[&[&str]]>) -> String {
        let input = match command {
            "refs" | "cited-by" => r#""seed":"lamport1978""#,
            _ => r#""query":"q""#,
        };
        let outcome = match keys {
            Some(keys) => {
                let keys: Vec<Vec<&str>> = keys.iter().map(|record| record.to_vec()).collect();
                format!(
                    r#""total":null,"retrieved":{},"keys":{}"#,
                    keys.len(),
                    serde_json::to_string(&keys).unwrap()
                )
            }
            None => r#""total":null,"error":"request failed""#.to_string(),
        };
        format!(
            r#"{{"v":1,"command":"{command}",{input},"provider":"{provider}","date":"2026-01-01T00:00:00Z","limit":50,"filters":[],{outcome},"merge":{{"before":0,"after":0,"unmatched":0}}}}"#
        )
    }

    fn write_log(root: &Path, lines: &[String]) {
        fs::write(search_log_path(root), lines.join("\n") + "\n").unwrap();
    }

    #[test]
    fn a_fresh_library_is_all_zeros() {
        let root = scratch_library("empty");
        assert_eq!(stats(&root).unwrap(), Stats::default());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn no_library_is_an_io_error() {
        let root = std::env::temp_dir().join(format!("pax-stats-none-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        assert!(matches!(stats(&root), Err(PaxError::Io(_))));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn databases_and_citation_searching_are_counted_apart_per_provider() {
        let root = scratch_library("columns");
        write_log(
            &root,
            &[
                line(
                    "search",
                    "openalex",
                    Some(&[&["doi:10.1/a"], &["doi:10.1/b"]]),
                ),
                line("search", "arxiv", Some(&[&["arxiv:1"]])),
                line("search-author", "openalex", Some(&[&["doi:10.1/c"]])),
                line("refs", "openalex", Some(&[&["doi:10.1/d"]])),
                line("cited-by", "semanticscholar", Some(&[&["doi:10.1/e"], &[]])),
                line("search", "crossref", Some(&[])),
            ],
        );
        let stats = stats(&root).unwrap();
        assert_eq!(
            stats.identified.databases,
            BTreeMap::from([
                ("arxiv".to_string(), 1),
                ("crossref".to_string(), 0),
                ("openalex".to_string(), 3),
            ])
        );
        assert_eq!(
            stats.identified.citation_searching,
            BTreeMap::from([
                ("openalex".to_string(), 1),
                ("semanticscholar".to_string(), 2)
            ])
        );
        assert_eq!(stats.identified.total, 7);
        assert_eq!(stats.duplicates_removed, 0);
        assert_eq!(stats.records_screened, 7);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn duplicates_are_counted_across_the_whole_log() {
        let root = scratch_library("dedup");
        write_log(
            &root,
            &[
                // A, and its preprint by title+year.
                line(
                    "search",
                    "openalex",
                    Some(&[&["doi:10.1/a", "title:attention|2017"], &["doi:10.1/b"]]),
                ),
                line(
                    "search",
                    "arxiv",
                    Some(&[&["arxiv:1706.03762", "title:attention|2017"]]),
                ),
                // A again from another command; B's preprint by arXiv id.
                line(
                    "refs",
                    "openalex",
                    Some(&[&["doi:10.1/a"], &["doi:10.1/b", "arxiv:2001.1"]]),
                ),
                line("cited-by", "semanticscholar", Some(&[&["arxiv:2001.1"]])),
                // Same title and year, different DOI: a different paper.
                line(
                    "search",
                    "crossref",
                    Some(&[&["doi:10.1/z", "title:attention|2017"]]),
                ),
                // Two keyless records: two papers.
                line("search", "crossref", Some(&[&[], &[]])),
            ],
        );
        let stats = stats(&root).unwrap();
        assert_eq!(stats.identified.total, 9);
        // A (4 records), B (3), Z, and the two keyless ones.
        assert_eq!(stats.records_screened, 5);
        assert_eq!(stats.duplicates_removed, 4);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn failed_provider_lines_are_skipped() {
        let root = scratch_library("failed");
        write_log(
            &root,
            &[
                line("search", "openalex", Some(&[&["doi:10.1/a"]])),
                line("search", "semanticscholar", None),
            ],
        );
        let stats = stats(&root).unwrap();
        assert_eq!(stats.skipped_log_lines, 1);
        assert_eq!(stats.identified.total, 1);
        assert!(!stats.identified.databases.contains_key("semanticscholar"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_malformed_log_line_is_an_error_naming_it() {
        let root = scratch_library("malformed");
        write_log(
            &root,
            &[
                line("search", "openalex", Some(&[&["doi:10.1/a"]])),
                String::new(),
                "{not json".to_string(),
            ],
        );
        match stats(&root) {
            Err(PaxError::SearchLogParse { line, .. }) => assert_eq!(line, 3),
            other => panic!("expected a parse error, got {other:?}"),
        }

        write_log(&root, &[line("search", "openalex", Some(&[&["isbn:1"]]))]);
        let error = stats(&root).unwrap_err();
        assert!(matches!(error, PaxError::SearchLogParse { line: 1, .. }));
        assert!(error.to_string().contains("\"isbn:1\""), "{error}");

        write_log(
            &root,
            &[line("search", "openalex", None).replace("\"v\":1", "\"v\":2")],
        );
        assert!(matches!(
            stats(&root),
            Err(PaxError::SearchLogParse { line: 1, .. })
        ));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn exclusions_by_stage_and_full_text_by_reason_and_papers() {
        let root = scratch_library("exclusions");
        fs::write(
            nix::excluded_path(&root),
            r#"{
  "10.1/a" = { title = "A"; reason = "off topic"; stage = "screening"; };
  "10.1/b" = { title = "B"; reason = "off topic"; stage = "screening"; };
  "10.1/c" = { title = "C"; reason = "not empirical"; stage = "full-text"; };
  "10.1/d" = { title = "D"; reason = "wrong population"; stage = "full-text"; };
  "10.1/e" = { title = "E"; reason = "not empirical"; stage = "full-text"; };
}
"#,
        )
        .unwrap();
        fs::write(
            nix::papers_path(&root),
            r#"{
  fetched2000 = {
    doi = null;
    title = "Fetched";
    authors = [ ];
    year = 2000;
    source_url = "https://example.org/a.pdf";
    hash = "sha256-AAAA";
    tags = [ ];
    notes = null;
  };
  nohash2001 = {
    doi = null;
    title = "No hash";
    authors = [ ];
    year = 2001;
    source_url = "https://example.org/b.pdf";
    hash = null;
    tags = [ ];
    notes = null;
  };
  nosource2002 = {
    doi = null;
    title = "No source";
    authors = [ ];
    year = 2002;
    source_url = null;
    hash = null;
    tags = [ ];
    notes = null;
  };
}
"#,
        )
        .unwrap();
        let stats = stats(&root).unwrap();
        assert_eq!(stats.excluded_screening, 2);
        assert_eq!(
            stats.excluded_full_text,
            FullTextExclusions {
                total: 3,
                reasons: vec![
                    ReasonCount {
                        reason: "not empirical".into(),
                        count: 2
                    },
                    ReasonCount {
                        reason: "wrong population".into(),
                        count: 1
                    },
                ],
            }
        );
        assert_eq!(stats.included, 1);
        assert_eq!(stats.not_retrieved, 2);
        // No log: nothing identified.
        assert_eq!(stats.identified, Identified::default());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn serializes_to_the_documented_json_shape() {
        let stats = Stats {
            identified: Identified {
                databases: BTreeMap::from([("openalex".to_string(), 2)]),
                citation_searching: BTreeMap::new(),
                total: 2,
            },
            duplicates_removed: 1,
            records_screened: 1,
            excluded_full_text: FullTextExclusions {
                total: 1,
                reasons: vec![ReasonCount {
                    reason: "r".into(),
                    count: 1,
                }],
            },
            ..Stats::default()
        };
        assert_eq!(
            serde_json::to_string(&stats).unwrap(),
            r#"{"identified":{"databases":{"openalex":2},"citation_searching":{},"total":2},"duplicates_removed":1,"records_screened":1,"excluded_screening":0,"reports_sought":0,"not_retrieved":0,"reports_assessed":0,"excluded_full_text":{"total":1,"reasons":[{"reason":"r","count":1}]},"included":0,"skipped_log_lines":0}"#
        );
    }

    #[test]
    fn identical_key_sets_are_grouped_once() {
        let a = || vec![IdentityKey::Doi("10.1/a".into())];
        assert_eq!(distinct_papers(vec![a(), a(), a(), vec![], vec![]]), 3);
        let many: Vec<Vec<IdentityKey>> = (0..10_000)
            .map(|i| vec![IdentityKey::Doi(format!("10.1/{}", i % 100))])
            .collect();
        assert_eq!(distinct_papers(many), 100);
    }
}
