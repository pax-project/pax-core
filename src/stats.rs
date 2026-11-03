//! The review's PRISMA 2020 flow-diagram counts, computed from the files a
//! library already holds: the search log (`research/search-log.jsonl`), the
//! exclusions (`research/excluded.nix`) and the declared papers
//! (`research/papers.nix`). Nothing new is stored; [`stats`] reads the three
//! files and counts.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use serde::Serialize;

use crate::error::PaxError;
use crate::exclusions::{Exclusion, ExclusionStage, Exclusions};
use crate::identity::{self, IdentityKey, parse_identity_key};
use crate::library::Library;
use crate::library_index::exclusion_id_keys;
use crate::nix;
use crate::search_log::{SEARCH_LOG_VERSION, SearchCommand, SearchLogEntry, search_log_path};

/// A library's PRISMA 2020 flow-diagram counts. See [`stats`] for where each
/// one comes from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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
    /// Distinct logged papers with no screening decision yet: neither
    /// excluded nor declared.
    pub awaiting_screening: usize,
    /// Papers that passed screening: declared papers plus papers excluded at
    /// full text (a declared paper can't be excluded, so a full-text
    /// exclusion passed screening first).
    pub reports_sought: usize,
    /// Declared papers with no artifact: no `hash` recorded.
    pub not_retrieved: usize,
    /// Papers read in full: `reports_sought - not_retrieved`.
    pub reports_assessed: usize,
    /// Papers excluded after reading the full text, by reason.
    pub excluded_full_text: FullTextExclusions,
    /// Papers excluded at each stage, by the protocol's criterion.
    pub excluded_by_criterion: CriterionExclusions,
    /// Declared papers with an artifact: a `hash`, fetched from a URL or
    /// added from a local file. A declared paper without one is only
    /// counted in `not_retrieved`.
    pub included: usize,
    /// Log lines where the provider failed, so retrieved no records; they
    /// count nowhere else.
    pub skipped_log_lines: usize,
    /// Declared or excluded papers that match no logged record, e.g. added
    /// from a DOI the reviewer already knew without `add --label`. Still
    /// counted in their box.
    pub decided_outside_log: usize,
    /// The records identified under each search-log label, labels in the
    /// order they first appear in the log; unlabelled lines are one group.
    /// Empty for an empty log.
    pub by_label: Vec<LabelCounts>,
}

/// The records identified under one search-log label.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct LabelCounts {
    /// The label; `None` for the lines without one.
    pub label: Option<String>,
    /// Records retrieved by the label's lines.
    pub identified: usize,
    /// Distinct papers among them.
    pub distinct: usize,
    /// Distinct papers not identified under any earlier label, so summing
    /// it over every label gives `records_screened`.
    pub new_distinct: usize,
    /// Duplicates within the label: `identified - distinct`.
    pub duplicates: usize,
}

/// Records identified, per provider (by its log tag, e.g. `openalex`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Identified {
    /// From database searches (`search`, `search --author`).
    pub databases: BTreeMap<String, usize>,
    /// From citation searching (`refs`, `cited-by`): PRISMA's "other
    /// methods".
    pub citation_searching: BTreeMap<String, usize>,
    /// From the providers' related-works recommendations (`related`): also
    /// PRISMA's "other methods", but a source of its own, counted apart from
    /// citation searching. PRISMA 2020 asks for each "other methods" source
    /// to be reported separately, and a recommendation isn't a citation edge:
    /// folding it into citation searching would overstate what the reference
    /// lists and forward citations found.
    pub related_works: BTreeMap<String, usize>,
    /// Papers the reviewer added by hand (`add --label`, logged as `manual`),
    /// keyed by the label rather than a provider, e.g. `seeds`: PRISMA's
    /// "other methods" too, one source per label, each reported apart from
    /// the searches. A `manual` line with no label is keyed by `""`.
    pub other_sources: BTreeMap<String, usize>,
    /// Every record identified, from all four.
    pub total: usize,
}

