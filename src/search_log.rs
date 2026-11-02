//! The search log, `research/search-log.jsonl`: the library's provenance.
//! Every search a review runs — `search`, `search --author`, `refs`,
//! `cited-by`, `related` — appends one JSON line per provider asked,
//! recording what PRISMA 2020 asks a review to report (each source, the date
//! it was searched, the full search string with its filters and limits) plus
//! the identity key of every record retrieved, so duplicates can be counted
//! across the whole review, not just within one command. A paper added by
//! hand with a label (`add --label`, [`add_candidate_logged`]) appends one
//! `manual` line too, so it counts as identified from that source.
//!
//! Written by the library, not a client, so every client logs the same way:
//! the `*_logged` functions run a search and log it, and [`log_results`]
//! logs results a caller fetched itself. The log is append-only: pax never
//! rewrites an earlier line. It exists only inside a library (a root with
//! `research/papers.nix`); outside one, nothing is logged.

use std::collections::HashMap;
use std::fmt;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::PaxError;
use crate::identity::identity_keys;
use crate::merge::{MERGE_ORDER, merge_works};
use crate::paper::PaperRef;
use crate::provider::{
    CandidateId, CandidateWork, FilterApplication, FilterKind, Filters, Limit, ProviderError,
    ProviderId, SearchOptions, SearchPage,
};
use crate::{CitationError, CitationResults, Config, IncompleteReason, SeedResults, nix};

/// The schema version every entry carries in its `v` field. Bumped when a
/// field changes meaning or is removed; adding a field doesn't bump it.
pub const SEARCH_LOG_VERSION: u32 = 1;

/// Where `research/search-log.jsonl` lives under a library root.
pub fn search_log_path(root: &Path) -> PathBuf {
    root.join("research").join("search-log.jsonl")
}

/// Which search a log entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum SearchCommand {
    /// A free-text search ([`crate::search_all_paged`]).
    Search,
    /// A search by author name ([`crate::search_by_author_paged`]).
    SearchAuthor,
    /// The works a seed cites ([`crate::references`]).
    Refs,
    /// The works citing a seed ([`crate::cited_by`]).
    CitedBy,
    /// The works the providers recommend as related to a seed
    /// ([`crate::related`]). Logged as `related`.
    Related,
    /// A paper the reviewer added by hand rather than found by a search,
    /// e.g. one from a list of seed papers they already knew
    /// ([`add_candidate_logged`]). Its line's `label` names the source.
    /// Logged as `manual`.
    Manual,
}

impl SearchCommand {
    /// Whether the command's input is a seed paper rather than a query.
    fn takes_seed(self) -> bool {
        matches!(
            self,
            SearchCommand::Refs | SearchCommand::CitedBy | SearchCommand::Related
        )
    }
}

/// One provider's part in one search: a line of the log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SearchLogEntry {
    /// Schema version, [`SEARCH_LOG_VERSION`].
    pub v: u32,
    pub command: SearchCommand,
    /// The query or author name, for `search` / `search-author`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// The seed as given (a `provider:id` reference or a citation key), for
    /// `refs` / `cited-by` / `related`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<String>,
    /// The `provider:id` reference added, for `manual`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// The provider's tag, e.g. `openalex`; for `manual`, the provider the
    /// added reference was resolved from.
    pub provider: String,
    /// When the search ran: ISO 8601 in UTC, to the second. Every line of one
    /// command carries the same date.
    pub date: String,
    /// An id shared by every line one command wrote, and by no other
    /// command's: with several `refs` / `cited-by` / `related` seeds, it
    /// ties their lines together. Opaque. Absent on lines written before it
    /// was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// The search's label ([`SearchOptions::label`]), the same on every line
    /// of one command; absent when it had none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// How many records were asked for per provider.
    pub limit: LoggedLimit,
    /// Every requested filter, with how this provider applied it.
    pub filters: Vec<LoggedFilter>,
    /// How many records the provider reported matching; `None` when it
    /// doesn't report one, or failed.
    pub total: Option<u64>,
    /// How many records were retrieved; absent when the provider failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieved: Option<usize>,
    /// Each retrieved record's identity keys (see [`crate::IdentityKey`]'s
    /// text form), strongest first, one list per record in the order
    /// retrieved; an empty list is a record with no key. Absent when the
    /// provider failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<Vec<Vec<String>>>,
    /// Why the provider returned no records — including not knowing the
    /// seed. Absent when it answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What kind of failure `error` is, as [`IncompleteReason::as_str`]
    /// names it: `withheld`, `failed` or `unresolved`. Absent when the
    /// provider answered, and on lines logged through the generic
    /// [`log_results`], which can't tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The command's merge counts across every provider that answered; the
    /// same on every line of one command, or with several seeds, on every
    /// line of one seed.
    pub merge: MergeCounts,
}

/// The per-provider limit a search ran with: a count, or `"all"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LoggedLimit {
    Count(usize),
    All,
}

impl From<Limit> for LoggedLimit {
    fn from(limit: Limit) -> Self {
        match limit {
            Limit::Count(n) => LoggedLimit::Count(n),
            Limit::All => LoggedLimit::All,
        }
    }
}

