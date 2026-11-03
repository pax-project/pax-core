//! Owns how the CLI renders output. Every command handler goes through a
//! `Sink` instead of calling `println!` directly, so a future alternate
//! output mode (e.g. JSON, for an external adapter UI) is a new `Sink` impl
//! rather than a rewrite of every command handler.
//!
//! Results go to stdout; errors and warnings go to stderr, so redirecting
//! stdout (`pax search … > results.txt`) captures the results alone.
//!
//! Two sinks: [`TextSink`], for people, and [`JsonSink`] (`--json`), which
//! prints each command's result as one JSON document, a
//! [`pax_core::output`] shape, and writes stderr as one JSON object per
//! line: `{"error": {"code", "message"}}` for the failure that ends a
//! command, `{"warning": {"message"}}` (with `provider` when one is
//! concerned) for anything else.

use std::collections::HashMap;
use std::path::Path;

use pax_core::output;
use pax_core::{
    ArtifactCheck, CandidateWork, CheckStatus, Exclusion, FetchOutcome, FilterApplication,
    IncompleteReason, LabelCounts, LibraryIndex, LogOutcome, MergeReport, Paper, PaperRef,
    PaxError, ProviderError, ProviderId, SearchPage, SeedMergeReport, SeedResults, ShowResult,
    Stats,
};
use serde::Serialize;

use crate::exit::{self, Code, Failure};

/// A search's results, per provider.
pub type SearchResults = HashMap<ProviderId, Result<SearchPage, ProviderError>>;

/// `search --doi`'s records, per provider.
pub type DoiResults = HashMap<ProviderId, Result<Vec<CandidateWork>, ProviderError>>;

pub trait Sink {
    fn message(&mut self, message: &str);
    /// Why the command failed, exiting with `code`.
    fn failure(&mut self, code: Code, message: &str);
    /// An error that doesn't end the command, e.g. one seed or paper among
    /// several failing.
    fn error(&mut self, message: &str);
    /// A note about the results that isn't itself a result, e.g. a filter
    /// a provider couldn't apply.
    fn warning(&mut self, message: &str);
    /// A note about a provider, e.g. that it has no record of the seed or
    /// DOI asked about while another answered, or how long pacing it takes.
    fn provider_note(&mut self, provider: ProviderId, message: &str);
    /// A search's per-provider results, merged across providers and flagged
    /// against `index`; with `abstracts`, each paper's abstract.
    fn search(
        &mut self,
        results: &SearchResults,
        index: &LibraryIndex,
        log: &LogOutcome,
        abstracts: bool,
    );
    /// `search --doi`'s records of `doi`, per provider.
    fn doi(&mut self, doi: &str, results: &DoiResults, index: &LibraryIndex);
    /// A `refs`, `cited-by` or `related` lookup over one or more seeds.
    fn citations(
        &mut self,
        results: &SeedResults,
        lists: output::Lists,
        index: &LibraryIndex,
        log: &LogOutcome,
        abstracts: bool,
    );
    /// What `show` found: a declared paper, an excluded paper's record (given
    /// its alias), or a provider's record, flagged against `index`.
    fn show(&mut self, shown: &ShowResult, index: &LibraryIndex);
    /// `search --local`'s matches.
    fn papers(&mut self, papers: &[Paper]);
    /// `list`'s papers, those matching its filters; `library_empty` says
    /// whether the library has none at all.
    fn list(&mut self, papers: &[Paper], library_empty: bool);
    /// A library created at `root`.
    fn initialized(&mut self, root: &Path);
    /// A paper added as `key`; `log`, when the add was logged under a label.
    fn added(&mut self, key: &PaperRef, log: Option<&LogOutcome>);
    /// The paper `remove` removed.
    fn removed(&mut self, paper: &Paper);
    /// The exclusion `exclude` recorded.
    fn excluded(&mut self, exclusion: &Exclusion);
    /// The exclusion `unexclude` removed.
    fn unexcluded(&mut self, exclusion: &Exclusion);
    /// The paper `edit` edited, as `name` named it, as it is now.
    fn edited(&mut self, name: &str, paper: &Paper);
    fn export(&mut self, bibtex: &str);
    fn fetched(&mut self, citation_key: &str, outcome: &FetchOutcome);
    fn checked(&mut self, reports: &[ArtifactCheck]);
    /// The PRISMA flow-diagram counts; with `by_label`, also the per-label
    /// counts (which the JSON always has).
    fn stats(&mut self, stats: &Stats, by_label: bool);
}

