//! Owns how the CLI renders output. Every command handler goes through a
//! `Sink` instead of calling `println!` directly, so a future alternate
//! output mode (e.g. JSON, for an external adapter UI) is a new `Sink` impl
//! rather than a rewrite of every command handler.
//!
//! Results go to stdout; errors and warnings go to stderr, so redirecting
//! stdout (`pax search … > results.txt`) captures the results alone.

use pax_core::{
    CandidateWork, CheckReport, CheckStatus, FetchOutcome, FilterApplication, LibraryIndex,
    MergeReport, Paper, ProviderId, SearchPage, Stats,
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
    /// Results, each marked if it's in the library or excluded.
    fn candidates(&mut self, candidates: &[CandidateWork], index: &LibraryIndex);
    /// Search or citation results merged across providers. A row is marked
    /// in library or excluded if any of its records is. `language_unknown` says
    /// whether a record's language couldn't be checked by the filter; a
    /// merged row is marked only if that holds for every record in it.
    fn merged(
        &mut self,
        report: &MergeReport,
        index: &LibraryIndex,
        language_unknown: &dyn Fn(&CandidateWork) -> bool,
    );
    fn candidate(&mut self, candidate: &CandidateWork);
    fn papers(&mut self, papers: &[Paper]);
    fn paper(&mut self, paper: &Paper);
    fn export(&mut self, bibtex: &str);
    fn fetched(&mut self, citation_key: &str, outcome: &FetchOutcome);
    fn checked(&mut self, reports: &[CheckReport]);
    /// The PRISMA flow-diagram counts, aligned under the box names.
    fn stats(&mut self, stats: &Stats);
    /// The PRISMA flow-diagram counts as one JSON object.
    fn stats_json(&mut self, stats: &Stats);
}

pub struct TextSink;

impl Sink for TextSink {
    fn message(&mut self, message: &str) {
        println!("{message}");
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
        println!("{provider}:");
    }