impl Serialize for LoggedLimit {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            LoggedLimit::Count(n) => serializer.serialize_u64(*n as u64),
            LoggedLimit::All => serializer.serialize_str("all"),
        }
    }
}

impl<'de> Deserialize<'de> for LoggedLimit {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Count(usize),
            Word(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Count(n) => Ok(LoggedLimit::Count(n)),
            Raw::Word(word) if word == "all" => Ok(LoggedLimit::All),
            Raw::Word(word) => Err(serde::de::Error::custom(format!(
                "expected a count or \"all\", got {word:?}"
            ))),
        }
    }
}

/// One requested filter, as one provider applied it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LoggedFilter {
    /// `year`, `type` or `language`.
    pub filter: String,
    /// The requested value, e.g. `2015-2020`, `journal-article`, `en`.
    pub value: String,
    /// `server-side`, `post-retrieval` or `not-applied`; absent when the
    /// provider failed, so never applied it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied: Option<String>,
}

/// A command's merge counts (see [`crate::MergeReport`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MergeCounts {
    pub before: usize,
    pub after: usize,
    pub unmatched: usize,
}

/// The log entries for one command: one per provider in `results`, in
/// [`MERGE_ORDER`]. `input` is the query, author name or seed, as
/// `command` takes it; `date` is when the command ran.
pub fn log_entries<E: fmt::Display>(
    command: SearchCommand,
    input: &str,
    options: &SearchOptions,
    results: &HashMap<ProviderId, Result<SearchPage, E>>,
    date: SystemTime,
) -> Vec<SearchLogEntry> {
    let date = DateTime::<Utc>::from(date).to_rfc3339_opts(SecondsFormat::Secs, true);
    let pages = || {
        MERGE_ORDER
            .iter()
            .filter_map(|provider| results.get(provider)?.as_ref().ok())
    };
    let report = merge_works(pages().flat_map(|page| page.works.iter().cloned()));
    let merge = MergeCounts {
        before: report.before,
        after: report.after,
        unmatched: report.unmatched,
    };
    let (query, seed) = if command.takes_seed() {
        (None, Some(input.to_string()))
    } else {
        (Some(input.to_string()), None)
    };

    MERGE_ORDER
        .iter()
        .filter_map(|provider| Some((*provider, results.get(provider)?)))
        .map(|(provider, result)| {
            let mut entry = SearchLogEntry {
                v: SEARCH_LOG_VERSION,
                command,
                query: query.clone(),
                seed: seed.clone(),
                reference: None,
                provider: provider.to_string(),
                date: date.clone(),
                run: None,
                label: options.label.clone(),
                limit: options.limit.into(),
                filters: Vec::new(),
                total: None,
                retrieved: None,
                keys: None,
                error: None,
                reason: None,
                merge,
            };
            match result {
                Ok(page) => {
                    entry.filters = page
                        .filters
                        .iter()
                        .map(|applied| {
                            logged_filter(&options.filters, applied.kind, Some(applied.application))
                        })
                        .collect();
                    entry.total = page.total;
                    entry.retrieved = Some(page.retrieved());
                    entry.keys = Some(
                        page.works
                            .iter()
                            .map(|work| {
                                identity_keys(work)
                                    .iter()
                                    .map(ToString::to_string)
                                    .collect()
                            })
                            .collect(),
                    );
                }
                Err(e) => {
                    entry.filters = options
                        .filters
                        .kinds()
                        .into_iter()
                        .map(|kind| logged_filter(&options.filters, kind, None))
                        .collect();
                    entry.error = Some(e.to_string());
                }
            }
            entry
        })
        .collect()
}

fn logged_filter(
    filters: &Filters,
    kind: FilterKind,
    application: Option<FilterApplication>,
) -> LoggedFilter {
    let value = match kind {
        FilterKind::Year => filters.years.map(|years| years.to_string()),
        FilterKind::DocType => filters.doc_type.map(|doc_type| doc_type.to_string()),
        FilterKind::Language => filters.language.clone(),
    };
    LoggedFilter {
        filter: kind.to_string(),
        value: value.unwrap_or_default(),
        applied: application.map(|application| match application {
            FilterApplication::Server => "server-side".to_string(),
            FilterApplication::PostRetrieval => "post-retrieval".to_string(),
            FilterApplication::NotApplied => "not-applied".to_string(),
        }),
    }
}

/// A library's search log file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SearchLog {
    path: PathBuf,
}

impl SearchLog {
    /// The log of the library at `root`; `None` when `root` has no library
    /// (no `research/papers.nix`), where nothing is logged.
    pub fn for_library(root: &Path) -> Option<Self> {
        nix::papers_path(root)
            .exists()
            .then(|| SearchLog::at(search_log_path(root)))
    }