/// `println!` that ends the process quietly with exit 0 when stdout's reader
/// has stopped reading (`pax list | head`), as other Unix tools do, instead
/// of panicking on the broken pipe.
macro_rules! out {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        if let Err(e) = writeln!(std::io::stdout(), $($arg)*) {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            }
            panic!("failed printing to stdout: {e}");
        }
    }};
}

pub struct TextSink;

/// The pieces [`TextSink`]'s search and citation output is built from.
impl TextSink {
    /// A provider that failed while the others may still have answered.
    fn provider_error(&mut self, provider: ProviderId, message: &str) {
        eprintln!("Error: {provider}: {message}");
    }

    /// Reports a provider that returned no record for a DOI. One that has no
    /// record of it has answered, so while another provider returned one,
    /// that's a note rather than an error.
    fn provider_failed(&mut self, provider: ProviderId, error: &ProviderError, another: bool) {
        let failure = Failure::Provider(error);
        if another && exit::is_answer(failure) {
            self.provider_note(provider, &failure.to_string());
        } else {
            self.provider_error(provider, &failure.to_string());
        }
    }

    fn provider_header(&mut self, provider: ProviderId) {
        out!("{provider}:");
    }

    fn page_header(&mut self, provider: ProviderId, page: &SearchPage) {
        self.counts(provider, page, "");
    }

    fn citation_header(
        &mut self,
        provider: ProviderId,
        page: Option<&SearchPage>,
        reason: Option<IncompleteReason>,
    ) {
        let mark = reason
            .map(|reason| format!(" [{reason}]"))
            .unwrap_or_default();
        match page {
            Some(page) => self.counts(provider, page, &mark),
            None => out!("{provider}: no list{mark}"),
        }
    }

    fn candidates(&mut self, candidates: &[CandidateWork], index: &LibraryIndex) {
        for work in candidates {
            print_work(work, &work.id.to_string(), &Marks::of([work], index), None);
        }
    }

    fn merged(
        &mut self,
        report: &MergeReport,
        index: &LibraryIndex,
        language_unknown: &dyn Fn(&CandidateWork) -> bool,
        abstracts: bool,
    ) {
        out!(
            "merged: {} → {} ({} duplicates, {} unmatched)",
            report.before,
            report.after,
            report.duplicates(),
            report.unmatched
        );
        for merged in &report.works {
            let ids: Vec<String> = merged.ids().iter().map(|id| id.to_string()).collect();
            let marks = Marks {
                language_unknown: merged.records().all(language_unknown),
                ..Marks::of(merged.records(), index)
            };
            print_work(&merged.work, &ids.join(" · "), &marks, None);
            if abstracts {
                print_abstract(merged.abstract_text());
            }
        }
    }

    fn seed_header(&mut self, seed: &str, same_as: Option<&str>) {
        match same_as {
            Some(other) => out!("seed {seed}: same paper as {other}"),
            None => out!("seed {seed}:"),
        }
    }

    fn merged_seeds(
        &mut self,
        report: &SeedMergeReport,
        seeds: &[&str],
        index: &LibraryIndex,
        language_unknown: &dyn Fn(&CandidateWork) -> bool,
        abstracts: bool,
    ) {
        out!(
            "merged: {} → {} ({} duplicates, {} unmatched)",
            report.before,
            report.after,
            report.duplicates(),
            report.unmatched
        );
        for row in &report.works {
            let merged = &row.merged;
            let ids: Vec<String> = merged.ids().iter().map(|id| id.to_string()).collect();
            let marks = Marks {
                language_unknown: merged.records().all(language_unknown),
                ..Marks::of(merged.records(), index)
            };
            let names: Vec<&str> = row.seeds().into_iter().map(|i| seeds[i]).collect();
            let line = format!("seeds: {}", names.join(", "));
            print_work(&merged.work, &ids.join(" · "), &marks, Some(&line));
            if abstracts {
                print_abstract(merged.abstract_text());
            }
        }
    }
}

impl Sink for TextSink {
    fn message(&mut self, message: &str) {
        out!("{message}");
    }

    fn failure(&mut self, _code: Code, message: &str) {
        self.error(message);
    }

    fn error(&mut self, message: &str) {
        eprintln!("Error: {message}");
    }

    fn warning(&mut self, message: &str) {
        eprintln!("Warning: {message}");
    }

    fn provider_note(&mut self, provider: ProviderId, message: &str) {
        eprintln!("Note: {provider}: {message}");
    }

