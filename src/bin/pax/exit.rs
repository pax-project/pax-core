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
#[derive(Clone, Copy)]
pub enum Failure<'a> {
    Pax(&'a PaxError),
    Provider(&'a ProviderError),
    Citation(&'a CitationError),
    /// `pax init`, whose library call returns a bare I/O error.
    Init(&'a std::io::Error),
    /// Arguments clap accepted but the command can't use together.
    Usage(&'a str),
    /// No library in the current directory: `research/papers.nix` is missing.
    NoLibrary,
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
            Failure::NoLibrary => f.write_str(
                "no pax library here (research/papers.nix not found); run pax init first",
            ),
        }
    }
}

/// The exit code a failure maps to.
pub fn code(failure: Failure<'_>) -> Code {
    match failure {
        // A refusal without a variant of its own, carried as an I/O error.
        Failure::Pax(e) if e.refusal().is_some() => Code::Refused,
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
        Failure::NoLibrary => Code::NoLibrary,
    }
}

/// Whether a provider's failure is really an answer: it has no record of the
/// seed or DOI it was asked about. Unlike a network failure, a rate limit or
/// a provider error, that leaves nothing missing from the results.
pub fn is_answer(failure: Failure<'_>) -> bool {
    code(failure) == Code::NotFound
}

/// The exit code of a command that asked several providers, from each one's
/// outcome. A provider that doesn't know the seed or DOI has answered (see
/// [`is_answer`]), so: not found if every provider said so, success if no
/// provider failed, every provider failed if none answered at all, and
/// partial if some failed while others answered.
pub fn providers<'a>(outcomes: impl IntoIterator<Item = Result<(), Failure<'a>>>) -> Code {
    let (mut listed, mut not_found, mut failed) = (0u32, 0u32, 0u32);
    for outcome in outcomes {
        match outcome {
            Ok(()) => listed += 1,
            Err(failure) if is_answer(failure) => not_found += 1,
            Err(_) => failed += 1,
        }
    }
    match (listed, not_found, failed) {
        (0, 1.., 0) => Code::NotFound,
        (_, _, 0) => Code::Success,
        (0, 0, _) => Code::ProvidersFailed,
        _ => Code::Partial,
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
        assert_eq!(providers([Ok(()), Ok(())]), Code::Success);
    }

    #[test]
    fn a_provider_not_knowing_the_seed_or_doi_is_an_answer() {
        let unknown_seed = CitationError::SeedNotFound {
            provider: ProviderId::SemanticScholar,
            key: "DOI:10.1/x".into(),
        };
        let unresolvable = CitationError::Unresolvable {
            provider: ProviderId::OpenAlex,
            lookup_error: None,
        };
        let unknown_doi = ProviderError::NotFound;
        let failed = CitationError::Provider(ProviderError::Request("429".into()));

        // The other providers answered: success, not partial.
        assert_eq!(
            providers([Ok(()), Err(Failure::Citation(&unknown_seed))]),
            Code::Success
        );
        assert_eq!(
            providers([Err(Failure::Citation(&unresolvable)), Ok(())]),
            Code::Success
        );
        assert_eq!(
            providers([Ok(()), Err(Failure::Provider(&unknown_doi)), Ok(())]),
            Code::Success
        );

        // No provider knows it: not found.
        assert_eq!(
            providers([
                Err(Failure::Citation(&unknown_seed)),
                Err(Failure::Citation(&unresolvable)),
            ]),
            Code::NotFound
        );

        // A real failure beside it still leaves the answer incomplete.
        assert_eq!(
            providers([
                Err(Failure::Citation(&unknown_seed)),
                Err(Failure::Citation(&failed)),
            ]),
            Code::Partial
        );
        assert_eq!(
            providers([
                Ok(()),
                Err(Failure::Citation(&unknown_seed)),
                Err(Failure::Citation(&failed)),
            ]),
            Code::Partial
        );
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
        let citation_failed = CitationError::Provider(ProviderError::Request("429".into()));
        assert_eq!(
            providers([
                Err(Failure::Provider(&failed)),
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
        assert_eq!(code(Failure::NoLibrary), Code::NoLibrary);
    }
}