    /// The log at `path`, whether or not it's in a library.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        SearchLog { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends `entries`, one JSON object per line, creating the file if it
    /// doesn't exist. Never touches what's already there; the lines go out
    /// in one write, so one command's lines stay together.
    pub fn append(&self, entries: &[SearchLogEntry]) -> io::Result<()> {
        let mut lines = String::new();
        for entry in entries {
            lines.push_str(&serde_json::to_string(entry).map_err(io::Error::other)?);
            lines.push('\n');
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?
            .write_all(lines.as_bytes())
    }
}

/// Whether a search was logged.
#[derive(Debug)]
#[non_exhaustive]
pub enum LogOutcome {
    /// Appended to the library's log, at this path.
    Written(PathBuf),
    /// No library at the root, so nothing was logged.
    NoLibrary,
    /// Writing the log failed; the search results are still good.
    Failed(io::Error),
    /// There was nothing to log: every seed of a multi-seed lookup was
    /// invalid, so no provider was asked.
    Nothing,
}

/// A search's results, and whether it was logged.
#[derive(Debug)]
#[non_exhaustive]
pub struct Logged<T> {
    pub results: T,
    pub log: LogOutcome,
}

/// Logs one command's per-provider results in the library at `root`, dated
/// now. For a client that runs a search itself rather than through a
/// `*_logged` function. Generic over the error type, it can't tell what kind
/// of failure an error is, so its lines carry no `reason`; the `*_logged`
/// functions record one.
pub fn log_results<E: fmt::Display>(
    root: &Path,
    command: SearchCommand,
    input: &str,
    options: &SearchOptions,
    results: &HashMap<ProviderId, Result<SearchPage, E>>,
) -> LogOutcome {
    append_to_library(root, || {
        log_entries(command, input, options, results, SystemTime::now())
    })
}

/// Appends `entries()` to the log of the library at `root`, if there is one,
/// giving every line one fresh [`SearchLogEntry::run`] id. With no entries
/// it writes nothing, and says so.
fn append_to_library(root: &Path, entries: impl FnOnce() -> Vec<SearchLogEntry>) -> LogOutcome {
    let mut entries = entries();
    if entries.is_empty() {
        return LogOutcome::Nothing;
    }
    let Some(log) = SearchLog::for_library(root) else {
        return LogOutcome::NoLibrary;
    };
    let run = new_run_id();
    for entry in &mut entries {
        entry.run.get_or_insert_with(|| run.clone());
    }
    match log.append(&entries) {
        Ok(()) => LogOutcome::Written(log.path),
        Err(e) => LogOutcome::Failed(e),
    }
}

/// A run id no other command writes: the time in nanoseconds, the process
/// id, and a count of the runs this process logged, in hex.
fn new_run_id() -> String {
    static RUNS: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let count = RUNS.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}-{:x}-{count:x}", std::process::id())
}

/// An error whose kind of failure a log line records as its `reason`.
trait FailureReason: fmt::Display {
    fn failure_reason(&self) -> IncompleteReason;
}

impl FailureReason for CitationError {
    fn failure_reason(&self) -> IncompleteReason {
        self.reason()
    }
}

impl FailureReason for ProviderError {
    fn failure_reason(&self) -> IncompleteReason {
        CitationError::Provider(self.clone()).reason()
    }
}

/// [`log_entries`], with each failed line's `reason`.
fn entries_with_reasons<E: FailureReason>(
    command: SearchCommand,
    input: &str,
    options: &SearchOptions,
    results: &HashMap<ProviderId, Result<SearchPage, E>>,
    date: SystemTime,
) -> Vec<SearchLogEntry> {
    let mut entries = log_entries(command, input, options, results, date);
    for entry in &mut entries {
        let error = entry
            .provider
            .parse::<ProviderId>()
            .ok()
            .and_then(|provider| results.get(&provider)?.as_ref().err());
        entry.reason = error.map(|e| e.failure_reason().as_str().to_string());
    }
    entries
}

/// [`log_results`] for the `*_logged` functions, recording each failed
/// line's `reason`.
fn log_with_reasons<E: FailureReason>(
    root: &Path,
    command: SearchCommand,
    input: &str,
    options: &SearchOptions,
    results: &HashMap<ProviderId, Result<SearchPage, E>>,
) -> LogOutcome {
    append_to_library(root, || {
        entries_with_reasons(command, input, options, results, SystemTime::now())
    })
}

/// Per-provider search results, as [`crate::search_all_paged`] returns them.
pub type SearchResults = HashMap<ProviderId, Result<SearchPage, ProviderError>>;