    fn search(
        &mut self,
        results: &SearchResults,
        index: &LibraryIndex,
        _log: &LogOutcome,
        abstracts: bool,
    ) {
        for &provider in pax_core::merge::MERGE_ORDER {
            match results.get(&provider) {
                Some(Ok(page)) => self.page_header(provider, page),
                Some(Err(e)) => self.provider_error(provider, &e.to_string()),
                None => {}
            }
        }
        let language_unknown = |work: &CandidateWork| {
            results
                .values()
                .flatten()
                .any(|page| page.is_language_unknown(work))
        };
        self.merged(
            &pax_core::merge_pages(results),
            index,
            &language_unknown,
            abstracts,
        );
    }

    fn doi(&mut self, _doi: &str, results: &DoiResults, index: &LibraryIndex) {
        let listed = results.values().any(Result::is_ok);
        // arXiv and dblp have no DOI lookup.
        for provider in [
            ProviderId::OpenAlex,
            ProviderId::Crossref,
            ProviderId::SemanticScholar,
        ] {
            match results.get(&provider) {
                Some(Ok(works)) => {
                    self.provider_header(provider);
                    self.candidates(works, index);
                }
                Some(Err(e)) => self.provider_failed(provider, e, listed),
                None => {}
            }
        }
    }

    /// Each seed's provider counts, marked with why a list is incomplete if
    /// it is, then one list merged across every seed and provider, the way
    /// `search` prints it. With one seed the output is that seed's alone;
    /// with several, each seed's counts sit under a `seed <seed>:` line and
    /// each merged row names the seeds that returned it.
    ///
    /// For related works, only a missing list is marked: a related list
    /// never has a total, so it's never complete, and `truncated` (the limit
    /// stopping short of a total) would say something untrue; its count line
    /// already says the total isn't reported.
    fn citations(
        &mut self,
        results: &SeedResults,
        lists: output::Lists,
        index: &LibraryIndex,
        _log: &LogOutcome,
        abstracts: bool,
    ) {
        let several = results.seeds.len() > 1;
        for seed in &results.seeds {
            // With several seeds, stderr lines name theirs.
            let of_seed = |message: String| {
                if several {
                    format!("{message} (seed {})", seed.seed)
                } else {
                    message
                }
            };
            if several {
                let same_as = seed.same_as.map(|i| results.seeds[i].seed.as_str());
                self.seed_header(&seed.seed, same_as);
            }
            let by_provider = match &seed.results {
                Ok(by_provider) => by_provider,
                Err(e) => {
                    self.error(&of_seed(e.to_string()));
                    continue;
                }
            };
            for provider in pax_core::CITATION_PROVIDERS {
                let Some(result) = by_provider.get(&provider) else {
                    continue;
                };
                let reason = pax_core::incomplete_reason(result)
                    .filter(|reason| lists == output::Lists::Citations || reason.is_missing());
                self.citation_header(provider, result.as_ref().ok(), reason);
                if let Err(e) = result {
                    self.provider_error(provider, &of_seed(e.to_string()));
                }
            }
        }
        if !results.any_listed() {
            return;
        }
        let language_unknown = |work: &CandidateWork| {
            results
                .seeds
                .iter()
                .filter_map(|seed| seed.results.as_ref().ok())
                .flat_map(|lists| lists.values().flatten())
                .any(|page| page.is_language_unknown(work))
        };
        if several {
            let seeds: Vec<&str> = results.seeds.iter().map(|s| s.seed.as_str()).collect();
            self.merged_seeds(
                &results.merge(),
                &seeds,
                index,
                &language_unknown,
                abstracts,
            );
        } else if let Some(Ok(by_provider)) = results.seeds.first().map(|seed| &seed.results) {
            self.merged(
                &pax_core::merge_pages(by_provider),
                index,
                &language_unknown,
                abstracts,
            );
        }
    }

    fn show(&mut self, shown: &ShowResult, _index: &LibraryIndex) {
        match shown {
            ShowResult::Declared(paper) => print_paper(paper),
            ShowResult::Excluded(exclusion) => print_exclusion(exclusion),
            ShowResult::Candidate(work) => print_candidate(work),
        }
    }

    fn papers(&mut self, papers: &[Paper]) {
        for paper in papers {
            match &paper.local.alias {
                Some(alias) => out!("{alias}  {}", paper.local.citation_key),
                None => out!("{}", paper.local.citation_key),
            }
            out!("\t{}", paper.identity.title);
            if !paper.identity.authors.is_empty() {
                out!("\t{}", paper.identity.authors.join(", "));
            }
            if let Some(year) = paper.identity.year {
                out!("\t{}", year);
            }
        }
    }

