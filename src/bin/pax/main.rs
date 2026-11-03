use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use pax_core::output::Lists;
use pax_core::{
    CandidateId, DocType, ExclusionStage, Filters, Limit, ProviderId, SearchOptions, YearRange,
};

mod exit;
mod sink;
use exit::{Code, Failure};
use sink::{JsonSink, Sink, TextSink};

#[derive(Subcommand)]
enum Command {
    ///Initiate a new empty library
    Init,
    ///Search for papers
    Search {
        /// Free-text query, searched across every provider (or those given by
        /// --provider)
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
        /// Ask only this provider: openalex, crossref, semanticscholar, arxiv
        /// or dblp. Repeat for several. Without it, every provider but dblp
        /// (computer science only) is asked
        #[arg(long = "provider", value_name = "PROVIDER")]
        providers: Vec<ProviderId>,
        #[command(flatten)]
        paging: PageArgs,
    },
    ///List the papers one or more papers cite, from OpenAlex and Semantic Scholar
    Refs {
        /// One or more seeds: a reference from `search` (e.g.
        /// openalex:W2072794470), or a library citation key or alias (e.g.
        /// turing1936 or P17)
        #[arg(required = true, value_name = "SEED")]
        seeds: Vec<String>,
        #[command(flatten)]
        paging: PageArgs,
    },
    ///List the papers citing one or more papers, from OpenAlex and Semantic Scholar
    CitedBy {
        /// One or more seeds: a reference from `search` (e.g.
        /// openalex:W2072794470), or a library citation key or alias (e.g.
        /// turing1936 or P17)
        #[arg(required = true, value_name = "SEED")]
        seeds: Vec<String>,
        #[command(flatten)]
        paging: PageArgs,
    },
    ///List the papers OpenAlex and Semantic Scholar recommend as related to one or more papers
    Related {
        /// One or more seeds: a reference from `search` (e.g.
        /// openalex:W2072794470), or a library citation key or alias (e.g.
        /// turing1936 or P17)
        #[arg(required = true, value_name = "SEED")]
        seeds: Vec<String>,
        #[command(flatten)]
        paging: PageArgs,
    },
    ///Inspect a search result, or a declared or excluded paper
    Show {
        /// A fully-qualified reference from `search` (e.g. openalex:W2072794470),
        /// or a library citation key or alias (e.g. turing1936 or P17); an
        /// excluded paper's alias shows its exclusion record
        reference: String,
    },
    ///Declare a search result in the local library
    Add {
        /// A fully-qualified reference from `search`, e.g. openalex:W2072794470
        reference: String,
        /// Record the add in the search log under this source, e.g. seeds,
        /// so `stats` counts the paper as identified from other sources;
        /// doesn't change what is added
        #[arg(long)]
        label: Option<String>,
    },
    ///List papers already declared in the library, or with --excluded the excluded ones
    List {
        /// Only papers with a matching author (case-insensitive substring)
        #[arg(long, conflicts_with = "excluded")]
        author: Option<String>,
        /// Only papers published in this year
        #[arg(long)]
        year: Option<i32>,
        /// Only papers with this exact tag
        #[arg(long, conflicts_with = "excluded")]
        tag: Option<String>,
        /// List the papers in research/excluded.nix instead
        #[arg(long)]
        excluded: bool,
        /// With --excluded: only exclusions under this exact criterion, e.g. X3
        #[arg(long, requires = "excluded")]
        criterion: Option<String>,
        /// With --excluded: only exclusions from this exact round, e.g. R1
        #[arg(long, requires = "excluded")]
        round: Option<String>,
    },
    ///Remove a declared paper from the library
    Remove {
        /// The paper's citation key or alias, e.g. turing1936 or P17
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
        /// A short name of your own for the paper, e.g. P18: accepted wherever
        /// a citation key is, unique across the library and its exclusions.
        /// Re-excluding without it keeps it; --alias "" clears it
        #[arg(long)]
        alias: Option<String>,
    },
    ///Reverse an exclusion: remove a paper's entry from research/excluded.nix
    Unexclude {
        /// The exclusion key (as `add` names it), its alias, a DOI, or a
        /// reference from `search`, e.g. openalex:W2072794470
        reference: String,
    },
    ///Modify a declared paper's tags, notes, citation key, alias, or metadata
    Edit {
        /// The paper's citation key or alias, e.g. turing1936 or P17
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
        /// Rename the paper's citation key (its alias is kept)
        #[arg(long)]
        rename: Option<String>,
        /// Set the paper's alias, a short name of your own such as P17,
        /// accepted wherever its citation key is; "" clears it. Letters,
        /// digits, `_`, `.` and `-`, unique across the library and its
        /// exclusions
        #[arg(long)]
        alias: Option<String>,
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
    ///
    ///With --file, from a PDF you already have (e.g. a paywalled paper
    ///downloaded through a university login): the file is added to the Nix
    ///store and its hash recorded, as a download's would be.
    Fetch {
        /// The paper's citation key or alias, e.g. turing1936 or P17
        citation_key: String,
        /// Add this file as the paper's artifact instead of downloading it
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
        /// With --file, replace an already-recorded hash the file doesn't match
        #[arg(long, requires = "file")]
        force: bool,
    },
    ///Verify declared artifacts still reproduce, without materializing them
    Check,
    ///Open a declared paper in the configured PDF viewer, fetching it first if needed
    Open {
        /// The paper's citation key or alias, e.g. turing1936 or P17
        citation_key: String,
    },
    ///Materialize every declared paper that isn't fetched yet
    ///
    ///With --from, papers added from a local file are first restored from a
    ///backup directory (`pax export pdfs <dir>`): each <citation_key>.pdf
    ///whose hash is the recorded one is added back to the Nix store; one
    ///that doesn't match is reported and skipped, never forced.
    Sync {
        /// Restore papers added from a local file from this backup directory
        #[arg(long, value_name = "DIR")]
        from: Option<PathBuf>,
    },
    ///Print the review's PRISMA 2020 flow-diagram counts
    ///
    ///Computed from research/search-log.jsonl, research/excluded.nix and
    ///research/papers.nix; a missing log or excluded.nix counts as empty.
    ///Duplicates are counted across the whole log by identity key. "Reports
    ///not retrieved" is the declared papers with no hash (what `check`
    ///reports as not fetched), decided without the network; one added with
    ///`fetch --file` has a hash, so it's included.
    ///"Records awaiting screening" is the logged papers neither excluded nor
    ///declared yet; "reports sought" is the declared papers plus those
    ///excluded at full text.
    Stats {
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
    ///Back up the PDFs added from a local file (`fetch --file`)
    ///
    ///Copies each one out of the Nix store as <DIR>/<citation_key>.pdf, and
    ///writes <DIR>/manifest.json with the hash each file must match. Papers
    ///with a source URL aren't copied: Nix can download them again. Restore
    ///with `pax sync --from <DIR>`.
    Pdfs {
        /// The backup directory, created if needed
        dir: PathBuf,
    },
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Print the result as one JSON document on stdout, and stderr as one
    /// JSON object per line (a failure as {"error": {"code", "message"}}),
    /// for scripts and agents. Accepted before or after the command
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

/// Reports `failure` and returns the exit code it maps to.
fn fail<'a>(sink: &mut impl Sink, failure: impl Into<Failure<'a>>) -> Code {
    let failure = failure.into();
    let code = exit::code(failure);
    sink.failure(code, &failure.to_string());
    code
}

/// Whether `--json` is among the arguments, before any `--`: how a
/// command line clap rejected still gets its error as JSON.
fn wants_json(args: impl IntoIterator<Item = std::ffi::OsString>) -> bool {
    args.into_iter()
        .skip(1)
        .take_while(|arg| arg != "--")
        .any(|arg| arg == "--json")
}

/// The first paragraph of clap's error, without its `error: ` prefix.
fn usage_message(error: &clap::Error) -> String {
    let rendered = error.render().to_string();
    let first = rendered.split("\n\n").next().unwrap_or_default().trim();
    first.strip_prefix("error: ").unwrap_or(first).to_string()
}

/// The citation key `name` (a citation key or alias) names in the library,
/// for reporting; `name` itself when it names no declared paper.
fn citation_key_of(name: &str) -> String {
    pax_core::Library::load(&pax_core::nix::papers_path(Path::new(".")))
        .ok()
        .and_then(|library| {
            let paper = library.find(name).or_else(|| library.find_alias(name))?;
            Some(paper.local.citation_key.clone())
        })
        .unwrap_or_else(|| name.to_string())
}

/// `sync`, restoring local artifacts from `from` when it's given.
fn sync(from: Option<&Path>) -> Result<Vec<pax_core::SyncReport>, pax_core::PaxError> {
    let root = Path::new(".");
    match from {
        Some(dir) => pax_core::sync_library_from(root, dir),
        None => pax_core::sync_library(root),
    }
}

fn open_in_viewer(sink: &mut impl Sink, path: &Path) -> Code {
    let viewer = std::env::var("PAX_PDF_VIEWER").unwrap_or_else(|_| "xdg-open".to_string());
    let status = std::process::Command::new(&viewer)
        .arg(path)
        .stdout(sink.child_stdout())
        .status();
    match status {
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

/// The fewest requests one seed's list takes from Semantic Scholar: the
/// seed's counts and a page for a citation list, a single request for
/// recommendations.
fn semantic_scholar_requests(lists: Lists) -> u32 {
    match lists {
        Lists::Related => 1,
        _ => 2,
    }
}

/// The exit code of a `refs` / `cited-by` / `related` lookup, from each
/// seed's: an invalid seed's own failure, else its lists' (see
/// [`exit::seeds`]).
fn citations_code(results: &pax_core::SeedResults) -> Code {
    exit::seeds(results.seeds.iter().map(|seed| match &seed.results {
        Ok(lists) => exit::citations(lists),
        Err(e) => exit::code(Failure::from(e)),
    }))
}

/// Prints a `refs` / `cited-by` / `related` lookup and returns its exit
/// code.
fn show_citations(
    sink: &mut impl Sink,
    logged: pax_core::Logged<pax_core::SeedResults>,
    lists: Lists,
    abstracts: bool,
) -> Code {
    let pax_core::Logged { results, log, .. } = logged;
    let index = pax_core::LibraryIndex::load(Path::new("."));
    sink.citations(&results, lists, &index, &log, abstracts);
    report_log(sink, log, citations_code(&results))
}

/// How long a run must be expected to wait on Semantic Scholar's pacing
/// before [`pacing_note`] says so.
const PACING_NOTE_AFTER: std::time::Duration = std::time::Duration::from_secs(10);

/// A note for stderr, before a run of `seeds` seeds, on how long pacing
/// Semantic Scholar's requests will make it take at least, so a long run
/// doesn't look hung; `None` for a run too short to notice.
fn pacing_note(seeds: usize, lists: Lists, has_api_key: bool) -> Option<String> {
    let interval = pax_core::SemanticScholarProvider::DEFAULT_MIN_INTERVAL;
    let requests = u32::try_from(seeds)
        .unwrap_or(u32::MAX)
        .saturating_mul(semantic_scholar_requests(lists));
    let wait = interval.saturating_mul(requests.saturating_sub(1));
    if wait < PACING_NOTE_AFTER {
        return None;
    }
    let mut note =
        format!("requests are spaced {interval:?} apart, so {seeds} seeds take at least {wait:?}");
    if !has_api_key {
        note.push_str(
            "; without SEMANTIC_SCHOLAR_API_KEY they share the unauthenticated rate limit, \
             so retries may add more",
        );
    }
    Some(note)
}

/// Prints [`pacing_note`], if there is one, as a Semantic Scholar note.
fn note_pacing(sink: &mut impl Sink, seeds: usize, lists: Lists, config: &pax_core::Config) {
    let has_api_key = config.semantic_scholar_api_key.is_some();
    if let Some(note) = pacing_note(seeds, lists, has_api_key) {
        sink.provider_note(ProviderId::SemanticScholar, &note);
    }
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
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // Help and version aren't errors, and print as usual.
        Err(e) if e.use_stderr() && wants_json(std::env::args_os()) => {
            let code = e.exit_code();
            eprintln!("{}", JsonSink::failure_line(code, &usage_message(&e)));
            return ExitCode::from(u8::try_from(code).unwrap_or(1));
        }
        Err(e) => e.exit(),
    };
    let config = pax_core::Config {
        semantic_scholar_api_key: std::env::var("SEMANTIC_SCHOLAR_API_KEY").ok(),
        arxiv_contact: std::env::var("ARXIV_CONTACT").ok(),
    };
    if cli.json {
        run(cli.command, &mut JsonSink, &config).await.into()
    } else {
        run(cli.command, &mut TextSink, &config).await.into()
    }
}

/// Whether `command` reads or writes the library in the current directory.
/// A `show` reference or `refs` / `cited-by` / `related` seed with a `:` is
/// a provider reference, which needs no library; without one it's a citation
/// key or alias, so one such seed among several needs the library.
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
        | Command::Sync { .. }
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
                sink.initialized(Path::new("."));
                Code::Success
            }
            Err(e) => fail(sink, Failure::Init(&e)),
        },
        Command::Search {
            query,
            author,
            doi,
            local,
            providers,
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
            if !providers.is_empty() && (doi.is_some() || local.is_some()) {
                return fail(
                    sink,
                    Failure::Usage("--provider only applies to QUERY and --author searches"),
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
            if let Some(doi) = &doi {
                let results = pax_core::search_by_doi(doi, config).await;
                sink.doi(doi, &results, &index);
                return exit::provider_results(results.values());
            }

            let options = paging.options().with_providers(providers);
            let root = Path::new(".");
            let pax_core::Logged { results, log, .. } = if let Some(author) = &author {
                pax_core::search_by_author_logged(author, &options, root, config).await
            } else {
                let query = query.as_deref().unwrap_or_default();
                pax_core::search_all_logged(query, &options, root, config).await
            };
            sink.search(&results, &index, &log, options.abstracts);
            report_log(sink, log, exit::provider_results(results.values()))
        }
        Command::Refs { seeds, paging } => {
            let options = paging.options();
            note_pacing(sink, seeds.len(), Lists::Citations, config);
            let logged =
                pax_core::references_many_logged(&seeds, &options, Path::new("."), config).await;
            show_citations(sink, logged, Lists::Citations, options.abstracts)
        }
        Command::CitedBy { seeds, paging } => {
            let options = paging.options();
            note_pacing(sink, seeds.len(), Lists::Citations, config);
            let logged =
                pax_core::cited_by_many_logged(&seeds, &options, Path::new("."), config).await;
            show_citations(sink, logged, Lists::Citations, options.abstracts)
        }
        Command::Related { seeds, paging } => {
            let options = paging.options();
            note_pacing(sink, seeds.len(), Lists::Related, config);
            let logged =
                pax_core::related_many_logged(&seeds, &options, Path::new("."), config).await;
            show_citations(sink, logged, Lists::Related, options.abstracts)
        }
        Command::Show { reference } => {
            match pax_core::show_reference(&reference, Path::new("."), config).await {
                Ok(shown) => {
                    let index = pax_core::LibraryIndex::load(Path::new("."));
                    sink.show(&shown, &index);
                    Code::Success
                }
                Err(e) => fail(sink, &e),
            }
        }
        Command::Add { reference, label } => {
            let id: CandidateId = match reference.parse() {
                Ok(id) => id,
                Err(e) => return fail(sink, &pax_core::PaxError::InvalidReference(e)),
            };
            let Some(label) = label else {
                match pax_core::add_candidate(&id, Path::new("."), config).await {
                    Ok(paper_ref) => sink.added(&paper_ref, None),
                    Err(e) => return fail(sink, &e),
                }
                return Code::Success;
            };
            match pax_core::add_candidate_logged(&id, &label, Path::new("."), config).await {
                Ok(logged) => {
                    sink.added(&logged.results, Some(&logged.log));
                    report_log(sink, logged.log, Code::Success)
                }
                Err(e) => fail(sink, &e),
            }
        }
        Command::List {
            excluded: true,
            year,
            criterion,
            round,
            ..
        } => {
            let exclusions =
                match pax_core::Exclusions::load(&pax_core::nix::excluded_path(Path::new("."))) {
                    Ok(exclusions) => exclusions,
                    Err(e) => return fail(sink, &e),
                };
            let filter = pax_core::ExclusionFilter::new()
                .with_year(year)
                .with_criterion(criterion)
                .with_round(round);
            let matching = pax_core::filter_exclusions(exclusions.entries(), &filter);
            sink.exclusion_list(&matching, exclusions.entries().is_empty());
            Code::Success
        }
        Command::List {
            author, year, tag, ..
        } => {
            let library = match pax_core::Library::load(&pax_core::nix::papers_path(Path::new(".")))
            {
                Ok(library) => library,
                Err(e) => return fail(sink, &e),
            };
            let filter = pax_core::ListFilter { author, year, tag };
            let papers = pax_core::filter_papers(library.papers(), &filter);
            sink.list(&papers, library.papers().is_empty());
            Code::Success
        }
        Command::Remove { citation_key } => {
            match pax_core::remove_paper_entry(&citation_key, Path::new(".")) {
                Ok(paper) => sink.removed(&paper),
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
            alias,
        } => {
            let stage: ExclusionStage = stage.parse().expect("clap restricts --stage");
            let options = pax_core::ExclusionOptions::new(reason, stage)
                .with_title(title)
                .with_year(year)
                .with_criterion(criterion)
                .with_round(round)
                .with_alias(alias);
            match pax_core::exclude_paper_with(&reference, &options, Path::new("."), config).await {
                Ok(exclusion) => {
                    sink.excluded(&exclusion);
                    Code::Success
                }
                Err(e @ pax_core::PaxError::ExclusionTitleUnresolved(..)) => {
                    let code = exit::code(Failure::Pax(&e));
                    sink.failure(code, &format!("{e} with --title"));
                    code
                }
                Err(e) => fail(sink, &e),
            }
        }
        Command::Unexclude { reference } => {
            match pax_core::unexclude_paper(&reference, Path::new("."), config).await {
                Ok(exclusion) => {
                    sink.unexcluded(&exclusion);
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
            alias,
            title,
            author,
            year,
            doi,
            source_url,
        } => {
            let authors = (!author.is_empty()).then_some(author);
            let edits = pax_core::PaperEdits::new()
                .with_add_tags(add_tag)
                .with_remove_tags(remove_tag)
                .with_notes(notes)
                .with_rename(rename)
                .with_title(title)
                .with_authors(authors)
                .with_year(year)
                .with_doi(doi)
                .with_source_url(source_url)
                .with_alias(alias);
            match pax_core::edit_paper_entry(&citation_key, Path::new("."), &edits) {
                Ok(paper) => sink.edited(&citation_key, &paper),
                Err(e) => return fail(sink, &e),
            }
            Code::Success
        }
        Command::Export {
            format: ExportFormat::Bibtex,
        } => match pax_core::Library::load(&pax_core::nix::papers_path(Path::new("."))) {
            Ok(library) => {
                sink.export(library.papers());
                Code::Success
            }
            Err(e) => fail(sink, &e),
        },
        Command::Export {
            format: ExportFormat::Pdfs { dir },
        } => match pax_core::export_pdfs(Path::new("."), &dir) {
            Ok(export) => {
                sink.exported_pdfs(&export);
                exit::artifacts(
                    export
                        .papers
                        .iter()
                        .filter_map(|copy| copy.result.as_ref().err())
                        .map(|e| exit::code(Failure::Pax(e))),
                )
            }
            Err(e) => fail(sink, &e),
        },
        Command::Fetch {
            citation_key: name,
            file,
            force,
        } => {
            let root = Path::new(".");
            let citation_key = citation_key_of(&name);
            let outcome = match &file {
                Some(file) => pax_core::fetch_file(&name, file, force, root),
                None => pax_core::fetch_paper(&name, root),
            };
            match outcome {
                Ok(outcome) => sink.fetched(&name, &citation_key, &outcome),
                Err(e) => return fail(sink, &e),
            }
            if file.is_some() && !pax_core::nix::flake_builds_local_artifacts(root) {
                sink.warning(
                    "research/flake.nix predates `fetch --file`, so `nix build` can't build a \
                     paper added from a local file (pax open still opens it); replace it with \
                     the current template, \
                     https://github.com/pax-project/pax-core/blob/main/templates/flake.nix.template",
                );
            }
            Code::Success
        }
        Command::Check => match pax_core::check_artifacts(Path::new(".")) {
            Ok(checks) => {
                sink.checked(&checks);
                exit::artifacts(checks.iter().filter_map(|check| {
                    match &check.status {
                        pax_core::CheckStatus::Mismatch { .. } => Some(Code::Failure),
                        pax_core::CheckStatus::Error(_) => Some(
                            check
                                .error
                                .as_ref()
                                .map_or(Code::Failure, |e| exit::code(Failure::Pax(e))),
                        ),
                        _ => None,
                    }
                }))
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
                Ok(pax_core::ResolvedArtifact::Path(path)) => {
                    let code = open_in_viewer(sink, &path);
                    if code == Code::Success {
                        sink.opened(&citation_key_of(&citation_key), &path);
                    }
                    code
                }
                Ok(pax_core::ResolvedArtifact::NoSourceUrl) => {
                    let message = format!(
                        "{citation_key:?} has no PDF source recorded — the provider found no \
                         open-access copy when it was added, so there's nothing to fetch; add a \
                         copy you have with `pax fetch {citation_key} --file <path>`"
                    );
                    let error = pax_core::PaxError::NoSourceUrl(citation_key);
                    let code = exit::code(Failure::Pax(&error));
                    sink.failure(code, &message);
                    code
                }
                Ok(pax_core::ResolvedArtifact::NotFetched) => fail(
                    sink,
                    Failure::Other("fetched, but the artifact still couldn't be resolved"),
                ),
                Err(e) => fail(sink, &e),
            }
        }
        Command::Sync { from } => match sync(from.as_deref()) {
            Ok(reports) => {
                sink.synced(&reports);
                exit::artifacts(reports.iter().filter_map(|report| {
                    report
                        .result
                        .as_ref()
                        .err()
                        .map(|e| exit::code(Failure::Pax(e)))
                }))
            }
            Err(e) => fail(sink, &e),
        },
        Command::Stats { by_label } => match pax_core::stats(Path::new(".")) {
            Ok(stats) => {
                sink.stats(&stats, by_label);
                Code::Success
            }
            Err(e) => fail(sink, &e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_few_seeds_get_no_pacing_note() {
        assert_eq!(pacing_note(1, Lists::Citations, false), None);
        assert_eq!(pacing_note(5, Lists::Citations, false), None);
        assert_eq!(pacing_note(10, Lists::Related, false), None);
    }

    #[test]
    fn many_seeds_are_told_how_long_pacing_takes() {
        assert_eq!(
            pacing_note(20, Lists::Citations, true).as_deref(),
            Some("requests are spaced 1s apart, so 20 seeds take at least 39s")
        );
    }

    #[test]
    fn without_a_key_the_note_mentions_the_shared_limit() {
        let note = pacing_note(20, Lists::Related, false).unwrap();
        assert!(note.starts_with("requests are spaced 1s apart, so 20 seeds take at least 19s"));
        assert!(note.contains("SEMANTIC_SCHOLAR_API_KEY"));
    }
}
