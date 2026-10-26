# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [2.0.0](https://github.com/pax-project/pax-core/compare/v1.1.1...v2.0.0) - 2026-10-26

### Features

- *(stats)* pax stats reports PRISMA flow-diagram counts ([#43](https://github.com/pax-project/pax-core/pull/43))
- *(search-log)* record every search in research/search-log.jsonl ([#42](https://github.com/pax-project/pax-core/pull/42))
- *(citations)* merge refs/cited-by and flag in-library and excluded papers by identity ([#41](https://github.com/pax-project/pax-core/pull/41))
- *(cli)* exit codes for every outcome, errors and warnings on stderr ([#40](https://github.com/pax-project/pax-core/pull/40))
- *(exclusions)* research/excluded.nix and pax exclude ([#33](https://github.com/pax-project/pax-core/pull/33))
- *(merge)* merge duplicate search results across providers ([#32](https://github.com/pax-project/pax-core/pull/32))
- *(citations)* pax refs and pax cited-by ([#36](https://github.com/pax-project/pax-core/pull/36))
- *(search)* filter by year, document type and language ([#38](https://github.com/pax-project/pax-core/pull/38))

### Fixed

- *(crossref)* keep paged search futures Send ([#44](https://github.com/pax-project/pax-core/pull/44))
- *(provider)* take PDF links only from real PDFs ([#37](https://github.com/pax-project/pax-core/pull/37))
- *(provider)* parse Crossref responses leniently ([#34](https://github.com/pax-project/pax-core/pull/34))

## [1.1.1](https://github.com/pax-project/pax-core/compare/v1.1.0...v1.1.1) - 2026-10-05

### Fixed

- *(provider)* replace abandoned crossref/arxiv-client deps ([#14](https://github.com/pax-project/pax-core/pull/14))