    fn initialized(&mut self, _root: &Path) {
        self.message("Library created");
    }

    fn added(&mut self, key: &PaperRef, _log: Option<&LogOutcome>) {
        self.message(&format!("Added {}", key.0));
    }

    fn removed(&mut self, paper: &Paper) {
        self.message(&format!("Removed {}", paper.local.citation_key));
        if let Some(alias) = &paper.local.alias {
            self.message(&freed(alias));
        }
    }

    fn excluded(&mut self, exclusion: &Exclusion) {
        let mut decision = exclusion.stage.to_string();
        if let Some(criterion) = &exclusion.criterion {
            decision.push_str(&format!(", criterion {criterion}"));
        }
        if let Some(round) = &exclusion.round {
            decision.push_str(&format!(", round {round}"));
        }
        if let Some(alias) = &exclusion.alias {
            decision.push_str(&format!(", alias {alias}"));
        }
        self.message(&format!(
            "Excluded {} ({decision}): {}",
            exclusion.key, exclusion.title
        ));
    }

    fn unexcluded(&mut self, exclusion: &Exclusion) {
        self.message(&format!(
            "Unexcluded {} ({}, {:?}): {}",
            exclusion.key, exclusion.stage, exclusion.reason, exclusion.title
        ));
        if let Some(alias) = &exclusion.alias {
            self.message(&freed(alias));
        }
    }

    fn edited(&mut self, name: &str, _paper: &Paper) {
        self.message(&format!("Updated {name}"));
    }

    fn list(&mut self, papers: &[Paper], library_empty: bool) {
        if library_empty {
            self.message("Library is empty");
        } else if papers.is_empty() {
            self.message("No papers match the given filters");
        } else {
            self.papers(papers);
        }
    }

    fn export(&mut self, bibtex: &str) {
        if !bibtex.is_empty() {
            out!("{bibtex}");
        }
    }

    fn fetched(&mut self, citation_key: &str, outcome: &FetchOutcome) {
        match outcome {
            FetchOutcome::AlreadyFetched { hash } => {
                out!("Already fetched {citation_key} ({hash})");
            }
            FetchOutcome::Fetched { hash } => {
                out!("Fetched {citation_key} ({hash})");
            }
        }
    }

    fn checked(&mut self, reports: &[ArtifactCheck]) {
        let (mut reproducible, mut not_fetched, mut mismatched, mut errored) = (0, 0, 0, 0);
        let mut missing = 0;
        for report in reports {
            if let Some(PaxError::LocalArtifactMissing(_)) = &report.error {
                missing += 1;
                out!(
                    "{}: MISSING — added from a local file that isn't in this Nix store; provide it again with `pax fetch {} --file <path>`",
                    report.citation_key,
                    report.citation_key
                );
                continue;
            }
            match &report.status {
                CheckStatus::NotFetched => {
                    not_fetched += 1;
                    out!("{}: not fetched", report.citation_key);
                }
                CheckStatus::Reproducible => {
                    reproducible += 1;
                    out!("{}: reproducible", report.citation_key);
                }
                CheckStatus::Mismatch { expected, actual } => {
                    mismatched += 1;
                    out!(
                        "{}: MISMATCH (expected {expected}, got {actual})",
                        report.citation_key
                    );
                }
                CheckStatus::Error(message) => {
                    errored += 1;
                    out!("{}: ERROR — {message}", report.citation_key);
                }
            }
        }
        let missing = if missing > 0 {
            format!(", {missing} missing")
        } else {
            String::new()
        };
        out!(
            "{} checked: {reproducible} reproducible, {not_fetched} not fetched, {mismatched} mismatched, {errored} errored{missing}",
            reports.len()
        );
    }

    fn stats(&mut self, stats: &Stats, by_label: bool) {
        for line in stats_lines(stats) {
            out!("{line}");
        }
        if stats.skipped_log_lines > 0 {
            self.warning(&format!(
                "skipped {} search-log line(s) whose provider failed, so retrieved no records",
                stats.skipped_log_lines
            ));
        }
        if stats.decided_outside_log > 0 {
            self.warning(&format!(
                "{} declared or excluded paper(s) match no search-log record: counted in their box, but not among the records screened",
                stats.decided_outside_log
            ));
        }
        if by_label && !stats.by_label.is_empty() {
            out!();
            out!("By label");
            for line in label_lines(&stats.by_label) {
                out!("{line}");
            }
        }
    }
}

/// The hint `remove` and `unexclude` print when the paper had an alias.
fn freed(alias: &str) -> String {
    format!(
        "Freed alias {alias}; `edit --alias {alias}` or `exclude --alias {alias}` takes it again"
    )
}