/// Full-text exclusions, grouped by reason.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct FullTextExclusions {
    pub total: usize,
    /// One entry per distinct reason (exact text), sorted by reason.
    pub reasons: Vec<ReasonCount>,
}

/// Exclusions at each stage, grouped by criterion.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct CriterionExclusions {
    /// Title/abstract screening exclusions.
    pub screening: Vec<CriterionCount>,
    /// Full-text exclusions.
    pub full_text: Vec<CriterionCount>,
}

/// How many papers were excluded under one criterion. One entry per distinct
/// criterion (exact text), sorted by criterion, then one with `criterion:
/// None` for exclusions recording none; a stage with no exclusions has no
/// entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct CriterionCount {
    pub criterion: Option<String>,
    pub count: usize,
}

/// How many papers were excluded for one reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ReasonCount {
    pub reason: String,
    pub count: usize,
}

/// The PRISMA 2020 flow-diagram counts of the library at `root`:
///
/// - **identified**: each answering provider's retrieved records, summed per
///   provider — `search` / `search --author` lines as databases, `refs` /
///   `cited-by` lines as citation searching, `related` lines as related
///   works (see [`Identified::related_works`]), and `manual` lines (papers
///   added by hand with `add --label`) per label as other sources (see
///   [`Identified::other_sources`]). A line whose provider failed
///   retrieved nothing and is only counted in `skipped_log_lines`.
/// - **duplicates removed**: identified records minus distinct papers across
///   the *whole* log, not per command. Two records are the same paper when
///   they share an identity key, by [`identity::group`]'s rules (a
///   title+year match never joins two different non-arXiv DOIs); a record
///   with no key is a paper of its own.
/// - **excluded at screening / full text**: `excluded.nix` entries by
///   `stage`, the full-text ones grouped by `reason`, and both grouped by
///   `criterion` (`excluded_by_criterion`).
/// - **not retrieved**: declared papers with no `hash` — those
///   `check_library` reports as not fetched. A paper with a hash and no
///   `source_url` was added from a local file ([`crate::fetch_file`]) and is
///   retrieved. Decided from `papers.nix` alone, with no network: a recorded
///   artifact whose source has since gone away isn't counted (that's
///   `check_library`'s job).
/// - **reports sought**: the papers that passed screening — declared, or
///   excluded at full text. **Assessed**: reports sought minus those not
///   retrieved (never below zero).
/// - **awaiting screening**: distinct logged papers no decision matches. A
///   declared paper or an exclusion decides the logged paper it shares an
///   identity key with, by [`identity::group`]'s rules (a declared paper by
///   its DOI, arXiv id or title+year; an exclusion by its key's DOI or
///   arXiv id, or by title+year when it records a year). A decision
///   matching no logged record — one keyed by an opaque `provider:id` with
///   no year, say — is counted in `decided_outside_log` instead, so
///   `awaiting_screening` never goes wrong or negative.
/// - **by label**: per search-log label ([`crate::SearchOptions::label`]),
///   in the order labels first appear, the records identified, the distinct
///   papers among them, and how many of those no earlier label identified;
///   that last count sums to `records_screened` over every label.
/// - **included**: the declared papers with a recorded artifact (a `hash`,
///   whether fetched from a URL or added from a local file), so a paper is
///   never both included and not retrieved.
///
/// The boxes balance, as a PRISMA diagram's must, when every declared or
/// excluded paper was found by a logged search (`decided_outside_log` is
/// 0): then `records_screened = excluded_screening + reports_sought +
/// awaiting_screening`. A paper added or excluded without one is still
/// counted in its box.
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
    // Each record's label, as an index into `labels` (first appearance).
    let mut labels: Vec<Option<String>> = Vec::new();
    let mut record_labels: Vec<usize> = Vec::new();
    for (line, entry) in entries {
        let label = match labels.iter().position(|label| *label == entry.label) {
            Some(index) => index,
            None => {
                labels.push(entry.label);
                labels.len() - 1
            }
        };
        let Some(keys) = entry.keys else {
            stats.skipped_log_lines += 1;
            continue;
        };
        let (column, source) = match entry.command {
            SearchCommand::Refs | SearchCommand::CitedBy => {
                (&mut stats.identified.citation_searching, entry.provider)
            }
            SearchCommand::Related => (&mut stats.identified.related_works, entry.provider),
            SearchCommand::Manual => (
                &mut stats.identified.other_sources,
                labels[label].clone().unwrap_or_default(),
            ),
            _ => (&mut stats.identified.databases, entry.provider),
        };
        *column.entry(source).or_default() += keys.len();
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
            record_labels.push(label);
        }
    }
    stats.identified.total = records.len();
    let decisions: Vec<Vec<IdentityKey>> = library
        .papers()
        .iter()
        .map(identity::paper_keys)
        .chain(exclusions.entries().iter().map(exclusion_keys))
        .collect();
    (stats.awaiting_screening, stats.decided_outside_log) = match_decisions(&records, &decisions);
    stats.by_label = label_counts(labels, &record_labels, &records);
    stats.records_screened = distinct_papers(records);
    stats.duplicates_removed = stats.identified.total - stats.records_screened;

    let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
    let (mut screening, mut full_text) = (BTreeMap::new(), BTreeMap::new());
    for exclusion in exclusions.entries() {
        let criteria = match exclusion.stage {
            ExclusionStage::Screening => {
                stats.excluded_screening += 1;
                &mut screening
            }
            ExclusionStage::FullText => {
                *reasons.entry(&exclusion.reason).or_default() += 1;
                &mut full_text
            }
        };
        // `None` sorts first in a map; it's listed last instead.
        *criteria
            .entry((
                exclusion.criterion.is_none(),
                exclusion.criterion.as_deref(),
            ))
            .or_default() += 1;
    }
    let by_criterion = |criteria: BTreeMap<(bool, Option<&str>), usize>| {
        criteria
            .into_iter()
            .map(|((_, criterion), count)| CriterionCount {
                criterion: criterion.map(str::to_string),
                count,
            })
            .collect()
    };
    stats.excluded_by_criterion = CriterionExclusions {
        screening: by_criterion(screening),
        full_text: by_criterion(full_text),
    };
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
        .partition(|paper| paper.artifact.hash.is_some());
    stats.included = retrieved.len();
    stats.not_retrieved = not_retrieved.len();
    stats.reports_sought = library.papers().len() + stats.excluded_full_text.total;
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

