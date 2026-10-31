use std::path::Path;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use pax_core::{
    CandidateId, DocType, ExclusionStage, Filters, Limit, ProviderId, SearchOptions, YearRange,
};

mod exit;
mod sink;
use exit::{Code, Failure};
use sink::{Sink, TextSink};

#[derive(Subcommand)]
enum Command {
    ///Initiate a new empty library
    Init,
    ///Search for papers
    Search {
        /// Free-text query, searched across all providers
        query: Option<String>,
        /// Search by author name instead of free text
        #[arg(long)]
        author: Option<String>,
        /// Resolve a DOI directly instead of searching
        #[arg(long)]
        doi: Option<String>,
        /// Search only the local library, without querying any provider
        #[arg(long)]
        local: Option<String>,
        #[command(flatten)]
        paging: PageArgs,
    },
    ///List the papers one or more papers cite, from OpenAlex and Semantic Scholar
    Refs {
        /// One or more seeds: a reference from `search` (e.g.
        /// openalex:W2072794470) or a library citation key (e.g. turing1936)
        #[arg(required = true, value_name = "SEED")]
        seeds: Vec<String>,
        #[command(flatten)]
        paging: PageArgs,
    },
    ///List the papers citing one or more papers, from OpenAlex and Semantic Scholar
    CitedBy {
        /// One or more seeds: a reference from `search` (e.g.
        /// openalex:W2072794470) or a library citation key (e.g. turing1936)
        #[arg(required = true, value_name = "SEED")]
        seeds: Vec<String>,
        #[command(flatten)]
        paging: PageArgs,
    },
    ///List the papers OpenAlex and Semantic Scholar recommend as related to one or more papers
    Related {
        /// One or more seeds: a reference from `search` (e.g.
        /// openalex:W2072794470) or a library citation key (e.g. turing1936)
        #[arg(required = true, value_name = "SEED")]
        seeds: Vec<String>,
        #[command(flatten)]
        paging: PageArgs,
    },
    ///Inspect a search result before adding it
    Show {
        /// A fully-qualified reference from `search`, e.g. openalex:W2072794470
        reference: String,
    },
    ///Declare a search result in the local library
    Add {
        /// A fully-qualified reference from `search`, e.g. openalex:W2072794470
        reference: String,
    },
    ///List papers already declared in the library
    List {
        /// Only papers with a matching author (case-insensitive substring)
        #[arg(long)]
        author: Option<String>,
        /// Only papers published in this year
        #[arg(long)]
        year: Option<i32>,
        /// Only papers with this exact tag
        #[arg(long)]
        tag: Option<String>,
    },
    ///Remove a declared paper from the library
    Remove {
        /// The paper's citation key, e.g. turing1936
        citation_key: String,
    },
    ///Exclude a paper: record it in research/excluded.nix with a reason and stage
    Exclude {
        /// A DOI, or a reference from `search`, e.g. openalex:W2072794470
        reference: String,
        /// Why it was excluded, e.g. "not empirical"
        #[arg(long)]
        reason: String,
        /// Where in screening it was excluded
        #[arg(long, value_parser = ["screening", "full-text"])]
        stage: String,
        /// Title to record; overrides the provider's, and is required if the
        /// reference can't be resolved
        #[arg(long)]
        title: Option<String>,
        /// Publication year to record; overrides the provider's. With the
        /// title, it lets `stats` match a paper with no DOI or arXiv id to
        /// its search-log records
        #[arg(long)]
        year: Option<i32>,
        /// The protocol's exclusion criterion, e.g. X3 (free text)
        #[arg(long)]
        criterion: Option<String>,
        /// The screening round, e.g. R1 (free text)
        #[arg(long)]
        round: Option<String>,
    },
    ///Reverse an exclusion: remove a paper's entry from research/excluded.nix
    Unexclude {
        /// The exclusion key (as `add` names it), a DOI, or a reference from
        /// `search`, e.g. openalex:W2072794470
        reference: String,
    },
    ///Modify a declared paper's tags, notes, citation key, or metadata
    Edit {
        /// The paper's citation key, e.g. turing1936
        citation_key: String,
        /// Add a tag (repeatable)
        #[arg(long = "add-tag")]
        add_tag: Vec<String>,
        /// Remove a tag (repeatable)
        #[arg(long = "remove-tag")]
        remove_tag: Vec<String>,
        /// Set the paper's notes
        #[arg(long)]
        notes: Option<String>,
        /// Rename the paper's citation key
        #[arg(long)]
        rename: Option<String>,
        /// Correct the paper's title
        #[arg(long)]
        title: Option<String>,
        /// Correct the paper's author list (repeatable; replaces the whole list)
        #[arg(long = "author")]
        author: Vec<String>,
        /// Correct the paper's year
        #[arg(long)]
        year: Option<i32>,
        /// Correct the paper's DOI
        #[arg(long)]
        doi: Option<String>,
        /// Set/replace the paper's PDF source URL (e.g. a copy found by hand
        /// after `add` recorded none); invalidates any already-fetched hash
        #[arg(long = "source-url")]
        source_url: Option<String>,
    },
    ///Export the local library
    Export {
        #[command(subcommand)]
        format: ExportFormat,
    },
    ///Materialize a declared paper's artifact through Nix
    Fetch {
        /// The paper's citation key, e.g. turing1936
        citation_key: String,
    },
    ///Verify declared artifacts still reproduce, without materializing them
    Check,
    ///Open a declared paper in the configured PDF viewer, fetching it first if needed
    Open {
        /// The paper's citation key, e.g. turing1936
        citation_key: String,
    },
    ///Materialize every declared paper that isn't fetched yet
    Sync,
    ///Print the review's PRISMA 2020 flow-diagram counts
    ///
    ///Computed from research/search-log.jsonl, research/excluded.nix and
    ///research/papers.nix; a missing log or excluded.nix counts as empty.
    ///Duplicates are counted across the whole log by identity key. "Reports
    ///not retrieved" is the declared papers with no source_url or no hash
    ///(what `check` reports as not fetched), decided without the network.
    ///"Records awaiting screening" is the logged papers neither excluded nor
    ///declared yet; "reports sought" is the declared papers plus those
    ///excluded at full text.
    Stats {
        /// Print the counts as one JSON object, for scripts and agents
        #[arg(long)]
        json: bool,
        /// Also print the records identified under each search-log label
        /// (`--label`); `--json` always includes them, as `by_label`
        #[arg(long = "by-label")]
        by_label: bool,
    },
}

