use std::path::Path;

use clap::{Parser, Subcommand};
use pax_core::{CandidateId, ProviderId};

mod sink;
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

fn open_in_viewer(sink: &mut impl Sink, path: &Path) {
    let viewer = std::env::var("PAX_PDF_VIEWER").unwrap_or_else(|_| "xdg-open".to_string());
    match std::process::Command::new(&viewer).arg(path).status() {
        Ok(status) if status.success() => {}
        Ok(status) => sink.error(&format!("{viewer} exited with {status}")),
        Err(e) => sink.error(&format!("failed to launch {viewer}: {e}")),
    }
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    let mut sink = TextSink;
    let config = pax_core::Config {
        semantic_scholar_api_key: std::env::var("SEMANTIC_SCHOLAR_API_KEY").ok(),
        arxiv_contact: std::env::var("ARXIV_CONTACT").ok(),
    };

    match cli.command {
        Command::Init => match pax_core::init_library(Path::new(".")) {
            Ok(()) => sink.message("Library created"),
            Err(e) => sink.error(&e.to_string()),
        },
        Command::Search {
            query,
            author,
            doi,
            local,
        } => {
            let modes = [
                query.is_some(),
                author.is_some(),
                doi.is_some(),
                local.is_some(),
            ];
            if modes.iter().filter(|set| **set).count() != 1 {
                sink.error("specify exactly one of QUERY, --author, --doi, or --local");
                return;
            }

            if let Some(query) = &local {
                sink.papers(&pax_core::search_local(query, Path::new(".")));
                return;
            }

            let known_dois = pax_core::known_dois(Path::new("."));
            let (results, providers): (_, &[ProviderId]) = if let Some(author) = &author {
                (
                    pax_core::search_by_author(author, &config).await,
                    &[
                        ProviderId::OpenAlex,
                        ProviderId::Crossref,
                        ProviderId::SemanticScholar,
                        ProviderId::ArXiv,
                    ],
                )
            } else if let Some(doi) = &doi {
                (
                    pax_core::search_by_doi(doi, &config).await,
                    &[
                        ProviderId::OpenAlex,
                        ProviderId::Crossref,
                        ProviderId::SemanticScholar,
                    ],
                )
            } else {
                (
                    pax_core::search_all(query.as_deref().unwrap_or_default(), &config).await,
                    &[
                        ProviderId::OpenAlex,
                        ProviderId::Crossref,
                        ProviderId::SemanticScholar,
                        ProviderId::ArXiv,
                    ],
                )
            };
            for provider in providers {
                sink.provider_header(*provider);
                match results.get(provider) {
                    Some(Ok(works)) => sink.candidates(works, &known_dois),
                    Some(Err(e)) => sink.error(&e.to_string()),
                    None => {}
                }
            }
        }
        Command::Show { reference } => {
            match pax_core::show_reference(&reference, Path::new("."), &config).await {
                Ok(pax_core::ShowResult::Declared(paper)) => sink.paper(&paper),
                Ok(pax_core::ShowResult::Candidate(work)) => sink.candidate(&work),
                Err(e) => sink.error(&e.to_string()),
            }
        }
        Command::Add { reference } => {
            let id: CandidateId = match reference.parse() {
                Ok(id) => id,
                Err(e) => {
                    sink.error(&e.to_string());
                    return;
                }
            };
            match pax_core::add_candidate(&id, Path::new("."), &config).await {
                Ok(paper_ref) => sink.message(&format!("Added {}", paper_ref.0)),
                Err(e) => sink.error(&e.to_string()),
            }
        }
        Command::List { author, year, tag } => {
            match pax_core::Library::load(&pax_core::nix::papers_path(Path::new("."))) {
                Ok(library) if library.papers().is_empty() => sink.message("Library is empty"),
                Ok(library) => {
                    let filter = pax_core::ListFilter { author, year, tag };
                    let papers = pax_core::filter_papers(library.papers(), &filter);
                    if papers.is_empty() {
                        sink.message("No papers match the given filters");
                    } else {
                        sink.papers(&papers);
                    }
                }
                Err(e) => sink.error(&e.to_string()),
            }
        }
        Command::Remove { citation_key } => {
            match pax_core::remove_paper(&citation_key, Path::new(".")) {
                Ok(()) => sink.message(&format!("Removed {citation_key}")),
                Err(e) => sink.error(&e.to_string()),
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
                Err(e) => sink.error(&e.to_string()),
            }
        }
        Command::Export {
            format: ExportFormat::Bibtex,
        } => match pax_core::Library::load(&pax_core::nix::papers_path(Path::new("."))) {
            Ok(library) => sink.export(&pax_core::bibtex::render(library.papers())),
            Err(e) => sink.error(&e.to_string()),
        },
        Command::Fetch { citation_key } => {
            match pax_core::fetch_paper(&citation_key, Path::new(".")) {
                Ok(outcome) => sink.fetched(&citation_key, &outcome),
                Err(e) => sink.error(&e.to_string()),
            }
        }
        Command::Check => match pax_core::check_library(Path::new(".")) {
            Ok(reports) if reports.is_empty() => sink.message("Library is empty"),
            Ok(reports) => {
                let failed = reports.iter().any(|r| {
                    matches!(
                        r.status,
                        pax_core::CheckStatus::Mismatch { .. } | pax_core::CheckStatus::Error(_)
                    )
                });
                sink.checked(&reports);
                if failed {
                    std::process::exit(1);
                }
            }
            Err(e) => sink.error(&e.to_string()),
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
                Ok(pax_core::ResolvedArtifact::Path(path)) => open_in_viewer(&mut sink, &path),
                Ok(pax_core::ResolvedArtifact::NoSourceUrl) => {
                    sink.error(&format!(
                        "{citation_key:?} has no PDF source recorded — the provider found no \
                         open-access copy when it was added, so there's nothing to fetch"
                    ));
                }
                Ok(pax_core::ResolvedArtifact::NotFetched) => {
                    sink.error("fetched, but the artifact still couldn't be resolved");
                }
                Err(e) => sink.error(&e.to_string()),
            }
        }
        Command::Sync => match pax_core::sync_library(Path::new(".")) {
            Ok(reports) if reports.is_empty() => sink.message("Library is empty"),
            Ok(reports) => {
                let failed = reports.iter().any(|r| r.result.is_err());
                for report in &reports {
                    match &report.result {
                        Ok(outcome) => sink.fetched(&report.citation_key, outcome),
                        Err(e) => sink.error(&format!("{}: {e}", report.citation_key)),
                    }
                }
                if failed {
                    std::process::exit(1);
                }
            }
            Err(e) => sink.error(&e.to_string()),
        },
    }
}
