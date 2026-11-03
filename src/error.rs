use std::path::PathBuf;

use thiserror::Error;

use crate::exclusions::ExclusionStage;
use crate::names::NameHolder;
use crate::provider::{CandidateIdParseError, ProviderError};

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PaxError {
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("failed to parse library: {0}")]
    LibraryParse(String),
    /// A line of `research/search-log.jsonl` that isn't a log entry pax can
    /// read, numbered from 1.
    #[error("failed to parse research/search-log.jsonl, line {line}: {message}")]
    SearchLogParse { line: usize, message: String },
    #[error(transparent)]
    InvalidReference(#[from] CandidateIdParseError),
    #[error("no paper found for {0:?}: not declared locally, and not a valid candidate reference")]
    NotFound(String),
    #[error("no paper found with citation key or alias {0:?}")]
    NoSuchPaper(String),
    /// `name` is the alias of an excluded paper (exclusion key `key`), given
    /// to a command that needs a declared one.
    #[error("{name} is excluded, not in the library (excluded as {key})")]
    ExcludedNotDeclared { name: String, key: String },
    #[error(
        "nothing to edit: specify --add-tag, --remove-tag, --notes, --rename, --alias, --title, \
         --author, --year, and/or --doi"
    )]
    NoChangesSpecified,
    #[error(
        "no PDF source recorded for {0:?}; nothing to fetch (add a copy you have with `pax fetch \
         {0} --file <path>`)"
    )]
    NoSourceUrl(String),
    #[error("fetch failed: {0}")]
    Fetch(String),
    /// The path given to [`crate::fetch_file`] isn't a readable file.
    #[error("{} is not a file", .0.display())]
    NotAFile(PathBuf),
    /// The file given to [`crate::fetch_file`] isn't the content already
    /// recorded for the paper: its hash differs.
    #[error(
        "{citation_key} already has an artifact with hash {recorded}, but this file's hash is \
         {actual}; pass --force to replace it"
    )]
    HashMismatch {
        citation_key: String,
        recorded: String,
        actual: String,
    },
    /// The paper's artifact was added from a local file
    /// ([`crate::Artifact::is_local`]) that isn't in this machine's Nix
    /// store, so nothing can fetch it: the user has to provide it again.
    #[error(
        "{0} was added from a local file, which isn't in this Nix store; provide it again with \
         `pax fetch {0} --file <path>`"
    )]
    LocalArtifactMissing(String),
    #[error("build failed: {0}")]
    Build(String),
    #[error("upload failed: {0}")]
    Upload(String),
    #[error("citation key {0:?} is already in use")]
    CitationKeyExists(String),
    #[error(
        "{0:?} is not a valid citation key (must start with a letter or underscore, and \
         contain only letters, digits, underscores, and hyphens)"
    )]
    InvalidCitationKey(String),
    #[error(
        "{0:?} is not a valid alias (use only letters, digits, underscores, dots, and hyphens)"
    )]
    InvalidAlias(String),
    /// `name` can't be given to a paper as an alias or citation key: another
    /// paper, `holder`, already has it as one. Citation keys and aliases in
    /// `papers.nix` and `excluded.nix` are one namespace.
    #[error("{name:?} is already taken by {holder}")]
    NameTaken { name: String, holder: NameHolder },
    #[error("an exclusion needs a non-empty reason")]
    MissingExclusionReason,
    #[error("{0:?} is neither a DOI nor a provider:id reference")]
    InvalidExclusionReference(String),
    #[error("couldn't resolve a title for {0:?} ({1}); pass a title to exclude it anyway")]
    ExclusionTitleUnresolved(String, String),
    #[error("{0:?} is declared in the library as {1:?}; remove it before excluding it")]
    ExcludingDeclaredPaper(String, String),
    /// The paper `reference` resolves to is already declared in the library,
    /// as `citation_key`: by the same DOI, or the same arXiv id in any
    /// version.
    #[error("{reference} is already in the library as {citation_key:?}")]
    AlreadyDeclared {
        reference: String,
        citation_key: String,
    },
    /// The paper `reference` resolves to is excluded in `excluded.nix`, under
    /// `key`: by the same DOI, the same arXiv id in any version, or the same
    /// `provider:id`. Reversing that decision is its own step
    /// ([`crate::unexclude_paper`]).
    #[error(
        "{reference} was excluded at {stage} ({reason:?}, key {key}); run `pax unexclude {key}` \
         first to reverse that decision"
    )]
    AlreadyExcluded {
        reference: String,
        key: String,
        stage: ExclusionStage,
        reason: String,
    },
    /// Nothing in `excluded.nix` matches the reference given to
    /// [`crate::unexclude_paper`].
    #[error("{0:?} isn't excluded: no entry in research/excluded.nix matches it")]
    NotExcluded(String),
}
