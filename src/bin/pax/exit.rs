//! The `pax` exit codes, and the one place errors are mapped to them.
//! Commands report a [`Failure`] and take its [`code`]; they never pick a
//! number themselves.

use std::fmt;

use pax_core::{CitationError, CitationResults, PaxError, ProviderError};

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
    /// A paper added from a local file (`fetch --file`) isn't in this
    /// machine's Nix store, and nothing can download it: the user has to
    /// provide the file again.
    FileNeeded = 8,
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
        Failure::Pax(e) => match e {
            PaxError::Provider(e) => code(Failure::Provider(e)),
            // Every I/O error the library returns comes from reading or
            // writing the files under research/.
            PaxError::Io(_) | PaxError::LibraryParse(_) | PaxError::SearchLogParse { .. } => {
                Code::NoLibrary
            }
            PaxError::InvalidReference(_)
            | PaxError::InvalidExclusionReference(_)
            | PaxError::InvalidCitationKey(_)
            | PaxError::InvalidAlias(_)
            | PaxError::NoChangesSpecified
            | PaxError::MissingExclusionReason
            | PaxError::NotAFile(_)
            | PaxError::NotADirectory(_) => Code::Usage,
            PaxError::NotFound(_)
            | PaxError::NoSuchPaper(_)
            | PaxError::NotExcluded(_)
            | PaxError::ExclusionTitleUnresolved(..) => Code::NotFound,
            PaxError::CitationKeyExists(_)
            | PaxError::ExcludingDeclaredPaper(..)
            | PaxError::ExcludedNotDeclared { .. }
            | PaxError::NameTaken { .. }
            | PaxError::AlreadyDeclared { .. }
            | PaxError::AlreadyExcluded { .. }
            | PaxError::NoSourceUrl(_)
            | PaxError::HashMismatch { .. }
            | PaxError::BackupMismatch { .. } => Code::Refused,
            PaxError::LocalArtifactMissing(_) => Code::FileNeeded,
            PaxError::Fetch(_)
            | PaxError::Build(_)
            | PaxError::Upload(_)
            | PaxError::Backup { .. } => Code::Failure,
            // A variant added later, until it is mapped here.
            _ => Code::Failure,
        },
        Failure::Provider(e) => match e {
            ProviderError::NotFound => Code::NotFound,
            ProviderError::Request(_)
            | ProviderError::GetByIdUnsupported
            | ProviderError::Withheld { .. } => Code::ProvidersFailed,
            // A variant added later, until it is mapped here.
            _ => Code::Failure,
        },
        Failure::Citation(e) => match e {
            // Fetching the seed to learn its DOI failed: a provider with no
            // such work is not found, a failed request is an error.
            CitationError::Unresolvable {
                lookup_error: Some(lookup),
                ..
            } => code(Failure::Provider(&lookup.error)),
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

/// The exit code of a `refs` / `cited-by` / `related` lookup:
/// [`provider_results`], except that a provider not knowing the seed is no
/// longer an answer when another provider returned a list — that provider's
/// list is still missing, so the results are partial. A list cut short by
/// `--limit` (truncated), or a related list with no total, isn't: it's there.
pub fn citations(results: &CitationResults) -> Code {
    match provider_results(results.values()) {
        Code::Success
            if pax_core::incomplete_lists(results)
                .iter()
                .any(|incomplete| incomplete.reason.is_missing()) =>
        {
            Code::Partial
        }
        code => code,
    }
}

/// The exit code of a `refs` / `cited-by` / `related` run over several
/// seeds, from each seed's own code. A seed that fails fails only itself: every seed ending
/// the same way gives that code (so not found only if no seed resolves);
/// otherwise, partial if any seed got a list. With no list at all, a
/// malformed seed is still a usage error.
pub fn seeds(codes: impl IntoIterator<Item = Code>) -> Code {
    let codes: Vec<Code> = codes.into_iter().collect();
    let Some(&first) = codes.first() else {
        return Code::Success;
    };
    if codes.iter().all(|&code| code == first) {
        return first;
    }
    let listed = codes
        .iter()
        .any(|code| matches!(code, Code::Success | Code::Partial));
    if !listed && codes.contains(&Code::Usage) {
        Code::Usage
    } else {
        Code::Partial
    }
}

/// The exit code of `check` or `sync`, from the code of each paper with a
/// problem: success with none, [`Code::FileNeeded`] when every problem is a
/// local file to provide again, and a failure otherwise.
pub fn artifacts(problems: impl IntoIterator<Item = Code>) -> Code {
    let mut problems = problems.into_iter().peekable();
    if problems.peek().is_none() {
        Code::Success
    } else if problems.all(|code| code == Code::FileNeeded) {
        Code::FileNeeded
    } else {
        Code::Failure
    }
}

/// The exit code of a search whose results are good but whose search log
/// couldn't be written: a failure if the search otherwise succeeded, since
/// the review's record of it is missing; any other code (partial, not found,
/// every provider failed) already says the results fell short, and is kept.
pub fn log_failed(code: Code) -> Code {
    match code {
        Code::Success => Code::Failure,
        code => code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pax_core::ProviderId;

    fn pax(e: PaxError) -> Code {
        code(Failure::Pax(&e))
    }

    #[test]
    fn unexcluding_what_isnt_excluded_is_not_found() {
        assert_eq!(pax(PaxError::NotExcluded("10.1/x".into())), Code::NotFound);
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
        assert_eq!(pax(PaxError::InvalidAlias("P 1".into())), Code::Usage);
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

    fn unresolvable_after(error: ProviderError) -> CitationError {
        CitationError::Unresolvable {
            provider: ProviderId::SemanticScholar,
            lookup_error: Some(pax_core::SeedLookupError {
                provider: ProviderId::OpenAlex,
                error,
            }),
        }
    }

    #[test]
    fn a_seed_lookup_that_failed_on_the_network_is_an_error() {
        let unresolvable = unresolvable_after(ProviderError::Request("timed out".into()));
        assert_eq!(
            code(Failure::Citation(&unresolvable)),
            Code::ProvidersFailed
        );
        // The other provider answered: partial, not success.
        assert_eq!(
            providers([Ok(()), Err(Failure::Citation(&unresolvable))]),
            Code::Partial
        );
        // Neither did: every provider failed, not not-found.
        let failed = CitationError::Provider(ProviderError::Request("429".into()));
        assert_eq!(
            providers([
                Err(Failure::Citation(&failed)),
                Err(Failure::Citation(&unresolvable)),
            ]),
            Code::ProvidersFailed
        );
    }

    #[test]
    fn a_seed_its_own_provider_lacks_is_not_found() {
        let unresolvable = unresolvable_after(ProviderError::NotFound);
        assert_eq!(code(Failure::Citation(&unresolvable)), Code::NotFound);
        let unknown_seed = CitationError::SeedNotFound {
            provider: ProviderId::OpenAlex,
            key: "W1".into(),
        };
        assert_eq!(
            providers([
                Err(Failure::Citation(&unknown_seed)),
                Err(Failure::Citation(&unresolvable)),
            ]),
            Code::NotFound
        );
        assert_eq!(
            providers([Ok(()), Err(Failure::Citation(&unresolvable))]),
            Code::Success
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
        assert_eq!(
            pax(PaxError::ExcludedNotDeclared {
                name: "P18".into(),
                key: "10.1/x".into(),
            }),
            Code::Refused
        );
        assert_eq!(
            pax(PaxError::NameTaken {
                name: "P17".into(),
                holder: pax_core::NameHolder::Declared("a2020".into()),
            }),
            Code::Refused
        );
        assert_eq!(
            pax(PaxError::AlreadyDeclared {
                reference: "crossref:10.1/x".into(),
                citation_key: "a2020".into(),
            }),
            Code::Refused
        );
        assert_eq!(
            pax(PaxError::AlreadyExcluded {
                reference: "openalex:W1".into(),
                key: "10.1/x".into(),
                stage: pax_core::ExclusionStage::Screening,
                reason: "not empirical".into(),
            }),
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
        assert_eq!(
            pax(PaxError::SearchLogParse {
                line: 3,
                message: "bad".into()
            }),
            Code::NoLibrary
        );
        assert_eq!(pax(PaxError::LibraryParse("bad".into())), Code::NoLibrary);
        assert_eq!(code(Failure::NoLibrary), Code::NoLibrary);
    }

    fn page(retrieved: usize, total: u64) -> pax_core::SearchPage {
        let work = pax_core::CandidateWork {
            id: "openalex:W1".parse().unwrap(),
            title: "A Study".into(),
            authors: Vec::new(),
            publish_date: "2020".into(),
            doi: None,
            pdf_url: None,
            venue: None,
            abstract_text: None,
        };
        pax_core::SearchPage::new(vec![work; retrieved], Some(total), None)
    }

    fn citations_with(semantic_scholar: Result<pax_core::SearchPage, CitationError>) -> Code {
        citations(&CitationResults::from([
            (ProviderId::OpenAlex, Ok(page(1, 1))),
            (ProviderId::SemanticScholar, semantic_scholar),
        ]))
    }

    #[test]
    fn a_withheld_failed_or_unresolved_list_is_partial() {
        let withheld = CitationError::Provider(ProviderError::Withheld {
            message: "withheld".into(),
        });
        let failed = CitationError::Provider(ProviderError::Request("429".into()));
        let unresolved = CitationError::SeedNotFound {
            provider: ProviderId::SemanticScholar,
            key: "DOI:10.1/x".into(),
        };
        for error in [withheld, failed, unresolved] {
            assert_eq!(citations_with(Err(error)), Code::Partial);
        }
    }

    #[test]
    fn a_truncated_list_alone_is_success() {
        assert_eq!(citations_with(Ok(page(1, 40))), Code::Success);
        assert_eq!(citations_with(Ok(page(1, 1))), Code::Success);
    }

    #[test]
    fn a_seed_no_citation_provider_knows_is_still_not_found() {
        let unresolved = |provider| {
            Err(CitationError::Unresolvable {
                provider,
                lookup_error: None,
            })
        };
        let results = CitationResults::from([
            (ProviderId::OpenAlex, unresolved(ProviderId::OpenAlex)),
            (
                ProviderId::SemanticScholar,
                unresolved(ProviderId::SemanticScholar),
            ),
        ]);
        assert_eq!(citations(&results), Code::NotFound);
    }

    #[test]
    fn a_seed_that_fails_fails_only_itself() {
        assert_eq!(seeds([Code::Success, Code::Success]), Code::Success);
        assert_eq!(seeds([Code::Success, Code::NotFound]), Code::Partial);
        assert_eq!(seeds([Code::Usage, Code::Success]), Code::Partial);
        assert_eq!(seeds([Code::Success, Code::Partial]), Code::Partial);
        assert_eq!(
            seeds([Code::NotFound, Code::ProvidersFailed]),
            Code::Partial
        );
    }

    #[test]
    fn not_found_only_when_no_seed_resolves() {
        assert_eq!(seeds([Code::NotFound, Code::NotFound]), Code::NotFound);
        assert_eq!(seeds([Code::NotFound]), Code::NotFound);
        assert_eq!(seeds([Code::Usage, Code::NotFound]), Code::Usage);
    }

    #[test]
    fn a_local_file_to_provide_again_is_file_needed() {
        assert_eq!(
            pax(PaxError::LocalArtifactMissing("doe2020".into())),
            Code::FileNeeded
        );
        assert_eq!(
            pax(PaxError::HashMismatch {
                citation_key: "doe2020".into(),
                recorded: "sha256-a".into(),
                actual: "sha256-b".into(),
            }),
            Code::Refused
        );
        assert_eq!(pax(PaxError::NotAFile("x.pdf".into())), Code::Usage);
    }

    #[test]
    fn check_or_sync_is_file_needed_only_when_every_problem_is_a_missing_file() {
        assert_eq!(artifacts([]), Code::Success);
        assert_eq!(
            artifacts([Code::FileNeeded, Code::FileNeeded]),
            Code::FileNeeded
        );
        assert_eq!(artifacts([Code::FileNeeded, Code::Failure]), Code::Failure);
        assert_eq!(artifacts([Code::Refused]), Code::Failure);
    }

    #[test]
    fn a_failed_search_log_fails_only_an_otherwise_successful_search() {
        assert_eq!(log_failed(Code::Success), Code::Failure);
        for kept in [Code::Partial, Code::NotFound, Code::ProvidersFailed] {
            assert_eq!(log_failed(kept), kept);
        }
    }
}
