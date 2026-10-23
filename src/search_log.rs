//! The search log, `research/search-log.jsonl`: the library's provenance.
//! Every database search a review runs — `search`, `search --author`,
//! `refs`, `cited-by` — appends one JSON line per provider asked, recording
//! what PRISMA 2020 asks a review to report (each source, the date it was
//! searched, the full search string with its filters and limits) plus the
//! identity key of every record retrieved, so duplicates can be counted
//! across the whole review, not just within one command.
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
use std::time::SystemTime;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::PaxError;
use crate::identity::identity_keys;
use crate::merge::{PROVIDER_ORDER, merge_works};
use crate::provider::{
    FilterApplication, FilterKind, Filters, Limit, ProviderError, ProviderId, SearchOptions,
    SearchPage,
};
use crate::{CitationResults, Config, nix};

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
}

impl SearchCommand {
    /// Whether the command's input is a seed paper rather than a query.
    fn takes_seed(self) -> bool {
        matches!(self, SearchCommand::Refs | SearchCommand::CitedBy)
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
    /// `refs` / `cited-by`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<String>,
    /// The provider's tag, e.g. `openalex`.
    pub provider: String,
    /// When the search ran: ISO 8601 in UTC, to the second. Every line of one
    /// command carries the same date.
    pub date: String,
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
    /// The command's merge counts across every provider that answered; the
    /// same on every line of one command.
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
/// [`PROVIDER_ORDER`]. `input` is the query, author name or seed, as
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
        PROVIDER_ORDER
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

    PROVIDER_ORDER
        .iter()
        .filter_map(|provider| Some((*provider, results.get(provider)?)))
        .map(|(provider, result)| {
            let mut entry = SearchLogEntry {
                v: SEARCH_LOG_VERSION,
                command,
                query: query.clone(),
                seed: seed.clone(),
                provider: provider.to_string(),
                date: date.clone(),
                limit: options.limit.into(),
                filters: Vec::new(),
                total: None,
                retrieved: None,
                keys: None,
                error: None,
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
/// `*_logged` function.
pub fn log_results<E: fmt::Display>(
    root: &Path,
    command: SearchCommand,
    input: &str,
    options: &SearchOptions,
    results: &HashMap<ProviderId, Result<SearchPage, E>>,
) -> LogOutcome {
    let Some(log) = SearchLog::for_library(root) else {
        return LogOutcome::NoLibrary;
    };
    let entries = log_entries(command, input, options, results, SystemTime::now());
    match log.append(&entries) {
        Ok(()) => LogOutcome::Written(log.path),
        Err(e) => LogOutcome::Failed(e),
    }
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
    let log = log_results(root, SearchCommand::Search, query, options, &results);
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
    let log = log_results(root, SearchCommand::SearchAuthor, author, options, &results);
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
    let log = log_results(root, SearchCommand::Refs, seed, options, &results);
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
    let log = log_results(root, SearchCommand::CitedBy, seed, options, &results);
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
}
