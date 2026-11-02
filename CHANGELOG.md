# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [3.0.0](https://github.com/pax-project/pax-core/compare/v2.4.0...v3.0.0) - 2026-11-02

### Features

- *(aliases)* [**breaking**] address declared and excluded papers by a user-chosen alias ([#71](https://github.com/pax-project/pax-core/pull/71))

## [2.4.0](https://github.com/pax-project/pax-core/compare/v2.3.0...v2.4.0) - 2026-11-02

### Features

- *(search)* opt-in abstracts in search and citation results ([#58](https://github.com/pax-project/pax-core/pull/58))

## [2.3.0](https://github.com/pax-project/pax-core/compare/v2.2.0...v2.3.0) - 2026-11-02

### Features

- *(search-log)* label searches and count records per label in pax stats ([#60](https://github.com/pax-project/pax-core/pull/60))
- *(exclusions)* structured criterion and round, grouped by criterion in stats ([#59](https://github.com/pax-project/pax-core/pull/59))

## [2.2.0](https://github.com/pax-project/pax-core/compare/v2.1.0...v2.2.0) - 2026-11-01

### Features

- *(related)* pax related <seed>... ([#67](https://github.com/pax-project/pax-core/pull/67))
- *(related)* related / related_many, logged as related and counted in their own PRISMA box ([#66](https://github.com/pax-project/pax-core/pull/66))
- *(related)* provider support for related works (OpenAlex related_works, Semantic Scholar recommendations) ([#65](https://github.com/pax-project/pax-core/pull/65))

## [2.1.0](https://github.com/pax-project/pax-core/compare/v2.0.0...v2.1.0) - 2026-11-01

### Changed

- *(stats)* `reports_sought` counts the papers that passed screening, declared plus excluded at full text, instead of `records_screened - excluded_screening`, which counted every undecided paper as screened in. The two agree once every logged paper has a decision ([#57](https://github.com/pax-project/pax-core/pull/57))
- *(cli)* `refs` and `cited-by` exit 3 when a provider withheld its list, failed, or couldn't resolve the seed while another provider answered; a provider with no record of the seed used to leave the exit code at 0. A list cut short by `--limit` still exits 0 ([#56](https://github.com/pax-project/pax-core/pull/56))

### Features

- *(citations)* snowballing from several seeds with per-seed attribution ([#61](https://github.com/pax-project/pax-core/pull/61))
- *(citations)* typed reasons when a citation list is incomplete ([#56](https://github.com/pax-project/pax-core/pull/56))
- *(stats)* count papers awaiting a screening decision ([#57](https://github.com/pax-project/pax-core/pull/57))

### Fixed

- *(stats)* match exclusions keyed by provider id to the log by title+year ([#63](https://github.com/pax-project/pax-core/pull/63))
- *(add)* refuse excluded papers, and pax unexclude reverses an exclusion ([#55](https://github.com/pax-project/pax-core/pull/55))

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