/// [`crate::search_all_paged`], logged in the library at `root` (if any).
pub async fn search_all_logged(
    query: &str,
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> Logged<SearchResults> {
    let results = crate::search_all_paged(query, options, config).await;
    let log = log_with_reasons(root, SearchCommand::Search, query, options, &results);
    Logged { results, log }
}

/// [`crate::search_by_author_paged`], logged in the library at `root` (if
/// any).
pub async fn search_by_author_logged(
    author: &str,
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> Logged<SearchResults> {
    let results = crate::search_by_author_paged(author, options, config).await;
    let log = log_with_reasons(root, SearchCommand::SearchAuthor, author, options, &results);
    Logged { results, log }
}

/// [`crate::references`], logged in the library at `root` (if any). An
/// invalid seed is an error, as there, and nothing is logged.
pub async fn references_logged(
    seed: &str,
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> Result<Logged<CitationResults>, PaxError> {
    let results = crate::references(seed, options, root, config).await?;
    let log = log_with_reasons(root, SearchCommand::Refs, seed, options, &results);
    Ok(Logged { results, log })
}

/// [`crate::cited_by`], logged in the library at `root` (if any). An invalid
/// seed is an error, as there, and nothing is logged.
pub async fn cited_by_logged(
    seed: &str,
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> Result<Logged<CitationResults>, PaxError> {
    let results = crate::cited_by(seed, options, root, config).await?;
    let log = log_with_reasons(root, SearchCommand::CitedBy, seed, options, &results);
    Ok(Logged { results, log })
}

/// The log entries for a multi-seed lookup: each seed with lists logged as
/// its own `command` would be on its own — one line per provider, with that
/// seed's merge counts — so `pax stats` counts the same either way. An
/// invalid seed logs nothing, as on its own.
fn seed_log_entries(
    command: SearchCommand,
    options: &SearchOptions,
    results: &SeedResults,
    date: SystemTime,
) -> Vec<SearchLogEntry> {
    results
        .seeds
        .iter()
        .filter_map(|seed| Some((seed.seed.as_str(), seed.results.as_ref().ok()?)))
        .flat_map(|(seed, lists)| entries_with_reasons(command, seed, options, lists, date))
        .collect()
}

/// [`crate::references_many`], logged in the library at `root` (if any):
/// every seed's lines in one append, sharing one `run` id.
pub async fn references_many_logged<S: AsRef<str>>(
    seeds: &[S],
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> Logged<SeedResults> {
    let results = crate::references_many(seeds, options, root, config).await;
    let log = append_to_library(root, || {
        seed_log_entries(SearchCommand::Refs, options, &results, SystemTime::now())
    });
    Logged { results, log }
}

/// [`crate::cited_by_many`], logged in the library at `root` (if any); as
/// [`references_many_logged`].
pub async fn cited_by_many_logged<S: AsRef<str>>(
    seeds: &[S],
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> Logged<SeedResults> {
    let results = crate::cited_by_many(seeds, options, root, config).await;
    let log = append_to_library(root, || {
        seed_log_entries(SearchCommand::CitedBy, options, &results, SystemTime::now())
    });
    Logged { results, log }
}

/// [`crate::related`], logged in the library at `root` (if any) as a
/// [`SearchCommand::Related`]. An invalid seed is an error, as there, and
/// nothing is logged.
pub async fn related_logged(
    seed: &str,
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> Result<Logged<CitationResults>, PaxError> {
    let results = crate::related(seed, options, root, config).await?;
    let log = log_with_reasons(root, SearchCommand::Related, seed, options, &results);
    Ok(Logged { results, log })
}

/// [`crate::related_many`], logged in the library at `root` (if any); as
/// [`references_many_logged`].
pub async fn related_many_logged<S: AsRef<str>>(
    seeds: &[S],
    options: &SearchOptions,
    root: &Path,
    config: &Config,
) -> Logged<SeedResults> {
    let results = crate::related_many(seeds, options, root, config).await;
    let log = append_to_library(root, || {
        seed_log_entries(SearchCommand::Related, options, &results, SystemTime::now())
    });
    Logged { results, log }
}

/// The log line for adding `id` by hand under `label`: a
/// [`SearchCommand::Manual`] line for `id`'s provider that retrieved one
/// record, `work` (what `id` resolved to), with its identity keys.
pub fn manual_log_entry(
    id: &CandidateId,
    work: &CandidateWork,
    label: &str,
    date: SystemTime,
) -> SearchLogEntry {
    let keys: Vec<String> = identity_keys(work)
        .iter()
        .map(ToString::to_string)
        .collect();
    SearchLogEntry {
        v: SEARCH_LOG_VERSION,
        command: SearchCommand::Manual,
        query: None,
        seed: None,
        reference: Some(id.to_string()),
        provider: id.provider.to_string(),
        date: DateTime::<Utc>::from(date).to_rfc3339_opts(SecondsFormat::Secs, true),
        run: None,
        label: Some(label.to_string()),
        limit: LoggedLimit::Count(1),
        filters: Vec::new(),
        total: None,
        retrieved: Some(1),
        merge: MergeCounts {
            before: 1,
            after: 1,
            unmatched: usize::from(keys.is_empty()),
        },
        keys: Some(vec![keys]),
        error: None,
        reason: None,
    }
}

/// [`crate::add_candidate`], with the add logged in the library at `root`
/// as a [`SearchCommand::Manual`] line labelled `label` (e.g. `seeds`), so
/// [`crate::stats`] counts the paper as identified from that source
/// ([`crate::Identified::other_sources`]) rather than as decided outside the
/// log. The label only names the source: what is added is the same as
/// without it. A refused or failed add is an error, as there, and nothing is
/// logged; a failed log write leaves the paper added and says so in
/// [`Logged::log`].
pub async fn add_candidate_logged(
    id: &CandidateId,
    label: &str,
    root: &Path,
    config: &Config,
) -> Result<Logged<PaperRef>, PaxError> {
    let (results, work) = crate::declare_candidate(id, root, config).await?;
    let log = append_to_library(root, || {
        vec![manual_log_entry(id, &work, label, SystemTime::now())]
    });
    Ok(Logged { results, log })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{AppliedFilter, CandidateId, CandidateWork, DocType, YearRange};
    use crate::{CitationError, ProviderError};
    use std::time::Duration;

    fn work(
        provider: ProviderId,
        native_id: &str,
        doi: Option<&str>,
        title: &str,
    ) -> CandidateWork {
        CandidateWork {
            id: CandidateId {
                provider,
                native_id: native_id.to_string(),
            },
            title: title.to_string(),
            authors: Vec::new(),
            publish_date: "2017".to_string(),
            doi: doi.map(str::to_string),
            pdf_url: None,
            venue: None,
            abstract_text: None,
        }
    }

    /// 2017-06-12T00:00:00Z.
    fn date() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_497_225_600)
    }

    fn filtered_options() -> SearchOptions {
        SearchOptions::default()
            .with_limit(Limit::Count(25))
            .with_filters(
                Filters::default()
                    .with_years(YearRange::new(Some(2015), Some(2020)).unwrap())
                    .with_doc_type(DocType::JournalArticle),
            )
    }

    fn search_results() -> HashMap<ProviderId, Result<SearchPage, ProviderError>> {
        HashMap::from([
            (
                ProviderId::OpenAlex,
                Ok(SearchPage::new(
                    vec![
                        work(ProviderId::OpenAlex, "W1", Some("10.1/A"), "Attention"),
                        work(ProviderId::OpenAlex, "W2", None, ""),
                    ],
                    Some(1284),
                    None,
                )
                .with_filters(vec![
                    AppliedFilter::new(FilterKind::Year, FilterApplication::Server),
                    AppliedFilter::new(FilterKind::DocType, FilterApplication::Server),
                ])),
            ),
            (
                ProviderId::ArXiv,
                Ok(SearchPage::new(
                    vec![work(ProviderId::ArXiv, "1706.03762v5", None, "Attention")],
                    None,
                    None,
                )
                .with_filters(vec![
                    AppliedFilter::new(FilterKind::Year, FilterApplication::PostRetrieval),
                    AppliedFilter::new(FilterKind::DocType, FilterApplication::NotApplied),
                ])),
            ),
            (
                ProviderId::SemanticScholar,
                Err(ProviderError::Request("rate limited".into())),
            ),
        ])
    }

    #[test]
    fn a_line_per_provider_in_provider_order_with_the_query_and_date() {
        let entries = log_entries(
            SearchCommand::Search,
            "attention",
            &filtered_options(),
            &search_results(),
            date(),
        );
        let providers: Vec<&str> = entries.iter().map(|e| e.provider.as_str()).collect();
        assert_eq!(providers, ["openalex", "semanticscholar", "arxiv"]);
        for entry in &entries {
            assert_eq!(entry.v, 1);
            assert_eq!(entry.command, SearchCommand::Search);
            assert_eq!(entry.query.as_deref(), Some("attention"));
            assert_eq!(entry.seed, None);
            assert_eq!(entry.date, "2017-06-12T00:00:00Z");
            assert_eq!(entry.limit, LoggedLimit::Count(25));
        }
    }

    #[test]
    fn an_answering_provider_logs_every_records_keys_and_counts() {
        let entries = log_entries(
            SearchCommand::Search,
            "attention",
            &filtered_options(),
            &search_results(),
            date(),
        );
        let openalex = &entries[0];
        assert_eq!(openalex.total, Some(1284));
        assert_eq!(openalex.retrieved, Some(2));
        assert_eq!(
            openalex.keys,
            Some(vec![
                vec!["doi:10.1/a".to_string(), "title:attention|2017".to_string()],
                // A record with no key still gets its (empty) list.
                vec![],
            ])
        );
        assert_eq!(openalex.error, None);
        let arxiv = &entries[2];
        assert_eq!(
            arxiv.keys,
            Some(vec![vec![
                "arxiv:1706.03762".to_string(),
                "title:attention|2017".to_string(),
            ]])
        );
        // 3 records, 2 papers (the arXiv record joins W1 by title+year), 1
        // without a key — on every line, including the failed provider's.
        for entry in &entries {
            assert_eq!(
                entry.merge,
                MergeCounts {
                    before: 3,
                    after: 2,
                    unmatched: 1
                }
            );
        }
    }

    #[test]
    fn a_failed_provider_logs_its_error_instead_of_keys() {
        let entries = log_entries(
            SearchCommand::Search,
            "attention",
            &filtered_options(),
            &search_results(),
            date(),
        );
        let semantic_scholar = &entries[1];
        assert_eq!(
            semantic_scholar.error.as_deref(),
            Some("request failed: rate limited")
        );
        assert_eq!(semantic_scholar.keys, None);
        assert_eq!(semantic_scholar.retrieved, None);
        assert_eq!(semantic_scholar.total, None);
        let json = serde_json::to_string(semantic_scholar).unwrap();
        assert!(!json.contains("\"keys\""), "{json}");
        assert!(!json.contains("\"retrieved\""), "{json}");
    }

    #[test]
    fn filters_are_recorded_with_how_each_provider_applied_them() {
        let entries = log_entries(
            SearchCommand::Search,
            "attention",
            &filtered_options(),
            &search_results(),
            date(),
        );
        let filter = |filter: &str, value: &str, applied: Option<&str>| LoggedFilter {
            filter: filter.to_string(),
            value: value.to_string(),
            applied: applied.map(str::to_string),
        };
        assert_eq!(
            entries[0].filters,
            [
                filter("year", "2015-2020", Some("server-side")),
                filter("type", "journal-article", Some("server-side")),
            ]
        );
        assert_eq!(
            entries[1].filters,
            [
                filter("year", "2015-2020", None),
                filter("type", "journal-article", None),
            ]
        );
        assert_eq!(
            entries[2].filters,
            [
                filter("year", "2015-2020", Some("post-retrieval")),
                filter("type", "journal-article", Some("not-applied")),
            ]
        );
    }

    #[test]
    fn a_citation_list_logs_its_seed_and_a_provider_that_doesnt_know_it() {
        let results: CitationResults = HashMap::from([
            (
                ProviderId::OpenAlex,
                Ok(SearchPage::new(Vec::new(), Some(0), None)),
            ),
            (
                ProviderId::SemanticScholar,
                Err(CitationError::SeedNotFound {
                    provider: ProviderId::SemanticScholar,
                    key: "DOI:10.1/a".into(),
                }),
            ),
        ]);
        let entries = log_entries(
            SearchCommand::CitedBy,
            "openalex:W1",
            &SearchOptions::default().with_limit(Limit::All),
            &results,
            date(),
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].seed.as_deref(), Some("openalex:W1"));
        assert_eq!(entries[0].query, None);
        assert_eq!(entries[0].keys, Some(Vec::new()));
        assert!(entries[1].error.as_deref().unwrap().contains("no record"));
        let json = serde_json::to_string(&entries[0]).unwrap();
        assert!(json.starts_with(r#"{"v":1,"command":"cited-by","seed":"openalex:W1","#));
        assert!(json.contains(r#""limit":"all""#), "{json}");
    }

    #[test]
    fn a_related_works_line_logs_its_seed_and_no_total() {
        let results: CitationResults = HashMap::from([(
            ProviderId::SemanticScholar,
            Ok(SearchPage::new(
                vec![work(ProviderId::SemanticScholar, "s1", Some("10.1/a"), "A")],
                None,
                Some(500),
            )),
        )]);
        let entries = entries_with_reasons(
            SearchCommand::Related,
            "vaswani2017",
            &SearchOptions::default(),
            &results,
            date(),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].seed.as_deref(), Some("vaswani2017"));
        assert_eq!(entries[0].query, None);
        assert_eq!(entries[0].total, None);
        assert_eq!(entries[0].retrieved, Some(1));
        assert_eq!(entries[0].reason, None);
        let line = serde_json::to_string(&entries[0]).unwrap();
        assert!(
            line.starts_with(r#"{"v":1,"command":"related","seed":"vaswani2017","#),
            "{line}"
        );
        assert!(line.contains(r#""total":null"#), "{line}");
        assert_eq!(
            serde_json::from_str::<SearchLogEntry>(&line).unwrap(),
            entries[0]
        );
    }

    #[test]
    fn a_failed_line_records_its_reason_next_to_the_error() {
        let results: CitationResults = HashMap::from([
            (
                ProviderId::OpenAlex,
                Err(CitationError::Provider(ProviderError::Request(
                    "timed out".into(),
                ))),
            ),
            (
                ProviderId::SemanticScholar,
                Err(CitationError::Provider(ProviderError::Withheld {
                    message: "Semantic Scholar withholds this paper's references".into(),
                })),
            ),
        ]);
        let entries = entries_with_reasons(
            SearchCommand::Refs,
            "lamport1978",
            &SearchOptions::default(),
            &results,
            date(),
        );
        let reasons: Vec<_> = entries.iter().map(|e| e.reason.as_deref()).collect();
        assert_eq!(reasons, [Some("failed"), Some("withheld")]);
        let json = serde_json::to_string(&entries[1]).unwrap();
        assert!(
            json.contains(
                r#""error":"Semantic Scholar withholds this paper's references","reason":"withheld""#
            ),
            "{json}"
        );

        // A search's failed line is `failed`; an answering line has none.
        let entries = entries_with_reasons(
            SearchCommand::Search,
            "attention",
            &filtered_options(),
            &search_results(),
            date(),
        );
        let reasons: Vec<_> = entries.iter().map(|e| e.reason.as_deref()).collect();
        assert_eq!(reasons, [None, Some("failed"), None]);
        // The generic `log_entries` can't tell, so records none.
        let json = serde_json::to_string(&log_entries(
            SearchCommand::Refs,
            "lamport1978",
            &SearchOptions::default(),
            &results,
            date(),
        ))
        .unwrap();
        assert!(!json.contains("reason"), "{json}");
    }

    #[test]
    fn a_line_round_trips() {
        let entries = log_entries(
            SearchCommand::SearchAuthor,
            "Lamport",
            &filtered_options(),
            &search_results(),
            date(),
        );
        for entry in entries {
            let line = serde_json::to_string(&entry).unwrap();
            assert_eq!(
                serde_json::from_str::<SearchLogEntry>(&line).unwrap(),
                entry
            );
        }
    }

    #[test]
    fn a_label_is_on_every_line_and_absent_without_one() {
        let labelled = log_entries(
            SearchCommand::Refs,
            "lamport1978",
            &filtered_options().with_label("R2-backward"),
            &search_results(),
            date(),
        );
        assert_eq!(labelled.len(), 3);
        for entry in &labelled {
            assert_eq!(entry.label.as_deref(), Some("R2-backward"));
            let line = serde_json::to_string(entry).unwrap();
            assert!(line.contains(r#""label":"R2-backward""#), "{line}");
        }

        let unlabelled = log_entries(
            SearchCommand::Refs,
            "lamport1978",
            &filtered_options(),
            &search_results(),
            date(),
        );
        for entry in &unlabelled {
            assert_eq!(entry.label, None);
            let line = serde_json::to_string(entry).unwrap();
            assert!(!line.contains("label"), "{line}");
        }
    }

    #[test]
    fn a_line_written_before_labels_reads_as_unlabelled() {
        let line = r#"{"v":1,"command":"search","query":"q","provider":"openalex","date":"2026-01-01T00:00:00Z","limit":50,"filters":[],"total":null,"retrieved":0,"keys":[],"merge":{"before":0,"after":0,"unmatched":0}}"#;
        let entry: SearchLogEntry = serde_json::from_str(line).unwrap();
        assert_eq!(entry.label, None);
        assert_eq!(entry.reference, None);
    }

    #[test]
    fn a_manual_add_logs_one_record_under_its_label() {
        let id: CandidateId = "crossref:10.1/A".parse().unwrap();
        let added = work(ProviderId::Crossref, "10.1/A", Some("10.1/A"), "Attention");
        let entry = manual_log_entry(&id, &added, "seeds", date());
        assert_eq!(entry.command, SearchCommand::Manual);
        assert_eq!(entry.reference.as_deref(), Some("crossref:10.1/A"));
        assert_eq!((entry.query.as_ref(), entry.seed.as_ref()), (None, None));
        assert_eq!(entry.provider, "crossref");
        assert_eq!(entry.label.as_deref(), Some("seeds"));
        assert_eq!(entry.date, "2017-06-12T00:00:00Z");
        assert_eq!(entry.retrieved, Some(1));
        assert_eq!(
            entry.keys,
            Some(vec![vec![
                "doi:10.1/a".to_string(),
                "title:attention|2017".to_string()
            ]])
        );
        assert_eq!(
            entry.merge,
            MergeCounts {
                before: 1,
                after: 1,
                unmatched: 0
            }
        );
        let line = serde_json::to_string(&entry).unwrap();
        assert!(line.contains(r#""command":"manual""#), "{line}");
        assert!(line.contains(r#""reference":"crossref:10.1/A""#), "{line}");
        assert_eq!(
            serde_json::from_str::<SearchLogEntry>(&line).unwrap(),
            entry
        );

        let keyless = work(ProviderId::OpenAlex, "W1", None, "");
        let id: CandidateId = "openalex:W1".parse().unwrap();
        let entry = manual_log_entry(&id, &keyless, "seeds", date());
        assert_eq!(entry.keys, Some(vec![vec![]]));
        assert_eq!(entry.merge.unmatched, 1);
    }

    #[test]
    fn a_manual_add_appends_one_line_with_a_run_id() {
        let root = scratch_library("manual");
        let id: CandidateId = "crossref:10.1/A".parse().unwrap();
        let added = work(ProviderId::Crossref, "10.1/A", Some("10.1/A"), "Attention");
        let outcome = append_to_library(&root, || {
            vec![manual_log_entry(&id, &added, "seeds", date())]
        });
        assert!(matches!(outcome, LogOutcome::Written(_)));
        let text = std::fs::read_to_string(search_log_path(&root)).unwrap();
        let lines: Vec<SearchLogEntry> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].run.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    fn scratch_library(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("pax-search-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        nix::init_library(&root).unwrap();
        root
    }

    #[test]
    fn appending_twice_keeps_the_first_runs_lines() {
        let root = scratch_library("append");
        let path = search_log_path(&root);
        assert!(!path.exists());

        let results = search_results();
        let first = log_results(
            &root,
            SearchCommand::Search,
            "first",
            &filtered_options(),
            &results,
        );
        assert!(matches!(first, LogOutcome::Written(ref p) if *p == path));
        let after_first = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after_first.lines().count(), 3);

        let second = log_results(
            &root,
            SearchCommand::Search,
            "second",
            &SearchOptions::default(),
            &results,
        );
        assert!(matches!(second, LogOutcome::Written(_)));
        let after_second = std::fs::read_to_string(&path).unwrap();
        assert!(after_second.starts_with(&after_first));
        let queries: Vec<String> = after_second
            .lines()
            .map(|line| {
                serde_json::from_str::<SearchLogEntry>(line)
                    .unwrap()
                    .query
                    .unwrap()
            })
            .collect();
        assert_eq!(
            queries,
            ["first", "first", "first", "second", "second", "second"]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nothing_is_logged_outside_a_library() {
        let root = std::env::temp_dir().join(format!("pax-search-log-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("research")).unwrap();
        let outcome = log_results(
            &root,
            SearchCommand::Search,
            "q",
            &SearchOptions::default(),
            &search_results(),
        );
        assert!(matches!(outcome, LogOutcome::NoLibrary));
        assert!(!search_log_path(&root).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_write_is_reported() {
        let root = scratch_library("unwritable");
        // A directory where the file should be: opening it for append fails.
        std::fs::create_dir(search_log_path(&root)).unwrap();
        let outcome = log_results(
            &root,
            SearchCommand::Search,
            "q",
            &SearchOptions::default(),
            &search_results(),
        );
        assert!(matches!(outcome, LogOutcome::Failed(_)));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn seed_lists(doi: &str) -> CitationResults {
        HashMap::from([
            (
                ProviderId::OpenAlex,
                Ok(SearchPage::new(
                    vec![work(ProviderId::OpenAlex, "W1", Some(doi), "A")],
                    Some(1),
                    None,
                )),
            ),
            (
                ProviderId::SemanticScholar,
                Err(CitationError::Provider(ProviderError::Withheld {
                    message: "withheld".into(),
                })),
            ),
        ])
    }

    fn seed(name: &str, results: Result<CitationResults, PaxError>) -> crate::SeedResult {
        crate::SeedResult {
            seed: name.to_string(),
            same_as: None,
            results,
        }
    }

    #[test]
    fn several_seeds_log_exactly_what_each_would_on_its_own() {
        let options = SearchOptions::default();
        let results = SeedResults {
            seeds: vec![
                seed("a2020", Ok(seed_lists("10.1/a"))),
                seed("nosuchkey", Err(PaxError::NoSuchPaper("nosuchkey".into()))),
                seed("openalex:W9", Ok(seed_lists("10.1/b"))),
            ],
        };
        let together = seed_log_entries(SearchCommand::Refs, &options, &results, date());
        let one_at_a_time: Vec<SearchLogEntry> = [("a2020", "10.1/a"), ("openalex:W9", "10.1/b")]
            .into_iter()
            .flat_map(|(name, doi)| {
                entries_with_reasons(
                    SearchCommand::Refs,
                    name,
                    &options,
                    &seed_lists(doi),
                    date(),
                )
            })
            .collect();
        // One line per seed per provider, each seed with its own merge
        // counts; the invalid seed logs nothing.
        assert_eq!(together.len(), 4);
        assert_eq!(together, one_at_a_time);
        assert_eq!(together[1].reason.as_deref(), Some("withheld"));
    }

    #[test]
    fn one_command_s_lines_share_a_run_id_no_other_has() {
        let root = scratch_library("run");
        let options = SearchOptions::default();
        let results = SeedResults {
            seeds: vec![
                seed("a2020", Ok(seed_lists("10.1/a"))),
                seed("b2021", Ok(seed_lists("10.1/b"))),
            ],
        };
        for _ in 0..2 {
            let outcome = append_to_library(&root, || {
                seed_log_entries(SearchCommand::CitedBy, &options, &results, date())
            });
            assert!(matches!(outcome, LogOutcome::Written(_)));
        }
        let runs: Vec<String> = std::fs::read_to_string(search_log_path(&root))
            .unwrap()
            .lines()
            .map(|line| {
                let entry: SearchLogEntry = serde_json::from_str(line).unwrap();
                entry.run.unwrap()
            })
            .collect();
        assert_eq!(runs.len(), 8);
        assert!(runs[..4].iter().all(|run| *run == runs[0]));
        assert!(runs[4..].iter().all(|run| *run == runs[4]));
        assert_ne!(runs[0], runs[4]);

        // Every seed invalid: nothing to log, and the file is untouched.
        let invalid = SeedResults {
            seeds: vec![seed("nosuchkey", Err(PaxError::NoSuchPaper("x".into())))],
        };
        let outcome = append_to_library(&root, || {
            seed_log_entries(SearchCommand::Refs, &options, &invalid, date())
        });
        assert!(matches!(outcome, LogOutcome::Nothing));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_line_without_a_run_id_still_reads() {
        let line = r#"{"v":1,"command":"refs","seed":"a2020","provider":"openalex","date":"2026-01-01T00:00:00Z","limit":50,"filters":[],"total":0,"retrieved":0,"keys":[],"merge":{"before":0,"after":0,"unmatched":0}}"#;
        let entry: SearchLogEntry = serde_json::from_str(line).unwrap();
        assert_eq!(entry.run, None);
        assert_eq!(serde_json::to_string(&entry).unwrap(), line);
    }
}