/// How many results to retrieve per provider, and the filters to apply.
#[derive(Args)]
struct PageArgs {
    /// Retrieve at most this many results per provider (default 50)
    #[arg(long, conflicts_with = "all")]
    limit: Option<usize>,
    /// Retrieve every result each provider serves (can be slow)
    #[arg(long)]
    all: bool,
    /// Publication year: YYYY, or an inclusive range YYYY-YYYY, YYYY- or -YYYY
    #[arg(long, allow_hyphen_values = true)]
    year: Option<YearRange>,
    /// Document type: journal-article, conference-paper, preprint, review,
    /// book, book-chapter, thesis, or dataset
    #[arg(long = "type")]
    doc_type: Option<DocType>,
    /// Language, as an ISO 639-1 code (e.g. en)
    #[arg(long)]
    lang: Option<String>,
    /// Label the search in the search log, e.g. R2-backward, for `stats
    /// --by-label`; doesn't change what is searched
    #[arg(long)]
    label: Option<String>,
    /// Print each result's abstract under it (larger, slower responses)
    #[arg(long)]
    with_abstracts: bool,
}

impl PageArgs {
    /// Whether any flag is set.
    fn is_set(&self) -> bool {
        self.limit.is_some()
            || self.all
            || self.year.is_some()
            || self.doc_type.is_some()
            || self.lang.is_some()
            || self.label.is_some()
            || self.with_abstracts
    }

    fn options(self) -> SearchOptions {
        let defaults = SearchOptions::default();
        let limit = match (self.all, self.limit) {
            (true, _) => Limit::All,
            (false, Some(n)) => Limit::Count(n),
            (false, None) => defaults.limit,
        };
        let mut filters = Filters::default();
        if let Some(year) = self.year {
            filters = filters.with_years(year);
        }
        if let Some(doc_type) = self.doc_type {
            filters = filters.with_doc_type(doc_type);
        }
        if let Some(lang) = self.lang {
            filters = filters.with_language(lang);
        }
        let options = defaults
            .with_limit(limit)
            .with_filters(filters)
            .with_abstracts(self.with_abstracts);
        match self.label {
            Some(label) => options.with_label(label),
            None => options,
        }
    }
}

