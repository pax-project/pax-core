//! The `pax` exit codes, and the one place errors are mapped to them.
//! Commands report a [`Failure`] and take its [`code`]; they never pick a
//! number themselves.

use std::fmt;

use pax_core::{CitationError, PaxError, ProviderError};

/// How a command ended. The numbers are part of the CLI's interface: scripts
/// and agents branch on them, so they never change meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    Success = 0,
    /// Any other failure; also `check` or `sync` finding problems.
    Failure = 1,
    /// Wrong arguments: clap's own exit code, also used for arguments it
    /// can't validate itself (e.g. a malformed `provider:id`).
    Usage = 2,
    /// Some providers answered and at least one failed: the results are
    /// incomplete.
    Partial = 3,
    /// A reference, seed or citation key doesn't resolve.
    NotFound = 4,
    /// The library rejected the operation.
    Refused = 5,
    /// Every provider failed.
    ProvidersFailed = 6,
    /// The library is missing, not initialized, or unreadable.
    NoLibrary = 7,
}

impl From<Code> for std::process::ExitCode {
    fn from(code: Code) -> Self {
        Self::from(code as u8)
    }
}

/// Something that made a command fail, from the library or the CLI itself.
pub enum Failure<'a> {
    Pax(&'a PaxError),
    Provider(&'a ProviderError),
    Citation(&'a CitationError),
    /// `pax init`, whose library call returns a bare I/O error.
    Init(&'a std::io::Error),
    /// Arguments clap accepted but the command can't use together.
    Usage(&'a str),
    /// Anything else, described by its message.
    Other(&'a str),
}

impl<'a> From<&'a PaxError> for Failure<'a> {
    fn from(e: &'a PaxError) -> Self {
        Failure::Pax(e)
    }
}

impl<'a> From<&'a ProviderError> for Failure<'a> {
    fn from(e: &'a ProviderError) -> Self {
        Failure::Provider(e)
    }
}

impl<'a> From<&'a CitationError> for Failure<'a> {
    fn from(e: &'a CitationError) -> Self {
        Failure::Citation(e)
    }
}

impl fmt::Display for Failure<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Pax(e) => e.fmt(f),
            Failure::Provider(e) => e.fmt(f),
            Failure::Citation(e) => e.fmt(f),
            Failure::Init(e) => e.fmt(f),
            Failure::Usage(message) | Failure::Other(message) => f.write_str(message),
        }
    }
}

/// The exit code a failure maps to.
pub fn code(failure: Failure<'_>) -> Code {
    match failure {
        Failure::Pax(e) => match e {
            PaxError::Provider(e) => code(Failure::Provider(e)),
            // Every I/O error the library returns comes from reading or
            // writing the files under research/.
            PaxError::Io(_) | PaxError::LibraryParse(_) => Code::NoLibrary,
            PaxError::InvalidReference(_)
            | PaxError::InvalidExclusionReference(_)
            | PaxError::InvalidCitationKey(_)
            | PaxError::NoChangesSpecified
            | PaxError::MissingExclusionReason => Code::Usage,
            PaxError::NotFound(_)
            | PaxError::NoSuchPaper(_)
            | PaxError::ExclusionTitleUnresolved(..) => Code::NotFound,
            PaxError::CitationKeyExists(_)
            | PaxError::ExcludingDeclaredPaper(..)
            | PaxError::NoSourceUrl(_) => Code::Refused,
            PaxError::Fetch(_) | PaxError::Build(_) | PaxError::Upload(_) => Code::Failure,
            // A variant added later, until it is mapped here.
            _ => Code::Failure,
        },
        Failure::Provider(e) => match e {
            ProviderError::NotFound => Code::NotFound,
            ProviderError::Request(_) | ProviderError::GetByIdUnsupported => Code::ProvidersFailed,
            // A variant added later, until it is mapped here.
            _ => Code::Failure,
        },
        Failure::Citation(e) => match e {
            CitationError::Unresolvable { .. } | CitationError::SeedNotFound { .. } => {
                Code::NotFound
            }
            CitationError::Provider(e) => code(Failure::Provider(e)),
            // A variant added later, until it is mapped here.
            _ => Code::Failure,
        },
        Failure::Init(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Code::Refused,
        Failure::Init(_) | Failure::Other(_) => Code::Failure,
        Failure::Usage(_) => Code::Usage,
    }
}

/// The exit code of a command that asked several providers, from each one's
/// outcome: success if all answered, partial if some failed, and if none
/// answered, not found when each said so, otherwise every provider failed.
pub fn providers<'a>(outcomes: impl IntoIterator<Item = Result<(), Failure<'a>>>) -> Code {
    let (mut answered, mut not_found, mut failed) = (0u32, 0u32, 0u32);
    for outcome in outcomes {
        match outcome.map_err(code) {
            Ok(()) => answered += 1,
            Err(Code::NotFound) => not_found += 1,
            Err(_) => failed += 1,
        }
    }
    match (answered, not_found, failed) {
        (_, 0, 0) => Code::Success,
        (1.., _, _) => Code::Partial,
        (0, _, 0) => Code::NotFound,
        (0, _, _) => Code::ProvidersFailed,
    }
}