/// A provider's record, for `show`.
fn print_candidate(candidate: &CandidateWork) {
    out!("Title:      {}", candidate.title);
    out!("Authors:    {}", candidate.authors.join(", "));
    out!("Published:  {}", candidate.publish_date);
    out!(
        "DOI:        {}",
        candidate.doi.as_deref().unwrap_or("(no doi)")
    );
    out!(
        "Venue:      {}",
        candidate.venue.as_deref().unwrap_or("(no venue)")
    );
    out!(
        "PDF source: {}",
        candidate.pdf_url.as_deref().unwrap_or("(none found)")
    );
    if let Some(abstract_text) = &candidate.abstract_text {
        out!("Abstract:   {abstract_text}");
    }
    out!("Reference:  {}", candidate.id);
}

/// A declared paper, for `show`.
fn print_paper(paper: &Paper) {
    out!("Title:        {}", paper.identity.title);
    out!("Authors:      {}", paper.identity.authors.join(", "));
    if let Some(year) = paper.identity.year {
        out!("Year:         {}", year);
    }
    out!(
        "DOI:          {}",
        paper.identity.doi.as_deref().unwrap_or("(no doi)")
    );
    out!(
        "Venue:        {}",
        paper.identity.venue.as_deref().unwrap_or("(no venue)")
    );
    out!("Citation key: {}", paper.local.citation_key);
    if let Some(alias) = &paper.local.alias {
        out!("Alias:        {alias}");
    }
    out!(
        "PDF source:   {}",
        paper
            .artifact
            .source_url
            .as_deref()
            .unwrap_or("(not resolved)")
    );
    out!(
        "Artifact:     {}",
        if paper.artifact.hash.is_some() {
            "Fetched"
        } else {
            "Not fetched"
        }
    );
    if !paper.local.tags.is_empty() {
        out!("Tags:         {}", paper.local.tags.join(", "));
    }
    if let Some(notes) = &paper.local.notes {
        out!("Notes:        {}", notes);
    }
}

/// An excluded paper's record, for `show` given its alias.
fn print_exclusion(exclusion: &Exclusion) {
    out!("Excluded:     {}", exclusion.key);
    out!("Title:        {}", exclusion.title);
    if let Some(year) = exclusion.year {
        out!("Year:         {year}");
    }
    out!("Stage:        {}", exclusion.stage);
    out!("Reason:       {}", exclusion.reason);
    if let Some(criterion) = &exclusion.criterion {
        out!("Criterion:    {criterion}");
    }
    if let Some(round) = &exclusion.round {
        out!("Round:        {round}");
    }
    if let Some(alias) = &exclusion.alias {
        out!("Alias:        {alias}");
    }
}

impl TextSink {
    /// A provider's `retrieved N of M` line, ending in `mark`, then the
    /// warnings about what the page leaves out.
    fn counts(&mut self, provider: ProviderId, page: &SearchPage, mark: &str) {
        let retrieved = page.retrieved();
        let Some(total) = page.total else {
            out!("{provider}: retrieved {retrieved} (total not reported){mark}");
            return;
        };
        let scanned = page.scanned;
        let skipped = scanned.saturating_sub(retrieved as u64);
        out!("{provider}: retrieved {retrieved} of {total}{mark}");
        if page.is_complete() {
            if skipped > 0 && !page.has_post_retrieval_filter() {
                // e.g. references Semantic Scholar couldn't match to a paper.
                self.warning(&format!(
                    "{provider}: {skipped} entries without a {provider} record skipped"
                ));
            }
        } else if let Some(max) = page.max_retrievable.filter(|&max| scanned >= max) {
            self.warning(&format!("{provider} serves at most {max} per query"));
        } else {
            self.warning(&format!(
                "{provider}: more results available; use --limit N or --all for more"
            ));
        }
        for filter in &page.filters {
            match filter.application {
                FilterApplication::PostRetrieval => self.warning(&format!(
                    "{provider}: {} filter applied post-retrieval to the {scanned} records fetched; the total counts records before it",
                    filter.kind
                )),
                FilterApplication::NotApplied => self.warning(&format!(
                    "{provider}: {} filter not applied: {provider} has no such data, so these results ignore it",
                    filter.kind
                )),
                _ => {}
            }
        }
        let unknown = page.language_unknown.len();
        if unknown > 0 {
            self.warning(&format!(
                "{provider}: {unknown} records have no language, so the language filter couldn't check them; kept and marked [language unknown]"
            ));
        }
    }
}