/// The keys an exclusion decides a paper by: its key's DOI or arXiv id, then
/// its title+year when the year is recorded. An opaque `provider:id` key
/// carries no id keys, so an exclusion keyed by one is matched by title+year
/// alone.
fn exclusion_keys(exclusion: &Exclusion) -> Vec<IdentityKey> {
    let mut keys = exclusion_id_keys(&exclusion.key);
    keys.extend(identity::title_year_key(&exclusion.title, exclusion.year));
    keys
}

/// How many distinct papers `records` hold, by [`identity::group`].
fn distinct_papers(records: Vec<Vec<IdentityKey>>) -> usize {
    paper_ids(&records)
        .into_iter()
        .collect::<HashSet<_>>()
        .len()
}

/// Which paper each of `records` is, by [`identity::group`]: one id per
/// record, the same id for the same paper.
fn paper_ids(records: &[Vec<IdentityKey>]) -> Vec<usize> {
    // A record with no key is a paper of its own; of the rest, records with
    // identical keys always group together, so group each key set once — a
    // review's log repeats the same records across many searches.
    let mut unique: HashMap<&Vec<IdentityKey>, usize> = HashMap::new();
    let mut key_sets: Vec<Vec<IdentityKey>> = Vec::new();
    for keys in records.iter().filter(|keys| !keys.is_empty()) {
        unique.entry(keys).or_insert_with(|| {
            key_sets.push(keys.clone());
            key_sets.len() - 1
        });
    }
    let groups = identity::group(&key_sets);
    let mut keyless = groups.len();
    records
        .iter()
        .map(|keys| match unique.get(keys) {
            Some(&set) => groups[set],
            None => {
                keyless += 1;
                keyless
            }
        })
        .collect()
}