    fn page_header(&mut self, provider: ProviderId, page: &SearchPage) {
        let retrieved = page.retrieved();
        let Some(total) = page.total else {
            println!("{provider}: retrieved {retrieved} (total not reported)");
            return;
        };
        let scanned = page.scanned;
        let skipped = scanned.saturating_sub(retrieved as u64);
        println!("{provider}: retrieved {retrieved} of {total}");
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

    fn candidates(&mut self, candidates: &[CandidateWork], index: &LibraryIndex) {
        for work in candidates {
            print_work(work, &work.id.to_string(), &Marks::of([work], index));
        }
    }

    fn merged(
        &mut self,
        report: &MergeReport,
        index: &LibraryIndex,
        language_unknown: &dyn Fn(&CandidateWork) -> bool,
    ) {
        println!(
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
            print_work(&merged.work, &ids.join(" · "), &marks);
        }
    }

    fn candidate(&mut self, candidate: &CandidateWork) {
        println!("Title:      {}", candidate.title);
        println!("Authors:    {}", candidate.authors.join(", "));
        println!("Published:  {}", candidate.publish_date);
        println!(
            "DOI:        {}",
            candidate.doi.as_deref().unwrap_or("(no doi)")
        );
        println!(
            "Venue:      {}",
            candidate.venue.as_deref().unwrap_or("(no venue)")
        );
        println!(
            "PDF source: {}",
            candidate.pdf_url.as_deref().unwrap_or("(none found)")
        );
        if let Some(abstract_text) = &candidate.abstract_text {
            println!("Abstract:   {abstract_text}");
        }
        println!("Reference:  {}", candidate.id);
    }

    fn papers(&mut self, papers: &[Paper]) {
        for paper in papers {
            println!("{}", paper.local.citation_key);
            println!("\t{}", paper.identity.title);
            if !paper.identity.authors.is_empty() {
                println!("\t{}", paper.identity.authors.join(", "));
            }
            if let Some(year) = paper.identity.year {
                println!("\t{}", year);
            }
        }
    }

    fn paper(&mut self, paper: &Paper) {
        println!("Title:        {}", paper.identity.title);
        println!("Authors:      {}", paper.identity.authors.join(", "));
        if let Some(year) = paper.identity.year {
            println!("Year:         {}", year);
        }
        println!(
            "DOI:          {}",
            paper.identity.doi.as_deref().unwrap_or("(no doi)")
        );
        println!(
            "Venue:        {}",
            paper.identity.venue.as_deref().unwrap_or("(no venue)")
        );
        println!("Citation key: {}", paper.local.citation_key);
        println!(
            "PDF source:   {}",
            paper
                .artifact
                .source_url
                .as_deref()
                .unwrap_or("(not resolved)")
        );
        println!(
            "Artifact:     {}",
            if paper.artifact.hash.is_some() {
                "Fetched"
            } else {
                "Not fetched"
            }
        );
        if !paper.local.tags.is_empty() {
            println!("Tags:         {}", paper.local.tags.join(", "));
        }
        if let Some(notes) = &paper.local.notes {
            println!("Notes:        {}", notes);
        }
    }

    fn export(&mut self, bibtex: &str) {
        if !bibtex.is_empty() {
            println!("{bibtex}");
        }
    }

    fn fetched(&mut self, citation_key: &str, outcome: &FetchOutcome) {
        match outcome {
            FetchOutcome::AlreadyFetched { hash } => {
                println!("Already fetched {citation_key} ({hash})");
            }
            FetchOutcome::Fetched { hash } => {
                println!("Fetched {citation_key} ({hash})");
            }
        }
    }

    fn checked(&mut self, reports: &[CheckReport]) {
        let (mut reproducible, mut not_fetched, mut mismatched, mut errored) = (0, 0, 0, 0);
        for report in reports {
            match &report.status {
                CheckStatus::NotFetched => {
                    not_fetched += 1;
                    println!("{}: not fetched", report.citation_key);
                }
                CheckStatus::Reproducible => {
                    reproducible += 1;
                    println!("{}: reproducible", report.citation_key);
                }
                CheckStatus::Mismatch { expected, actual } => {
                    mismatched += 1;
                    println!(
                        "{}: MISMATCH (expected {expected}, got {actual})",
                        report.citation_key
                    );
                }
                CheckStatus::Error(message) => {
                    errored += 1;
                    println!("{}: ERROR — {message}", report.citation_key);
                }
            }
        }
        println!(
            "{} checked: {reproducible} reproducible, {not_fetched} not fetched, {mismatched} mismatched, {errored} errored",
            reports.len()
        );
    }

    fn stats(&mut self, stats: &Stats) {
        for line in stats_lines(stats) {
            println!("{line}");
        }
        if stats.skipped_log_lines > 0 {
            self.warning(&format!(
                "skipped {} search-log line(s) whose provider failed, so retrieved no records",
                stats.skipped_log_lines
            ));
        }
    }

    fn stats_json(&mut self, stats: &Stats) {
        println!(
            "{}",
            serde_json::to_string_pretty(stats).expect("Stats serializes")
        );
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
/// record's reference, or every merged record's reference for a merged row.
fn print_work(work: &CandidateWork, ids: &str, marks: &Marks) {
    let flag = |set: bool, text: &'static str| if set { text } else { "" };
    println!(
        "\t{}{}{}{}",
        work.title,
        flag(marks.in_library, " [in library]"),
        flag(marks.excluded, " [excluded]"),
        flag(marks.language_unknown, " [language unknown]"),
    );
    println!("\t\t{ids}");
    if !work.authors.is_empty() {
        println!("\t\t{}", work.authors.join(", "));
    }
    println!("\t\t{}", work.publish_date);
    println!("\t\t{}", work.doi.as_deref().unwrap_or("(no doi)"));
    println!("\t\t{}", work.venue.as_deref().unwrap_or("(no venue)"));
    println!(
        "\t\tPDF: {}",
        if work.pdf_url.is_some() { "✓" } else { "✗" }
    );
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
    rows.push(Row::Count(
        1,
        "Reports not retrieved".into(),
        stats.not_retrieved,
    ));
    rows.push(Row::Count(
        1,
        "Reports excluded".into(),
        stats.excluded_full_text.total,
    ));
    for reason in &stats.excluded_full_text.reasons {
        rows.push(Row::Count(2, reason.reason.clone(), reason.count));
    }
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