/// The marks after a row's title.
#[derive(Default)]
struct Marks {
    in_library: bool,
    excluded: bool,
    language_unknown: bool,
}

impl Marks {
    /// Whether any of `records` — a row's records — is declared or excluded.
    fn of<'a>(records: impl IntoIterator<Item = &'a CandidateWork>, index: &LibraryIndex) -> Self {
        let mut marks = Marks::default();
        for work in records {
            marks.in_library |= index.contains(work);
            marks.excluded |= index.is_excluded(work);
        }
        marks
    }
}

/// One result row: the title with any marks, then its details. `ids` is the
/// record's reference, or every merged record's reference for a merged row;
/// `extra`, a line printed under them, e.g. the seeds that returned the row.
fn print_work(work: &CandidateWork, ids: &str, marks: &Marks, extra: Option<&str>) {
    let flag = |set: bool, text: &'static str| if set { text } else { "" };
    out!(
        "\t{}{}{}{}",
        work.title,
        flag(marks.in_library, " [in library]"),
        flag(marks.excluded, " [excluded]"),
        flag(marks.language_unknown, " [language unknown]"),
    );
    out!("\t\t{ids}");
    if let Some(extra) = extra {
        out!("\t\t{extra}");
    }
    if !work.authors.is_empty() {
        out!("\t\t{}", work.authors.join(", "));
    }
    out!("\t\t{}", work.publish_date);
    out!("\t\t{}", work.doi.as_deref().unwrap_or("(no doi)"));
    out!("\t\t{}", work.venue.as_deref().unwrap_or("(no venue)"));
    out!(
        "\t\tPDF: {}",
        if work.pdf_url.is_some() { "✓" } else { "✗" }
    );
}

