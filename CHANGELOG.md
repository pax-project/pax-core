# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.1.0](https://github.com/pax-project/pax-core/releases/tag/v1.1.0) - 2026-10-05

### Features

- *(edit)* add citation-key rename and Identity corrections
- *(search)* add search --author/--doi/--local and list filters
- *(display)* show venue, abstract, in-library flag, and artifact status
- *(sync)* add pax sync to batch-materialize the whole library
- *(open)* add pax open to launch a paper's PDF in the configured viewer
- *(check)* add pax check to verify declared artifacts reproduce
- *(fetch)* add pax fetch to materialize artifacts through Nix
- *(add)* resolve PDF source URL from each provider
- *(export)* add pax export bibtex
- *(edit)* add pax edit for tags and notes
- *(remove)* add pax remove
- *(show)* make pax show dual-mode for declared papers and candidates
- *(list)* add pax list
- *(add)* add pax add with deterministic citation-key generation
- *(show)* add pax show, resolving a single candidate reference
- *(search)* Added support for ArXiV search
- *(search)* Semmantic School support added
- *(search)* added crossref for searching
- *(init)* added the library initialization

### Fixed

- *(nix)* correct flake template's fetchurl to read source_url, not url
- *(config)* move hardcoded Semantic Scholar key and arXiv contact to config
