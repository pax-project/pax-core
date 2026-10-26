# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [2.0.0](https://github.com/pax-project/pax-core/compare/v1.1.1...v2.0.0) - 2026-10-26

### Breaking changes

- `PaxError`, `ProviderError`, `CandidateIdParseError` and `ProviderId` are `#[non_exhaustive]`: a `match` on them outside pax-core needs a wildcard arm. `PaxError` has new variants for exclusions, duplicate declarations and the search log ([#39](https://github.com/pax-project/pax-core/pull/39))
- The `pax` CLI exits with a distinct code per outcome (0 success, 1 failure, 2 wrong arguments, 3 some providers failed, 4 not found, 5 refused, 6 every provider failed, 7 no library), and writes errors and warnings to stderr, results to stdout ([#40](https://github.com/pax-project/pax-core/pull/40))
- `add_candidate` / `pax add` refuse a paper already in the library, matched by DOI or arXiv id, with `PaxError::AlreadyDeclared` ([#40](https://github.com/pax-project/pax-core/pull/40))

### Deprecated

- `known_dois` and `excluded_keys`: use `LibraryIndex`, which matches by every DOI and version-less arXiv id ([#41](https://github.com/pax-project/pax-core/pull/41))

### Features

- *(search)* paged search reporting each provider's total (`search_all_paged`, `search_by_author_paged`, `SearchOptions`/`Limit`/`SearchPage`); `pax search --limit N` / `--all` with per-provider "retrieved N of M" ([#27](https://github.com/pax-project/pax-core/pull/27))
- *(stats)* pax stats reports PRISMA flow-diagram counts ([#43](https://github.com/pax-project/pax-core/pull/43))
- *(search-log)* record every search in research/search-log.jsonl ([#42](https://github.com/pax-project/pax-core/pull/42))
- *(citations)* merge refs/cited-by and flag in-library and excluded papers by identity ([#41](https://github.com/pax-project/pax-core/pull/41))
- *(cli)* exit codes for every outcome, errors and warnings on stderr ([#40](https://github.com/pax-project/pax-core/pull/40))
- *(exclusions)* research/excluded.nix and pax exclude ([#33](https://github.com/pax-project/pax-core/pull/33))
- *(merge)* merge duplicate search results across providers ([#32](https://github.com/pax-project/pax-core/pull/32))
- *(citations)* pax refs and pax cited-by ([#36](https://github.com/pax-project/pax-core/pull/36))
- *(search)* filter by year, document type and language ([#38](https://github.com/pax-project/pax-core/pull/38))

### Fixed

- *(provider)* take PDF links only from real PDFs ([#37](https://github.com/pax-project/pax-core/pull/37))
- *(provider)* parse Crossref responses leniently ([#34](https://github.com/pax-project/pax-core/pull/34))
- *(provider)* OpenAlex and Semantic Scholar searches no longer abort on records missing a title, date, or author name ([#27](https://github.com/pax-project/pax-core/pull/27))

## [1.1.1](https://github.com/pax-project/pax-core/compare/v1.1.0...v1.1.1) - 2026-10-05

### Fixed

- *(provider)* replace abandoned crossref/arxiv-client deps ([#14](https://github.com/pax-project/pax-core/pull/14))