/// The per-label table, indented under its heading: one row per label in
/// log order, the labels left-aligned and each count right-aligned under
/// its column's name. Lines without a label are `(unlabelled)`.
fn label_lines(labels: &[LabelCounts]) -> Vec<String> {
    const COLUMNS: [&str; 4] = ["Identified", "Distinct", "New", "Duplicates"];
    let names: Vec<&str> = labels
        .iter()
        .map(|counts| counts.label.as_deref().unwrap_or("(unlabelled)"))
        .collect();
    let rows: Vec<[String; 4]> = labels
        .iter()
        .map(|counts| {
            [
                counts.identified,
                counts.distinct,
                counts.new_distinct,
                counts.duplicates,
            ]
            .map(|count| count.to_string())
        })
        .collect();
    let label_width = names
        .iter()
        .map(|name| name.chars().count())
        .chain(["Label".len()])
        .max()
        .unwrap_or(0);
    let widths: Vec<usize> = (0..COLUMNS.len())
        .map(|column| {
            rows.iter()
                .map(|row| row[column].len())
                .chain([COLUMNS[column].len()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |label: &str, cells: [&str; 4]| {
        let pad = label_width - label.chars().count();
        let mut line = format!("  {label}{}", " ".repeat(pad));
        for (cell, width) in cells.iter().zip(&widths) {
            line.push_str(&format!("  {cell:>width$}"));
        }
        line
    };
    std::iter::once(line("Label", COLUMNS))
        .chain(
            names
                .iter()
                .zip(&rows)
                .map(|(name, row)| line(name, [&row[0], &row[1], &row[2], &row[3]])),
        )
        .collect()
}

/// The width a row's abstract is wrapped to, not counting its indent.
const ABSTRACT_WIDTH: usize = 80;

/// A row's abstract, under its details at the same indent, wrapped; or a
/// placeholder when no record had one, like `(no doi)`.
fn print_abstract(abstract_text: Option<&str>) {
    let text = abstract_text.unwrap_or("(no abstract)");
    for line in abstract_lines(&format!("Abstract: {text}"), ABSTRACT_WIDTH) {
        out!("\t\t{line}");
    }
}

/// `text` wrapped to lines of at most `width` characters (a longer word
/// gets a line of its own). Whitespace within a paragraph is collapsed, so
/// hard line breaks in an abstract (arXiv's) don't survive; paragraphs,
/// separated by a blank line, are kept apart by an empty line.
fn abstract_lines(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split("\n\n").filter(|p| !p.trim().is_empty()) {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let needed = if line.is_empty() { 0 } else { 1 } + word.chars().count();
            if !line.is_empty() && line.chars().count() + needed > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

/// The PRISMA 2020 flow-diagram boxes, one per line: a phase heading, then
/// each box's label with its count, the counts right-aligned in one column.
/// Per-provider and per-reason breakdowns are indented under their box.
fn stats_lines(stats: &Stats) -> Vec<String> {
    enum Row<'a> {
        Phase(&'a str),
        Count(usize, String, usize),
    }
    let mut rows = Vec::new();
    // Listed only once some exclusion records a criterion, so a review that
    // doesn't use them sees no rows of "no criterion".
    let criteria = &stats.excluded_by_criterion;
    let any_criterion = criteria
        .screening
        .iter()
        .chain(&criteria.full_text)
        .any(|c| c.criterion.is_some());
    let by_criterion = |rows: &mut Vec<Row>, counts: &[pax_core::CriterionCount]| {
        if !any_criterion {
            return;
        }
        for count in counts {
            let label = match &count.criterion {
                Some(criterion) => format!("criterion {criterion}"),
                None => "no criterion".to_string(),
            };
            rows.push(Row::Count(2, label, count.count));
        }
    };
    let by_provider =
        |rows: &mut Vec<Row>, providers: &std::collections::BTreeMap<String, usize>| {
            for (provider, count) in providers {
                rows.push(Row::Count(2, provider.clone(), *count));
            }
        };
    rows.push(Row::Phase("Identification"));
    rows.push(Row::Count(
        1,
        "Records identified from databases".into(),
        stats.identified.databases.values().sum(),
    ));
    by_provider(&mut rows, &stats.identified.databases);
    rows.push(Row::Count(
        1,
        "Records identified from citation searching".into(),
        stats.identified.citation_searching.values().sum(),
    ));
    by_provider(&mut rows, &stats.identified.citation_searching);
    // Only once `related` has been run: most reviews never use it.
    if !stats.identified.related_works.is_empty() {
        rows.push(Row::Count(
            1,
            "Records identified from related works".into(),
            stats.identified.related_works.values().sum(),
        ));
        by_provider(&mut rows, &stats.identified.related_works);
    }
    // Only once a paper has been added with `add --label`; one row per label.
    if !stats.identified.other_sources.is_empty() {
        rows.push(Row::Count(
            1,
            "Records identified from other sources".into(),
            stats.identified.other_sources.values().sum(),
        ));
        for (label, count) in &stats.identified.other_sources {
            let label = if label.is_empty() {
                "(unlabelled)".to_string()
            } else {
                label.clone()
            };
            rows.push(Row::Count(2, label, *count));
        }
    }
    rows.push(Row::Count(
        1,
        "Duplicate records removed".into(),
        stats.duplicates_removed,
    ));
    rows.push(Row::Phase("Screening"));
    rows.push(Row::Count(
        1,
        "Records screened".into(),
        stats.records_screened,
    ));
    rows.push(Row::Count(
        1,
        "Records excluded".into(),
        stats.excluded_screening,
    ));
    by_criterion(&mut rows, &criteria.screening);
    rows.push(Row::Count(
        1,
        "Records awaiting screening".into(),
        stats.awaiting_screening,
    ));
    rows.push(Row::Count(
        1,
        "Reports sought for retrieval".into(),
        stats.reports_sought,
    ));
    rows.push(Row::Count(
        1,
        "Reports not retrieved".into(),
        stats.not_retrieved,
    ));
    rows.push(Row::Count(
        1,
        "Reports assessed for eligibility".into(),
        stats.reports_assessed,
    ));
    rows.push(Row::Count(
        1,
        "Reports excluded".into(),
        stats.excluded_full_text.total,
    ));
    for reason in &stats.excluded_full_text.reasons {
        rows.push(Row::Count(2, reason.reason.clone(), reason.count));
    }
    by_criterion(&mut rows, &criteria.full_text);
    rows.push(Row::Phase("Included"));
    rows.push(Row::Count(
        1,
        "Studies included in review".into(),
        stats.included,
    ));

    let label_width = |indent: usize, label: &str| 2 * indent + label.chars().count();
    let width = rows
        .iter()
        .filter_map(|row| match row {
            Row::Count(indent, label, _) => Some(label_width(*indent, label)),
            Row::Phase(_) => None,
        })
        .max()
        .unwrap_or(0);
    let digits = rows
        .iter()
        .filter_map(|row| match row {
            Row::Count(_, _, count) => Some(count.to_string().len()),
            Row::Phase(_) => None,
        })
        .max()
        .unwrap_or(1);
    rows.into_iter()
        .map(|row| match row {
            Row::Phase(phase) => phase.to_string(),
            Row::Count(indent, label, count) => {
                let pad = width - label_width(indent, &label);
                format!(
                    "{}{label}{}  {count:>digits$}",
                    "  ".repeat(indent),
                    " ".repeat(pad)
                )
            }
        })
        .collect()
}

/// `--json`: each command's result as one pretty-printed JSON document on
/// stdout, and stderr as one compact JSON object per line.
pub struct JsonSink;

impl JsonSink {
    /// Prints `value` as the command's one JSON document.
    fn document(&mut self, value: &impl Serialize) {
        out!(
            "{}",
            serde_json::to_string_pretty(value).expect("output shapes serialize")
        );
    }

    /// The failure that ends a command, as `{"error": {"code", "message"}}`.
    pub fn failure_line(code: i32, message: &str) -> String {
        serde_json::json!({ "error": { "code": code, "message": message } }).to_string()
    }

    fn warning_line(&mut self, provider: Option<ProviderId>, message: &str) {
        let mut warning = serde_json::json!({ "message": message });
        if let Some(provider) = provider {
            warning["provider"] = provider.to_string().into();
        }
        eprintln!("{}", serde_json::json!({ "warning": warning }));
    }
}

impl Sink for JsonSink {
    fn message(&mut self, _message: &str) {
        // A confirmation for people; the document carries the result.
    }

    fn failure(&mut self, code: Code, message: &str) {
        eprintln!("{}", JsonSink::failure_line(code as i32, message));
    }

    fn error(&mut self, message: &str) {
        self.warning_line(None, message);
    }

    fn warning(&mut self, message: &str) {
        self.warning_line(None, message);
    }

    fn provider_note(&mut self, provider: ProviderId, message: &str) {
        self.warning_line(Some(provider), message);
    }

    fn search(
        &mut self,
        results: &SearchResults,
        index: &LibraryIndex,
        log: &LogOutcome,
        abstracts: bool,
    ) {
        self.document(&output::SearchResults::new(results, index, log, abstracts));
    }

    fn doi(&mut self, doi: &str, results: &DoiResults, index: &LibraryIndex) {
        self.document(&output::DoiRecords::new(doi, results, index));
    }

    fn citations(
        &mut self,
        results: &SeedResults,
        lists: output::Lists,
        index: &LibraryIndex,
        log: &LogOutcome,
        abstracts: bool,
    ) {
        self.document(&output::CitationResults::new(
            results, index, log, lists, abstracts,
        ));
    }

    fn show(&mut self, shown: &ShowResult, index: &LibraryIndex) {
        self.document(&output::Shown::new(shown, index));
    }

    fn papers(&mut self, papers: &[Paper]) {
        self.document(&output::PaperList::new(papers));
    }

    fn list(&mut self, papers: &[Paper], _library_empty: bool) {
        self.document(&output::PaperList::new(papers));
    }

    fn initialized(&mut self, root: &Path) {
        self.document(&output::Initialized::new(root));
    }

    fn added(&mut self, key: &PaperRef, log: Option<&LogOutcome>) {
        self.document(&output::Added::new(key, log));
    }

    fn removed(&mut self, paper: &Paper) {
        self.document(&output::DeclaredPaper::from(paper));
    }

    fn excluded(&mut self, exclusion: &Exclusion) {
        self.document(&output::Excluded::from(exclusion));
    }

    fn unexcluded(&mut self, exclusion: &Exclusion) {
        self.document(&output::Excluded::from(exclusion));
    }

    fn edited(&mut self, _name: &str, paper: &Paper) {
        self.document(&output::DeclaredPaper::from(paper));
    }

    fn export(&mut self, bibtex: &str) {
        TextSink.export(bibtex);
    }

    fn fetched(&mut self, citation_key: &str, outcome: &FetchOutcome) {
        TextSink.fetched(citation_key, outcome);
    }

    fn checked(&mut self, reports: &[ArtifactCheck]) {
        TextSink.checked(reports);
    }

    fn stats(&mut self, stats: &Stats, _by_label: bool) {
        self.document(stats);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_abstract_wraps_at_the_width_and_keeps_paragraphs() {
        let text = "Abstract: Actors send\nmessages   to other actors.\n\nSecond paragraph.";
        assert_eq!(
            abstract_lines(text, 20),
            [
                "Abstract: Actors",
                "send messages to",
                "other actors.",
                "",
                "Second paragraph.",
            ]
        );
    }

    #[test]
    fn a_word_longer_than_the_width_gets_its_own_line() {
        assert_eq!(
            abstract_lines("see https://example.org/a/very/long/path here", 10),
            ["see", "https://example.org/a/very/long/path", "here"]
        );
    }
}
