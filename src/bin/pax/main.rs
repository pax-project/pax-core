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
    ///List the papers a paper cites, from OpenAlex and Semantic Scholar
    Refs {
        /// A reference from `search` (e.g. openalex:W2072794470) or a
        /// library citation key (e.g. turing1936)
        reference: String,
        #[command(flatten)]
        paging: PageArgs,
    },
    ///List the papers citing a paper, from OpenAlex and Semantic Scholar
    CitedBy {
        /// A reference from `search` (e.g. openalex:W2072794470) or a
        /// library citation key (e.g. turing1936)
        reference: String,
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
    Stats {
        /// Print the counts as one JSON object, for scripts and agents
        #[arg(long)]
        json: bool,
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
}

impl PageArgs {
    /// Whether any flag is set.
    fn is_set(&self) -> bool {
        self.limit.is_some()
            || self.all
            || self.year.is_some()
            || self.doc_type.is_some()
            || self.lang.is_some()
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
        defaults.with_limit(limit).with_filters(filters)
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

/// Prints each citation provider's count, then one list merged across them,
/// the way `search` does.
fn show_citations(
    sink: &mut impl Sink,
    results: Result<pax_core::CitationResults, pax_core::PaxError>,
) -> Code {
    let results = match results {
        Ok(results) => results,
        Err(e) => return fail(sink, &e),
    };
    let index = pax_core::LibraryIndex::load(Path::new("."));
    let listed = results.values().any(Result::is_ok);
    for provider in pax_core::CITATION_PROVIDERS {
        match results.get(&provider) {
            Some(Ok(page)) => sink.page_header(provider, page),
            Some(Err(e)) => provider_failed(sink, provider, e, listed),
            None => {}
        }
    }
    if listed {
        let language_unknown = |work: &pax_core::CandidateWork| {
            results
                .values()
                .flatten()
                .any(|page| page.is_language_unknown(work))
        };
        sink.merged(&pax_core::merge_pages(&results), &index, &language_unknown);
    }
    exit::provider_results(results.values())
}

/// [`show_citations`] for a logged lookup, then [`report_log`].
fn show_logged_citations(
    sink: &mut impl Sink,
    logged: Result<pax_core::Logged<pax_core::CitationResults>, pax_core::PaxError>,
) -> Code {
    match logged {
        Ok(pax_core::Logged { results, log, .. }) => {
            let code = show_citations(sink, Ok(results));
            report_log(sink, log, code)
        }
        Err(e) => show_citations(sink, Err(e)),
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
    let cli = Cli::parse();
    let config = pax_core::Config {
        semantic_scholar_api_key: std::env::var("SEMANTIC_SCHOLAR_API_KEY").ok(),
        arxiv_contact: std::env::var("ARXIV_CONTACT").ok(),
    };
    run(cli.command, &mut TextSink, &config).await.into()
}

/// Whether `command` reads or writes the library in the current directory.
/// A `show`, `refs` or `cited-by` reference with a `:` is a provider
/// reference, which needs no library; without one it's a citation key.
/// `search --local` is checked after its arguments are.
fn needs_library(command: &Command) -> bool {
    match command {
        Command::Init | Command::Search { .. } => false,
        Command::Show { reference }
        | Command::Refs { reference, .. }
        | Command::CitedBy { reference, .. } => !reference.contains(':'),
        Command::Add { .. }
        | Command::List { .. }
        | Command::Remove { .. }
        | Command::Exclude { .. }
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
                        "--limit, --all, --year, --type, and --lang only apply to QUERY and --author searches",
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
            sink.merged(&pax_core::merge_pages(&results), &index, &language_unknown);
            report_log(sink, log, exit::provider_results(results.values()))
        }
        Command::Refs { reference, paging } => {
            let options = paging.options();
            let results =
                pax_core::references_logged(&reference, &options, Path::new("."), config).await;
            show_logged_citations(sink, results)
        }
        Command::CitedBy { reference, paging } => {
            let options = paging.options();
            let results =
                pax_core::cited_by_logged(&reference, &options, Path::new("."), config).await;
            show_logged_citations(sink, results)
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
        } => {
            let stage: ExclusionStage = stage.parse().expect("clap restricts --stage");
            match pax_core::exclude_paper(
                &reference,
                &reason,
                stage,
                title.as_deref(),
                Path::new("."),
                config,
            )
            .await
            {
                Ok(exclusion) => {
                    sink.message(&format!(
                        "Excluded {} ({}): {}",
                        exclusion.key, exclusion.stage, exclusion.title
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
        Command::Stats { json } => match pax_core::stats(Path::new(".")) {
            Ok(stats) => {
                if json {
                    sink.stats_json(&stats);
                } else {
                    sink.stats(&stats);
                }
                Code::Success
            }
            Err(e) => fail(sink, &e),
        },
    }
}