/// [`providers`] over a library call's per-provider results.
pub fn provider_results<'a, T: 'a, E: 'a>(
    results: impl IntoIterator<Item = &'a Result<T, E>>,
) -> Code
where
    &'a E: Into<Failure<'a>>,
{
    providers(
        results
            .into_iter()
            .map(|result| result.as_ref().map(|_| ()).map_err(Into::into)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pax_core::ProviderId;

    fn pax(e: PaxError) -> Code {
        code(Failure::Pax(&e))
    }

    #[test]
    fn a_fetch_or_build_failure_is_a_plain_failure() {
        assert_eq!(
            pax(PaxError::Fetch("connection refused".into())),
            Code::Failure
        );
        assert_eq!(pax(PaxError::Build("nix exited 1".into())), Code::Failure);
    }

    #[test]
    fn arguments_the_library_rejects_are_usage_errors() {
        let bad_ref = "nope".parse::<pax_core::CandidateId>().unwrap_err();
        assert_eq!(pax(PaxError::InvalidReference(bad_ref)), Code::Usage);
        assert_eq!(pax(PaxError::NoChangesSpecified), Code::Usage);
        assert_eq!(code(Failure::Usage("specify one")), Code::Usage);
    }

    #[test]
    fn some_providers_failing_is_partial() {
        let failed = ProviderError::Request("rate limited".into());
        assert_eq!(
            providers([Ok(()), Err(Failure::Provider(&failed))]),
            Code::Partial
        );
        let unknown_seed = CitationError::SeedNotFound {
            provider: ProviderId::SemanticScholar,
            key: "DOI:10.1/x".into(),
        };
        assert_eq!(
            providers([Ok(()), Err(Failure::Citation(&unknown_seed))]),
            Code::Partial
        );
        assert_eq!(providers([Ok(()), Ok(())]), Code::Success);
    }

    #[test]
    fn an_unresolved_reference_or_key_is_not_found() {
        assert_eq!(pax(PaxError::NoSuchPaper("bogus".into())), Code::NotFound);
        assert_eq!(pax(PaxError::NotFound("bogus".into())), Code::NotFound);
        assert_eq!(
            pax(PaxError::Provider(ProviderError::NotFound)),
            Code::NotFound
        );
        let unknown_seed = CitationError::SeedNotFound {
            provider: ProviderId::OpenAlex,
            key: "W1".into(),
        };
        let unresolvable = CitationError::Unresolvable {
            provider: ProviderId::SemanticScholar,
            lookup_error: None,
        };
        assert_eq!(
            providers([
                Err(Failure::Citation(&unknown_seed)),
                Err(Failure::Citation(&unresolvable)),
            ]),
            Code::NotFound
        );
    }

    #[test]
    fn the_library_refusing_is_refused() {
        assert_eq!(
            pax(PaxError::ExcludingDeclaredPaper(
                "10.1/x".into(),
                "a2020".into()
            )),
            Code::Refused
        );
        assert_eq!(
            pax(PaxError::CitationKeyExists("a2020".into())),
            Code::Refused
        );
        let exists = std::io::Error::from(std::io::ErrorKind::AlreadyExists);
        assert_eq!(code(Failure::Init(&exists)), Code::Refused);
    }

    #[test]
    fn every_provider_failing_is_providers_failed() {
        let failed = ProviderError::Request("timed out".into());
        assert_eq!(code(Failure::Provider(&failed)), Code::ProvidersFailed);
        let unknown_seed = CitationError::SeedNotFound {
            provider: ProviderId::OpenAlex,
            key: "W1".into(),
        };
        let citation_failed = CitationError::Provider(ProviderError::Request("429".into()));
        assert_eq!(
            providers([
                Err(Failure::Citation(&unknown_seed)),
                Err(Failure::Citation(&citation_failed)),
            ]),
            Code::ProvidersFailed
        );
    }

    #[test]
    fn a_missing_or_unparseable_library_is_no_library() {
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert_eq!(pax(PaxError::Io(missing)), Code::NoLibrary);
        assert_eq!(pax(PaxError::LibraryParse("bad".into())), Code::NoLibrary);
    }
}