#[derive(Subcommand)]
enum ExportFormat {
    ///Export as BibTeX
    Bibtex,
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Reports `failure` and returns the exit code it maps to.
fn fail<'a>(sink: &mut impl Sink, failure: impl Into<Failure<'a>>) -> Code {
    let failure = failure.into();
    sink.error(&failure.to_string());
    exit::code(failure)
}

fn open_in_viewer(sink: &mut impl Sink, path: &Path) -> Code {
    let viewer = std::env::var("PAX_PDF_VIEWER").unwrap_or_else(|_| "xdg-open".to_string());
    match std::process::Command::new(&viewer).arg(path).status() {
        Ok(status) if status.success() => Code::Success,
        Ok(status) => fail(
            sink,
            Failure::Other(&format!("{viewer} exited with {status}")),
        ),
        Err(e) => fail(
            sink,
            Failure::Other(&format!("failed to launch {viewer}: {e}")),
        ),
    }
}

/// Reports a provider that returned no list. One that has no record of the
/// seed or DOI has answered, so while another provider returned a list,
/// that's a note rather than an error.
fn provider_failed<'a>(
    sink: &mut impl Sink,
    provider: ProviderId,
    failure: impl Into<Failure<'a>>,
    another_listed: bool,
) {
    let failure = failure.into();
    if another_listed && exit::is_answer(failure) {
        sink.provider_note(provider, &failure.to_string());
    } else {
        sink.provider_error(provider, &failure.to_string());
    }
}

/// Which kind of per-seed lists [`show_citations`] prints.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lists {
    /// `refs` / `cited-by`: citation lists, each with the provider's total.
    Citations,
    /// `related`: recommendations, which have no total.
    Related,
}

/// Prints each seed's provider counts, marked with why a list is incomplete
/// if it is, then one list merged across every seed and provider, the way
/// `search` does. With one seed the output is that seed's alone; with
/// several, each seed's counts sit under a `seed <seed>:` line and each
/// merged row names the seeds that returned it. With `abstracts`, each row
/// is followed by its abstract.
///
/// For `related`, only a missing list is marked: a related list never has a
/// total, so it's never complete, and `truncated` (the limit stopping short
/// of a total) would say something untrue; its count line already says the
/// total isn't reported.
fn show_citations(
    sink: &mut impl Sink,
    results: &pax_core::SeedResults,
    kind: Lists,
    abstracts: bool,
) -> Code {
    let index = pax_core::LibraryIndex::load(Path::new("."));
    let several = results.seeds.len() > 1;
    let mut codes = Vec::new();
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
            sink.seed_header(&seed.seed, same_as);
        }
        let lists = match &seed.results {
            Ok(lists) => lists,
            Err(e) => {
                let failure = Failure::from(e);
                sink.error(&of_seed(failure.to_string()));
                codes.push(exit::code(failure));
                continue;
            }
        };
        for provider in pax_core::CITATION_PROVIDERS {
            let Some(result) = lists.get(&provider) else {
                continue;
            };
            let reason = pax_core::incomplete_reason(result)
                .filter(|reason| kind == Lists::Citations || reason.is_missing());
            sink.citation_header(provider, result.as_ref().ok(), reason);
            if let Err(e) = result {
                sink.provider_error(provider, &of_seed(e.to_string()));
            }
        }
        codes.push(exit::citations(lists));
    }
    if results.any_listed() {
        let language_unknown = |work: &pax_core::CandidateWork| {
            results
                .seeds
                .iter()
                .filter_map(|seed| seed.results.as_ref().ok())
                .flat_map(|lists| lists.values().flatten())
                .any(|page| page.is_language_unknown(work))
        };
        if several {
            let seeds: Vec<&str> = results.seeds.iter().map(|s| s.seed.as_str()).collect();
            sink.merged_seeds(
                &results.merge(),
                &seeds,
                &index,
                &language_unknown,
                abstracts,
            );
        } else if let Some(Ok(lists)) = results.seeds.first().map(|seed| &seed.results) {
            sink.merged(
                &pax_core::merge_pages(lists),
                &index,
                &language_unknown,
                abstracts,
            );
        }
    }
    exit::seeds(codes)
}

