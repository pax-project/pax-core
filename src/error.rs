use thiserror::Error;

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
    #[error(transparent)]
    InvalidReference(#[from] CandidateIdParseError),
    #[error("no paper found for {0:?}: not declared locally, and not a valid candidate reference")]
    NotFound(String),
    #[error("no paper found with citation key {0:?}")]
    NoSuchPaper(String),
    #[error(
        "nothing to edit: specify --add-tag, --remove-tag, --notes, --rename, --title, \
         --author, --year, and/or --doi"
    )]
    NoChangesSpecified,
    #[error("no PDF source recorded for {0:?}; nothing to fetch")]
    NoSourceUrl(String),
    #[error("fetch failed: {0}")]
    Fetch(String),
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
    #[error("an exclusion needs a non-empty reason")]
    MissingExclusionReason,
    #[error("{0:?} is neither a DOI nor a provider:id reference")]
    InvalidExclusionReference(String),
    #[error("couldn't resolve a title for {0:?} ({1}); pass a title to exclude it anyway")]
    ExclusionTitleUnresolved(String, String),
    #[error("{0:?} is declared in the library as {1:?}; remove it before excluding it")]
    ExcludingDeclaredPaper(String, String),
}

/// Why the library refused an operation, for refusals that have no
/// [`PaxError`] variant of their own.
///
/// [`PaxError`] isn't `#[non_exhaustive]`, so a new variant would break
/// callers that match it exhaustively. A refusal travels instead as
/// [`PaxError::Io`] holding an [`std::io::ErrorKind::AlreadyExists`] error
/// that wraps it, displayed as the refusal itself; [`PaxError::refusal`]
/// gets it back out.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Refusal {
    /// The paper `reference` resolves to is already declared in the
    /// library, as `citation_key`: by the same DOI, or the same arXiv id
    /// in any version.
    #[error("{reference} is already in the library as {citation_key:?}")]
    #[non_exhaustive]
    AlreadyDeclared {
        reference: String,
        citation_key: String,
    },
}

impl From<Refusal> for PaxError {
    fn from(refusal: Refusal) -> Self {
        PaxError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            refusal,
        ))
    }
}

impl PaxError {
    /// The [`Refusal`] this error carries, if it's one.
    pub fn refusal(&self) -> Option<&Refusal> {
        match self {
            PaxError::Io(e) => e.get_ref()?.downcast_ref(),
            _ => None,
        }
    }
}
