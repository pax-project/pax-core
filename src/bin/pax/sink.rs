//! Owns how the CLI renders output. Every command handler goes through a
//! `Sink` instead of calling `println!` directly, so a future alternate
//! output mode (e.g. JSON, for an external adapter UI) is a new `Sink` impl
//! rather than a rewrite of every command handler.
//!
//! Results go to stdout; errors and warnings go to stderr, so redirecting
//! stdout (`pax search … > results.txt`) captures the results alone.

use pax_core::{
    CandidateWork, CheckReport, CheckStatus, Exclusion, FetchOutcome, FilterApplication,
    IncompleteReason, LabelCounts, LibraryIndex, MergeReport, Paper, ProviderId, SearchPage,
    SeedMergeReport, Stats,
};

pub trait Sink {
    fn message(&mut self, message: &str);
    fn error(&mut self, message: &str);
    /// A note about the results that isn't itself a result, e.g. a filter
    /// a provider couldn't apply.
    fn warning(&mut self, message: &str);
    /// A provider that failed while the others may still have answered.
    fn provider_error(&mut self, provider: ProviderId, message: &str);
    /// A provider that has no record of the seed or DOI asked about, while
    /// another provider answered: an answer, not a failure.
    fn provider_note(&mut self, provider: ProviderId, message: &str);
    fn provider_header(&mut self, provider: ProviderId);
    fn page_header(&mut self, provider: ProviderId, page: &SearchPage);
    /// One citation provider's count, as [`Sink::page_header`], marked with
    /// why the list is incomplete, if it is. `page` is `None` when the
    /// provider returned no list.
    fn citation_header(
        &mut self,
        provider: ProviderId,
        page: Option<&SearchPage>,
        reason: Option<IncompleteReason>,
    );
    /// Results, each marked if it's in the library or excluded.
    fn candidates(&mut self, candidates: &[CandidateWork], index: &LibraryIndex);
    /// Search or citation results merged across providers. A row is marked
    /// in library or excluded if any of its records is. `language_unknown` says
    /// whether a record's language couldn't be checked by the filter; a
    /// merged row is marked only if that holds for every record in it. With
    /// `abstracts` set, each row's abstract ([`MergedWork::abstract_text`])
    /// follows it.
    ///
    /// [`MergedWork::abstract_text`]: pax_core::MergedWork::abstract_text
    fn merged(
        &mut self,
        report: &MergeReport,
        index: &LibraryIndex,
        language_unknown: &dyn Fn(&CandidateWork) -> bool,
        abstracts: bool,
    );
    /// The line a seed's counts sit under in a multi-seed `refs` /
    /// `cited-by` / `related`; `same_as` is the earlier seed it's the same
    /// paper as.
    fn seed_header(&mut self, seed: &str, same_as: Option<&str>);
    /// Citation results merged across several seeds, as [`Sink::merged`],
    /// each row also naming the seeds that returned it; `seeds` are their
    /// names by index.
    fn merged_seeds(
        &mut self,
        report: &SeedMergeReport,
        seeds: &[&str],
        index: &LibraryIndex,
        language_unknown: &dyn Fn(&CandidateWork) -> bool,
        abstracts: bool,
    );
    fn candidate(&mut self, candidate: &CandidateWork);
    fn papers(&mut self, papers: &[Paper]);
    fn paper(&mut self, paper: &Paper);
    /// An excluded paper's record, for `show` given its alias.
    fn exclusion(&mut self, exclusion: &Exclusion);
    fn export(&mut self, bibtex: &str);
    fn fetched(&mut self, citation_key: &str, outcome: &FetchOutcome);
    fn checked(&mut self, reports: &[CheckReport]);
    /// The PRISMA flow-diagram counts, aligned under the box names.
    fn stats(&mut self, stats: &Stats);
    /// The per-label counts, as a table after the flow-diagram counts.
    fn stats_by_label(&mut self, stats: &Stats);
    /// The PRISMA flow-diagram counts as one JSON object.
    fn stats_json(&mut self, stats: &Stats);
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

impl Sink for TextSink {
    fn message(&mut self, message: &str) {
        out!("{message}");
    }

    fn error(&mut self, message: &str) {
        eprintln!("Error: {message}");
    }

    fn warning(&mut self, message: &str) {
        eprintln!("Warning: {message}");
    }

    fn provider_error(&mut self, provider: ProviderId, message: &str) {
        eprintln!("Error: {provider}: {message}");
    }

    fn provider_note(&mut self, provider: ProviderId, message: &str) {
        eprintln!("Note: {provider}: {message}");
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

    fn candidate(&mut self, candidate: &CandidateWork) {
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

    fn paper(&mut self, paper: &Paper) {
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

    fn exclusion(&mut self, exclusion: &Exclusion) {
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

    fn checked(&mut self, reports: &[CheckReport]) {
        let (mut reproducible, mut not_fetched, mut mismatched, mut errored) = (0, 0, 0, 0);
        for report in reports {
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
        out!(
            "{} checked: {reproducible} reproducible, {not_fetched} not fetched, {mismatched} mismatched, {errored} errored",
            reports.len()
        );
    }

    fn stats(&mut self, stats: &Stats) {
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
    }

    fn stats_by_label(&mut self, stats: &Stats) {
        if stats.by_label.is_empty() {
            return;
        }
        out!();
        out!("By label");
        for line in label_lines(&stats.by_label) {
            out!("{line}");
        }
    }

    fn stats_json(&mut self, stats: &Stats) {
        out!(
            "{}",
            serde_json::to_string_pretty(stats).expect("Stats serializes")
        );
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