/// Notes on stderr when a search wasn't logged, and fails a search whose
/// log couldn't be written (see [`exit::log_failed`]).
fn report_log(sink: &mut impl Sink, log: pax_core::LogOutcome, code: Code) -> Code {
    match log {
        pax_core::LogOutcome::NoLibrary => {
            sink.warning("not in a pax library, so this search isn't recorded in a search log");
            code
        }
        pax_core::LogOutcome::Failed(e) => {
            sink.warning(&format!(
                "couldn't write {}: {e}",
                pax_core::search_log::search_log_path(Path::new(".")).display()
            ));
            exit::log_failed(code)
        }
        _ => code,
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    let config = pax_core::Config {
        semantic_scholar_api_key: std::env::var("SEMANTIC_SCHOLAR_API_KEY").ok(),
        arxiv_contact: std::env::var("ARXIV_CONTACT").ok(),
    };
    run(cli.command, &mut TextSink, &config).await.into()
}

/// Whether `command` reads or writes the library in the current directory.
/// A `show` reference or `refs` / `cited-by` / `related` seed with a `:` is
/// a provider reference, which needs no library; without one it's a citation
/// key, so one such seed among several needs the library.
/// `search --local` is checked after its arguments are.
fn needs_library(command: &Command) -> bool {
    match command {
        Command::Init | Command::Search { .. } => false,
        Command::Show { reference } => !reference.contains(':'),
        Command::Refs { seeds, .. }
        | Command::CitedBy { seeds, .. }
        | Command::Related { seeds, .. } => seeds.iter().any(|seed| !seed.contains(':')),
        Command::Add { .. }
        | Command::List { .. }
        | Command::Remove { .. }
        | Command::Exclude { .. }
        | Command::Unexclude { .. }
        | Command::Edit { .. }
        | Command::Export { .. }
        | Command::Fetch { .. }
        | Command::Check
        | Command::Open { .. }
        | Command::Sync
        | Command::Stats { .. } => true,
    }
}

/// Fails with a clear message when there's no library in the current
/// directory, before a command that needs one does anything else, e.g. `add`
/// resolving its reference over the network.
fn require_library(sink: &mut impl Sink) -> Result<(), Code> {
    if pax_core::nix::papers_path(Path::new(".")).exists() {
        Ok(())
    } else {
        Err(fail(sink, Failure::NoLibrary))
    }
}

async fn run(command: Command, sink: &mut impl Sink, config: &pax_core::Config) -> Code {
    if needs_library(&command)
        && let Err(code) = require_library(sink)
    {
        return code;
    }
    match command {
        Command::Init => match pax_core::init_library(Path::new(".")) {
            Ok(()) => {
                sink.message("Library created");
                Code::Success
            }
            Err(e) => fail(sink, Failure::Init(&e)),
        },
        Command::Search {
            query,
            author,
            doi,
            local,
            paging,
        } => {
            let modes = [
                query.is_some(),
                author.is_some(),
                doi.is_some(),
                local.is_some(),
            ];
            if modes.iter().filter(|set| **set).count() != 1 {
                return fail(
                    sink,
                    Failure::Usage("specify exactly one of QUERY, --author, --doi, or --local"),
                );
            }
            if paging.is_set() && (doi.is_some() || local.is_some()) {
                return fail(
                    sink,
                    Failure::Usage(
                        "--limit, --all, --year, --type, --lang, --label, and --with-abstracts only apply to QUERY and --author searches",
                    ),
                );
            }

            if let Some(query) = &local {
                if let Err(code) = require_library(sink) {
                    return code;
                }
                sink.papers(&pax_core::search_local(query, Path::new(".")));
                return Code::Success;
            }

            let index = pax_core::LibraryIndex::load(Path::new("."));
            const ALL_PROVIDERS: [ProviderId; 4] = [
                ProviderId::OpenAlex,
                ProviderId::Crossref,
                ProviderId::SemanticScholar,
                ProviderId::ArXiv,
            ];

            if let Some(doi) = &doi {
                let results = pax_core::search_by_doi(doi, config).await;
                let listed = results.values().any(Result::is_ok);
                // arXiv has no DOI lookup.
                for provider in [
                    ProviderId::OpenAlex,
                    ProviderId::Crossref,
                    ProviderId::SemanticScholar,
                ] {
                    match results.get(&provider) {
                        Some(Ok(works)) => {
                            sink.provider_header(provider);
                            sink.candidates(works, &index);
                        }
                        Some(Err(e)) => provider_failed(sink, provider, e, listed),
                        None => {}
                    }
                }
                return exit::provider_results(results.values());
            }

            let options = paging.options();
            let root = Path::new(".");
            let pax_core::Logged { results, log, .. } = if let Some(author) = &author {
                pax_core::search_by_author_logged(author, &options, root, config).await
            } else {
                let query = query.as_deref().unwrap_or_default();
                pax_core::search_all_logged(query, &options, root, config).await
            };
            for provider in ALL_PROVIDERS {
                match results.get(&provider) {
                    Some(Ok(page)) => sink.page_header(provider, page),
                    Some(Err(e)) => sink.provider_error(provider, &e.to_string()),
                    None => {}
                }
            }
            let language_unknown = |work: &pax_core::CandidateWork| {
                results
                    .values()
                    .flatten()
                    .any(|page| page.is_language_unknown(work))
            };
            sink.merged(
                &pax_core::merge_pages(&results),
                &index,
                &language_unknown,
                options.abstracts,
            );
            report_log(sink, log, exit::provider_results(results.values()))
        }
        Command::Refs { seeds, paging } => {
            let options = paging.options();
            let pax_core::Logged { results, log, .. } =
                pax_core::references_many_logged(&seeds, &options, Path::new("."), config).await;
            let code = show_citations(sink, &results, Lists::Citations, options.abstracts);
            report_log(sink, log, code)
        }
        Command::CitedBy { seeds, paging } => {
            let options = paging.options();
            let pax_core::Logged { results, log, .. } =
                pax_core::cited_by_many_logged(&seeds, &options, Path::new("."), config).await;
            let code = show_citations(sink, &results, Lists::Citations, options.abstracts);
            report_log(sink, log, code)
        }
        Command::Related { seeds, paging } => {
            let options = paging.options();
            let pax_core::Logged { results, log, .. } =
                pax_core::related_many_logged(&seeds, &options, Path::new("."), config).await;
            let code = show_citations(sink, &results, Lists::Related, options.abstracts);
            report_log(sink, log, code)
        }
        Command::Show { reference } => {
            match pax_core::show_reference(&reference, Path::new("."), config).await {
                Ok(pax_core::ShowResult::Declared(paper)) => sink.paper(&paper),
                Ok(pax_core::ShowResult::Candidate(work)) => sink.candidate(&work),
                Err(e) => return fail(sink, &e),
            }
            Code::Success
        }
        Command::Add { reference } => {
            let id: CandidateId = match reference.parse() {
                Ok(id) => id,
                Err(e) => return fail(sink, &pax_core::PaxError::InvalidReference(e)),
            };
            match pax_core::add_candidate(&id, Path::new("."), config).await {
                Ok(paper_ref) => sink.message(&format!("Added {}", paper_ref.0)),
                Err(e) => return fail(sink, &e),
            }
            Code::Success
        }
        Command::List { author, year, tag } => {
            let library = match pax_core::Library::load(&pax_core::nix::papers_path(Path::new(".")))
            {
                Ok(library) => library,
                Err(e) => return fail(sink, &e),
            };
            if library.papers().is_empty() {
                sink.message("Library is empty");
            } else {
                let filter = pax_core::ListFilter { author, year, tag };
                let papers = pax_core::filter_papers(library.papers(), &filter);
                if papers.is_empty() {
                    sink.message("No papers match the given filters");
                } else {
                    sink.papers(&papers);
                }
            }
            Code::Success
        }
        Command::Remove { citation_key } => {
            match pax_core::remove_paper(&citation_key, Path::new(".")) {
                Ok(()) => sink.message(&format!("Removed {citation_key}")),
                Err(e) => return fail(sink, &e),
            }
            Code::Success
        }
        Command::Exclude {
            reference,
            reason,
            stage,
            title,
            year,
            criterion,
            round,
        } => {
            let stage: ExclusionStage = stage.parse().expect("clap restricts --stage");
            let options = pax_core::ExclusionOptions::new(reason, stage)
                .with_title(title)
                .with_year(year)
                .with_criterion(criterion)
                .with_round(round);
            match pax_core::exclude_paper_with(&reference, &options, Path::new("."), config).await {
                Ok(exclusion) => {
                    let mut decision = exclusion.stage.to_string();
                    if let Some(criterion) = &exclusion.criterion {
                        decision.push_str(&format!(", criterion {criterion}"));
                    }
                    if let Some(round) = &exclusion.round {
                        decision.push_str(&format!(", round {round}"));
                    }
                    sink.message(&format!(
                        "Excluded {} ({decision}): {}",
                        exclusion.key, exclusion.title
                    ));
                    Code::Success
                }
                Err(e @ pax_core::PaxError::ExclusionTitleUnresolved(..)) => {
                    sink.error(&format!("{e} with --title"));
                    exit::code(Failure::Pax(&e))
                }
                Err(e) => fail(sink, &e),
            }
        }
        Command::Unexclude { reference } => {
            match pax_core::unexclude_paper(&reference, Path::new("."), config).await {
                Ok(exclusion) => {
                    sink.message(&format!(
                        "Unexcluded {} ({}, {:?}): {}",
                        exclusion.key, exclusion.stage, exclusion.reason, exclusion.title
                    ));
                    Code::Success
                }
                Err(e) => fail(sink, &e),
            }
        }
        Command::Edit {
            citation_key,
            add_tag,
            remove_tag,
            notes,
            rename,
            title,
            author,
            year,
            doi,
            source_url,
        } => {
            let edits = pax_core::PaperEdits {
                add_tags: add_tag,
                remove_tags: remove_tag,
                notes,
                rename,
                title,
                authors: if author.is_empty() {
                    None
                } else {
                    Some(author)
                },
                year,
                doi,
                source_url,
            };
            match pax_core::edit_paper(&citation_key, Path::new("."), &edits) {
                Ok(()) => sink.message(&format!("Updated {citation_key}")),
                Err(e) => return fail(sink, &e),
            }
            Code::Success
        }
        Command::Export {
            format: ExportFormat::Bibtex,
        } => match pax_core::Library::load(&pax_core::nix::papers_path(Path::new("."))) {
            Ok(library) => {
                sink.export(&pax_core::bibtex::render(library.papers()));
                Code::Success
            }
            Err(e) => fail(sink, &e),
        },
        Command::Fetch { citation_key } => {
            match pax_core::fetch_paper(&citation_key, Path::new(".")) {
                Ok(outcome) => sink.fetched(&citation_key, &outcome),
                Err(e) => return fail(sink, &e),
            }
            Code::Success
        }
        Command::Check => match pax_core::check_library(Path::new(".")) {
            Ok(reports) if reports.is_empty() => {
                sink.message("Library is empty");
                Code::Success
            }
            Ok(reports) => {
                let failed = reports.iter().any(|r| {
                    matches!(
                        r.status,
                        pax_core::CheckStatus::Mismatch { .. } | pax_core::CheckStatus::Error(_)
                    )
                });
                sink.checked(&reports);
                if failed { Code::Failure } else { Code::Success }
            }
            Err(e) => fail(sink, &e),
        },
        Command::Open { citation_key } => {
            let root = Path::new(".");
            let resolved = match pax_core::resolve_artifact_path(&citation_key, root) {
                Ok(pax_core::ResolvedArtifact::NotFetched) => {
                    pax_core::fetch_paper(&citation_key, root)
                        .and_then(|_| pax_core::resolve_artifact_path(&citation_key, root))
                }
                other => other,
            };
            match resolved {
                Ok(pax_core::ResolvedArtifact::Path(path)) => open_in_viewer(sink, &path),
                Ok(pax_core::ResolvedArtifact::NoSourceUrl) => {
                    sink.error(&format!(
                        "{citation_key:?} has no PDF source recorded — the provider found no \
                         open-access copy when it was added, so there's nothing to fetch"
                    ));
                    exit::code(Failure::Pax(&pax_core::PaxError::NoSourceUrl(citation_key)))
                }
                Ok(pax_core::ResolvedArtifact::NotFetched) => fail(
                    sink,
                    Failure::Other("fetched, but the artifact still couldn't be resolved"),
                ),
                Err(e) => fail(sink, &e),
            }
        }
        Command::Sync => match pax_core::sync_library(Path::new(".")) {
            Ok(reports) if reports.is_empty() => {
                sink.message("Library is empty");
                Code::Success
            }
            Ok(reports) => {
                let failed = reports.iter().any(|r| r.result.is_err());
                for report in &reports {
                    match &report.result {
                        Ok(outcome) => sink.fetched(&report.citation_key, outcome),
                        Err(e) => sink.error(&format!("{}: {e}", report.citation_key)),
                    }
                }
                if failed { Code::Failure } else { Code::Success }
            }
            Err(e) => fail(sink, &e),
        },
        Command::Stats { json, by_label } => match pax_core::stats(Path::new(".")) {
            Ok(stats) => {
                if json {
                    sink.stats_json(&stats);
                } else {
                    sink.stats(&stats);
                    if by_label {
                        sink.stats_by_label(&stats);
                    }
                }
                Code::Success
            }
            Err(e) => fail(sink, &e),
        },
    }
}