/// The per-label counts: `record_labels` holds each record's index into
/// `labels`. A paper is new under the first label, in `labels`' order,
/// that identified it.
fn label_counts(
    labels: Vec<Option<String>>,
    record_labels: &[usize],
    records: &[Vec<IdentityKey>],
) -> Vec<LabelCounts> {
    let ids = paper_ids(records);
    let mut counts: Vec<LabelCounts> = labels
        .into_iter()
        .map(|label| LabelCounts {
            label,
            ..LabelCounts::default()
        })
        .collect();
    let mut papers: Vec<HashSet<usize>> = vec![HashSet::new(); counts.len()];
    for (&label, &id) in record_labels.iter().zip(&ids) {
        counts[label].identified += 1;
        papers[label].insert(id);
    }
    let mut seen = HashSet::new();
    for (count, papers) in counts.iter_mut().zip(papers) {
        count.distinct = papers.len();
        count.duplicates = count.identified - count.distinct;
        count.new_distinct = papers.into_iter().filter(|id| seen.insert(*id)).count();
    }
    counts
}

/// Matches screening decisions to the logged papers: how many distinct
/// papers among `records` no decision matches, and how many `decisions`
/// (each a declared paper's or an exclusion's keys) match no record. A
/// decision matches a paper when [`identity::group`] puts it with one of
/// the paper's records.
fn match_decisions(records: &[Vec<IdentityKey>], decisions: &[Vec<IdentityKey>]) -> (usize, usize) {
    // A keyless record can't be matched, so it awaits a decision for good.
    // Of the rest, group each key set once, as `distinct_papers` does.
    let keyless = records.iter().filter(|keys| keys.is_empty()).count();
    let mut seen = HashSet::new();
    let unique: Vec<Vec<IdentityKey>> = records
        .iter()
        .filter(|keys| !keys.is_empty() && seen.insert(*keys))
        .cloned()
        .collect();
    let papers = identity::group(&unique);

    let mut combined = unique;
    combined.extend(decisions.iter().cloned());
    let groups = identity::group(&combined);
    let (logged, decided) = groups.split_at(papers.len());
    let decided: HashSet<usize> = decided.iter().copied().collect();
    // With decisions added, the title+year guard can leave one paper's
    // records in two groups (a decision's DOI blocking a title match): the
    // paper is decided if any of its records is.
    let decided_papers: HashSet<usize> = papers
        .iter()
        .zip(logged)
        .filter(|(_, group)| decided.contains(group))
        .map(|(paper, _)| *paper)
        .collect();
    let distinct = papers.iter().collect::<HashSet<_>>().len();
    let logged: HashSet<usize> = logged.iter().copied().collect();
    let outside = groups[papers.len()..]
        .iter()
        .filter(|group| !logged.contains(group))
        .count();
    (keyless + distinct - decided_papers.len(), outside)
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
            "refs" | "cited-by" | "related" => r#""seed":"lamport1978""#,
            "manual" => r#""reference":"crossref:10.1/x""#,
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
    fn related_works_are_counted_apart_from_citation_searching() {
        let root = scratch_library("related");
        write_log(
            &root,
            &[
                line("refs", "openalex", Some(&[&["doi:10.1/a"]])),
                line(
                    "related",
                    "openalex",
                    Some(&[&["doi:10.1/a"], &["doi:10.1/b"]]),
                ),
                line("related", "semanticscholar", Some(&[&["doi:10.1/c"]])),
                line("related", "semanticscholar", None),
            ],
        );
        let stats = stats(&root).unwrap();
        assert_eq!(
            stats.identified.citation_searching,
            BTreeMap::from([("openalex".to_string(), 1)])
        );
        assert_eq!(
            stats.identified.related_works,
            BTreeMap::from([
                ("openalex".to_string(), 2),
                ("semanticscholar".to_string(), 1)
            ])
        );
        assert!(stats.identified.databases.is_empty());
        assert_eq!(stats.identified.total, 4);
        // `a`, found by both refs and related, is one paper.
        assert_eq!(stats.duplicates_removed, 1);
        assert_eq!(stats.records_screened, 3);
        assert_eq!(stats.skipped_log_lines, 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn papers_added_by_hand_are_other_sources_per_label() {
        let root = scratch_library("manual");
        write_log(
            &root,
            &[
                line("search", "openalex", Some(&[&["doi:10.1/a"]])),
                labelled(
                    "seeds",
                    line("manual", "crossref", Some(&[&["doi:10.1/a"]])),
                ),
                labelled(
                    "seeds",
                    line("manual", "openalex", Some(&[&["doi:10.1/b"]])),
                ),
                labelled(
                    "expert",
                    line("manual", "arxiv", Some(&[&["arxiv:2001.1"]])),
                ),
                // Hand-written, unlabelled: still another source, not a
                // database.
                line("manual", "crossref", Some(&[&["doi:10.1/c"]])),
            ],
        );
        let stats = stats(&root).unwrap();
        assert_eq!(
            stats.identified.databases,
            BTreeMap::from([("openalex".to_string(), 1)])
        );
        assert_eq!(
            stats.identified.other_sources,
            BTreeMap::from([
                (String::new(), 1),
                ("expert".to_string(), 1),
                ("seeds".to_string(), 2),
            ])
        );
        assert!(stats.identified.citation_searching.is_empty());
        assert_eq!(stats.identified.total, 5);
        // `a`, found by a search and added as a seed, is one paper.
        assert_eq!(stats.duplicates_removed, 1);
        assert_eq!(stats.records_screened, 4);
        let seeds = stats
            .by_label
            .iter()
            .find(|counts| counts.label.as_deref() == Some("seeds"))
            .unwrap();
        assert_eq!((seeds.identified, seeds.new_distinct), (2, 1));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_seeds_only_review_decides_nothing_outside_the_log_and_balances() {
        let root = scratch_library("seeds-only");
        write_log(
            &root,
            &[
                labelled(
                    "seeds",
                    line(
                        "manual",
                        "crossref",
                        Some(&[&["doi:10.1/a", "title:a paper|2020"]]),
                    ),
                ),
                labelled(
                    "seeds",
                    line(
                        "manual",
                        "arxiv",
                        Some(&[&["arxiv:2001.00001", "title:b paper|2021"]]),
                    ),
                ),
            ],
        );
        write_papers(
            &root,
            &[
                ("a2020", Some("10.1/a"), "A paper", 2020, None),
                (
                    "b2021",
                    None,
                    "B paper",
                    2021,
                    Some("https://arxiv.org/pdf/2001.00001"),
                ),
            ],
        );
        let stats = stats(&root).unwrap();
        assert_eq!(stats.identified.total, 2);
        assert_eq!(stats.decided_outside_log, 0);
        assert_eq!(stats.awaiting_screening, 0);
        assert_eq!(stats.records_screened, 2);
        assert_eq!(stats.reports_sought, 2);
        assert_eq!(
            stats.records_screened,
            stats.excluded_screening + stats.reports_sought + stats.awaiting_screening
        );
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
  localfile2003 = {
    doi = null;
    title = "Added from a local file";
    authors = [ ];
    year = 2003;
    source_url = null;
    hash = "sha256-BBBB";
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
        // None of them records a criterion.
        let none = |count| CriterionCount {
            criterion: None,
            count,
        };
        assert_eq!(
            stats.excluded_by_criterion,
            CriterionExclusions {
                screening: vec![none(2)],
                full_text: vec![none(3)],
            }
        );
        // A local artifact (a hash, no source_url) is retrieved.
        assert_eq!(stats.included, 2);
        assert_eq!(stats.not_retrieved, 2);
        // No log: nothing identified.
        assert_eq!(stats.identified, Identified::default());
        let _ = fs::remove_dir_all(&root);
    }

    /// A declared paper: citation key, DOI, title, year, source URL.
    type Declared<'a> = (&'a str, Option<&'a str>, &'a str, i32, Option<&'a str>);

    /// A `papers.nix` declaring `papers`.
    fn write_papers(root: &Path, papers: &[Declared]) {
        let quoted = |value: Option<&str>| value.map_or("null".to_string(), |v| format!("{v:?}"));
        let mut text = String::from("{\n");
        for (key, doi, title, year, source_url) in papers {
            text.push_str(&format!(
                "  {key} = {{\n    doi = {};\n    title = {title:?};\n    authors = [ ];\n    year = {year};\n    source_url = {};\n    hash = null;\n    tags = [ ];\n    notes = null;\n  }};\n",
                quoted(*doi),
                quoted(*source_url),
            ));
        }
        text.push_str("}\n");
        fs::write(nix::papers_path(root), text).unwrap();
    }

    /// An `excluded.nix` with one entry per `(key, stage)`.
    fn write_exclusions(root: &Path, exclusions: &[(&str, &str)]) {
        let mut text = String::from("{\n");
        for (key, stage) in exclusions {
            text.push_str(&format!(
                "  {key:?} = {{ title = \"T\"; reason = \"r\"; stage = {stage:?}; }};\n"
            ));
        }
        text.push_str("}\n");
        fs::write(nix::excluded_path(root), text).unwrap();
    }

    /// Five distinct papers, `10.1/a` to `10.1/e`, `c` found twice and `d` by
    /// its arXiv id.
    fn five_logged_papers(root: &Path) {
        write_log(
            root,
            &[
                line(
                    "search",
                    "openalex",
                    Some(&[&["doi:10.1/a"], &["doi:10.1/b"], &["doi:10.1/c"]]),
                ),
                line(
                    "refs",
                    "openalex",
                    Some(&[
                        &["doi:10.1/c"],
                        &["arxiv:2001.00001", "title:d paper|2020"],
                        &["doi:10.1/e", "title:e paper|2021"],
                    ]),
                ),
            ],
        );
    }

    #[test]
    fn papers_without_a_decision_are_awaiting_screening() {
        let root = scratch_library("awaiting");
        five_logged_papers(&root);
        write_exclusions(&root, &[("10.1/a", "screening")]);
        write_papers(&root, &[("b2000", Some("10.1/b"), "B", 2000, None)]);
        let stats = stats(&root).unwrap();
        assert_eq!(stats.records_screened, 5);
        assert_eq!(stats.excluded_screening, 1);
        assert_eq!(stats.reports_sought, 1);
        assert_eq!(stats.awaiting_screening, 3);
        assert_eq!(stats.decided_outside_log, 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn once_every_paper_is_decided_none_await_and_the_boxes_balance() {
        let root = scratch_library("decided");
        five_logged_papers(&root);
        write_exclusions(&root, &[("10.1/a", "screening"), ("10.1/c", "full-text")]);
        write_papers(
            &root,
            &[
                ("b2000", Some("10.1/b"), "B", 2000, None),
                // By the arXiv id of its source, and by title+year alone.
                (
                    "d2020",
                    None,
                    "D paper",
                    2020,
                    Some("https://arxiv.org/pdf/2001.00001v2"),
                ),
                ("e2021", None, "E: Paper", 2021, None),
            ],
        );
        let stats = stats(&root).unwrap();
        assert_eq!(stats.awaiting_screening, 0);
        assert_eq!(stats.decided_outside_log, 0);
        // What `records_screened - excluded_screening` gave before.
        assert_eq!(stats.reports_sought, 4);
        assert_eq!(stats.reports_assessed, 1);
        assert_eq!(
            stats.records_screened,
            stats.excluded_screening + stats.reports_sought + stats.awaiting_screening
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn decisions_matching_no_logged_paper_are_counted_apart() {
        let root = scratch_library("outside");
        five_logged_papers(&root);
        // An opaque key carries no DOI or arXiv id, so can't match.
        write_exclusions(
            &root,
            &[("10.1/a", "screening"), ("openalex:W1", "screening")],
        );
        write_papers(
            &root,
            &[
                ("b2000", Some("10.1/b"), "B", 2000, None),
                ("known1999", Some("10.9/known"), "Known", 1999, None),
                // Same title and year as `e`, a different DOI: not `e`.
                ("e2021", Some("10.9/e"), "E paper", 2021, None),
            ],
        );
        let stats = stats(&root).unwrap();
        assert_eq!(stats.excluded_screening, 2);
        assert_eq!(stats.reports_sought, 3);
        assert_eq!(stats.awaiting_screening, 3);
        assert_eq!(stats.decided_outside_log, 3);
        let _ = fs::remove_dir_all(&root);
    }

    /// A log with one DOI-less record, found by title+year alone, and one
    /// record with a DOI and the same title and year.
    fn grey_literature_logged(root: &Path) {
        write_log(
            root,
            &[line(
                "search",
                "openalex",
                Some(&[
                    &["title:a grey literature report|2019"],
                    &["doi:10.1/x", "title:same title|2019"],
                ]),
            )],
        );
    }

    #[test]
    fn an_exclusion_by_provider_id_with_a_year_decides_its_logged_paper() {
        let root = scratch_library("provider-id-year");
        grey_literature_logged(&root);
        fs::write(
            nix::excluded_path(&root),
            r#"{
  "openalex:W9" = { title = "A Grey-Literature Report"; reason = "r"; stage = "screening"; };
}
"#,
        )
        .unwrap();
        let before = stats(&root).unwrap();
        assert_eq!(before.awaiting_screening, 2);
        assert_eq!(before.decided_outside_log, 1);

        fs::write(
            nix::excluded_path(&root),
            r#"{
  "openalex:W9" = { title = "A Grey-Literature Report"; year = 2019; reason = "r"; stage = "screening"; };
}
"#,
        )
        .unwrap();
        let after = stats(&root).unwrap();
        assert_eq!(after.awaiting_screening, 1);
        assert_eq!(after.decided_outside_log, 0);
        assert_eq!(
            after.records_screened,
            after.excluded_screening + after.reports_sought + after.awaiting_screening
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_exclusion_with_the_same_title_and_year_but_another_doi_is_not_matched() {
        let root = scratch_library("same-title-other-doi");
        grey_literature_logged(&root);
        fs::write(
            nix::excluded_path(&root),
            r#"{
  "10.1/y" = { title = "Same title"; year = 2019; reason = "r"; stage = "screening"; };
}
"#,
        )
        .unwrap();
        let stats = stats(&root).unwrap();
        assert_eq!(stats.awaiting_screening, 2);
        assert_eq!(stats.decided_outside_log, 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn exclusions_are_grouped_by_criterion_per_stage_with_none_last() {
        let root = scratch_library("criteria");
        fs::write(
            nix::excluded_path(&root),
            r#"{
  "10.1/a" = { title = "A"; reason = "r"; stage = "screening"; criterion = "X3"; round = "R1"; };
  "10.1/b" = { title = "B"; reason = "r"; stage = "screening"; };
  "10.1/c" = { title = "C"; reason = "r"; stage = "screening"; criterion = "X1"; };
  "10.1/d" = { title = "D"; reason = "r"; stage = "screening"; criterion = "X3"; };
  "10.1/e" = { title = "E"; reason = "r"; stage = "full-text"; criterion = "X2"; };
}
"#,
        )
        .unwrap();
        let count = |criterion: Option<&str>, count| CriterionCount {
            criterion: criterion.map(str::to_string),
            count,
        };
        assert_eq!(
            stats(&root).unwrap().excluded_by_criterion,
            CriterionExclusions {
                screening: vec![count(Some("X1"), 1), count(Some("X3"), 2), count(None, 1)],
                full_text: vec![count(Some("X2"), 1)],
            }
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// `line` with a `label`.
    fn labelled(label: &str, line: String) -> String {
        line.replacen(r#""v":1,"#, &format!(r#""v":1,"label":{label:?},"#), 1)
    }

    #[test]
    fn counts_per_label_in_log_order_with_new_papers_summing_to_records_screened() {
        let root = scratch_library("labels");
        write_log(
            &root,
            &[
                // An old, unlabelled line: grouped as unlabelled.
                line("search", "openalex", Some(&[&["doi:10.1/a"]])),
                labelled(
                    "R1",
                    line(
                        "search",
                        "openalex",
                        Some(&[&["doi:10.1/a"], &["doi:10.1/b"], &["doi:10.1/b"]]),
                    ),
                ),
                labelled(
                    "R2-backward",
                    line(
                        "refs",
                        "openalex",
                        Some(&[&["doi:10.1/b"], &["doi:10.1/c"], &[]]),
                    ),
                ),
                labelled("R2-backward", line("refs", "semanticscholar", None)),
                // R1 again: still R1's, and `c` isn't new (R2 came first in
                // the log, but R1 is the earlier label).
                labelled(
                    "R1",
                    line(
                        "search",
                        "arxiv",
                        Some(&[&["arxiv:1", "doi:10.1/d"], &["doi:10.1/c"]]),
                    ),
                ),
                labelled("R3", line("cited-by", "openalex", None)),
            ],
        );
        let stats = stats(&root).unwrap();
        let counts =
            |label: Option<&str>, identified, distinct, new_distinct, duplicates| LabelCounts {
                label: label.map(str::to_string),
                identified,
                distinct,
                new_distinct,
                duplicates,
            };
        assert_eq!(
            stats.by_label,
            vec![
                counts(None, 1, 1, 1, 0),
                // a, b, d, c: a was unlabelled first.
                counts(Some("R1"), 5, 4, 3, 1),
                // b, c, and a keyless record.
                counts(Some("R2-backward"), 3, 3, 1, 0),
                // Every line failed: nothing identified.
                counts(Some("R3"), 0, 0, 0, 0),
            ]
        );
        assert_eq!(stats.records_screened, 5);
        assert_eq!(
            stats
                .by_label
                .iter()
                .map(|label| label.new_distinct)
                .sum::<usize>(),
            stats.records_screened
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn serializes_to_the_documented_json_shape() {
        let stats = Stats {
            identified: Identified {
                databases: BTreeMap::from([("openalex".to_string(), 2)]),
                citation_searching: BTreeMap::new(),
                related_works: BTreeMap::new(),
                other_sources: BTreeMap::new(),
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
            by_label: vec![LabelCounts {
                label: Some("R1".into()),
                identified: 2,
                distinct: 1,
                new_distinct: 1,
                duplicates: 1,
            }],
            ..Stats::default()
        };
        assert_eq!(
            serde_json::to_string(&stats).unwrap(),
            r#"{"identified":{"databases":{"openalex":2},"citation_searching":{},"related_works":{},"other_sources":{},"total":2},"duplicates_removed":1,"records_screened":1,"excluded_screening":0,"awaiting_screening":0,"reports_sought":0,"not_retrieved":0,"reports_assessed":0,"excluded_full_text":{"total":1,"reasons":[{"reason":"r","count":1}]},"excluded_by_criterion":{"screening":[],"full_text":[]},"included":0,"skipped_log_lines":0,"decided_outside_log":0,"by_label":[{"label":"R1","identified":2,"distinct":1,"new_distinct":1,"duplicates":1}]}"#
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
