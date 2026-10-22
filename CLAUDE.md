# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

PAX is a Rust project for discovering, declaring, managing, and reproducibly acquiring academic papers, using Nix as the artifact backend. The design principle: **PAX understands papers, Nix understands artifacts.**

- PAX handles: academic search, metadata resolution, source resolution, paper declarations, local organization, bibliography export.
- Nix handles: fetching, hashing, verification, caching, storage, garbage collection.

Intended workflow: `Search → Select → Declare → Fetch → Manage → Reproduce` (see [`mvp.md` at tag `1.0.0`](https://github.com/pax-project/pax-core/blob/1.0.0/docs/mvp.md) for the original command surface and design philosophy — its full §4 command surface is built; ongoing work is tracked in this repo's Issues, see "Known gaps" below).

**This repository's primary artifact is `pax-core`, a library — not a CLI tool that happens to have a core module.** The `pax` binary (`src/bin/pax/main.rs`) is a thin, optional client of that library, gated behind the `cli` Cargo feature; [`lazypax`](https://github.com/pax-project/lazy-pax) is a separate repo that's just another client of the same library — README.md itself doesn't mention it, since lazy-pax depends on pax-core and not the other way around. [`mvp.md`](https://github.com/pax-project/pax-core/blob/1.0.0/docs/mvp.md) is the original, pre-implementation spec for the full command surface — don't assume a command or flag exists just because it's documented there. **Planned and in-progress work is tracked in this repo's [Issues](https://github.com/pax-project/pax-core/issues) and the org's [pax Project](https://github.com/orgs/pax-project/projects/2)** — check them before assuming something is done or not done. `mvp.md` and the 1.0.0-era `status.md` survive only at tag `1.0.0` — see "Known gaps" below.

## Environment

The project uses Nix + direnv for the dev shell (`flake.nix`, `.envrc`). `.envrc` does `use flake`, so `direnv allow` sets up a shell with cargo/rustc/rustfmt/clippy/rust-analyzer and the OpenSSL env vars (`PKG_CONFIG_PATH`, `OPENSSL_DIR`) needed to build the native TLS dependencies.

Secrets/config load via `dotenvy` from `.env` at startup, but only in the `pax` binary (`dotenvy::dotenv().ok()` in `src/bin/pax/main.rs`'s `main()`) — the library itself never reads the environment directly; it's passed a `pax_core::Config` instead. Recognized vars: `SEMANTIC_SCHOLAR_API_KEY` (Semantic Scholar falls back to unauthenticated requests if unset) and `ARXIV_CONTACT` (arXiv's contact-email etiquette header, omitted if unset).

## Commands

```bash
cargo build                        # builds the library + the pax binary (cli is a default feature)
cargo build --no-default-features  # builds pax-core alone — proves the CLI deps (clap, dotenvy, tokio's runtime) aren't pulled in
cargo run --bin pax -- <args>      # e.g. cargo run --bin pax -- search "actor model"
cargo test                         # unit tests (library.rs round-trip, provider CandidateId parsing)
cargo test <substring>             # run a single test, e.g. cargo test candidate_id_rejects_bare_doi
cargo fmt                          # format (rustfmt is in the flake devshell)
cargo clippy                       # lint
```

## Architecture

```
src/
  lib.rs          # pax-core's public API: re-exports + search_all()
  citations.rs    # references()/cited_by(): resolve a seed per provider, page its citation lists
  provider/       # Provider trait, ProviderId, CandidateId, CandidateWork, ProviderError
    mod.rs        #   + the four provider implementations, one file each; collect_pages() drives paging
    filter.rs     # Filters (year/type/language) and AppliedFilter: how each provider applied each one
  paper.rs        # Paper, Identity, Artifact, Local, PaperRef (citation key)
  library.rs      # Library::load()/save() against research/papers.nix
  library_index.rs # LibraryIndex: is a result declared or excluded, matched by DOI/arXiv identity key
  identity.rs     # IdentityKey (DOI → arXiv id → title+year) + group(): which records are the same paper
  merge.rs        # merge_works/merge_pages: duplicates collapsed into MergedWork, with before/after/unmatched counts
  exclusions.rs   # Exclusions::load()/save() against research/excluded.nix; exclusion_key()
  nix_attrs.rs    # the attrset subset both files use: tokenizer, parser, field writers, atomic write
  nix.rs          # init_library(): scaffolds research/flake.nix + research/papers.nix from templates/
  citation_key.rs # generates a citation key for a newly declared paper (surname+year, disambiguated)
  bibtex.rs       # renders declared papers as BibTeX (bibtex::render)
  github.rs       # publish_to_github_release: shells out to `gh` to publish a local PDF as a release asset
  process.rs      # shared subprocess-with-timeout helper for anything shelling out (nix, gh)
  error.rs        # PaxError, unifying ProviderError + io/parse errors
  bin/
    pax/
      main.rs     # clap CLI — argument parsing, dispatch, output formatting only
      sink.rs     # output formatting helpers for the CLI
```

**Reference types — the load-bearing design decision in this codebase:** `add`/`show` and `search` operate on fundamentally different address spaces, so they use different types:
- **`CandidateId`** (`provider/mod.rs`) — a fully-qualified `provider:native_id` reference (e.g. `openalex:W2741809807`, `arxiv:2301.01234`) to a single, unresolved search hit. Its `FromStr` impl is deliberately strict: it rejects a bare DOI, a title, or free text (see `provider::tests` for the exact rejected/accepted cases). This makes "search returned two results, which one did you mean?" structurally impossible at the `add`/`show` argument boundary — disambiguation is `search`'s job (it already returns a list); `add`/`show` only ever accept a reference that is already unambiguous.
- **`PaperRef`** (`paper.rs`) — a citation key (e.g. `turing1936`) addressing the closed set of papers already declared in the local library. Used by `remove`/`edit`/`fetch`/`open` (`sync` operates on the whole library rather than a single `PaperRef`, but reuses `fetch_paper` per key internally — see #7).

**`Provider` trait** (`provider/mod.rs`) — `fn id()`, `async fn search(query)`, `async fn get(native_id)`. One implementation per provider module (`openalex.rs`, `crossref.rs`, `semantic_scholar.rs`, `arxiv.rs`), each also providing `From<ProviderNativeType> for CandidateWork`. `get()` is implemented for all four today (`OpenAlexClient::get_work`, Crossref `/works/{doi}`, `SemanticScholar::get_paper`, arXiv `fetch_page` by id). Crossref is the exception to "one crate per provider": the `crossref` crate builds URLs and supplies the HTTP client, but `crossref.rs` parses responses into its own lenient types, because the crate's `Work` requires fields (e.g. `title`) that real records omit.

**`pax::search_all(query)`** fans out to all four providers concurrently-in-sequence (awaited one after another, not via `join!`) and returns `HashMap<ProviderId, Result<Vec<CandidateWork>, ProviderError>>` — a failing provider doesn't fail the whole search, and the *caller* (the CLI) decides how to display an error, since the library itself never prints.

**Citation graph** (`citations.rs`) — `references(seed, …)` / `cited_by(seed, …)` return the same per-provider `HashMap<ProviderId, Result<SearchPage, CitationError>>` shape as `search_all_paged`, for OpenAlex and Semantic Scholar only (`CITATION_PROVIDERS`). The seed accepts both address spaces: a `CandidateId` from any provider, or a `PaperRef` citation key (told apart by the colon, as in `show_reference`). `SeedIds` resolves it to each provider's lookup key — its own id if the seed came from it, else `doi:`/`DOI:` (a `CandidateId` seed is fetched from its own provider to learn the DOI), else the arXiv id (from an arXiv seed, an arXiv DOI, or a library paper's arxiv.org `source_url`) — and a provider it can't be resolved in or found in gets a `CitationError` while the other still answers. The lists are the provider-level `OpenAlexProvider::references_page`/`citations_page` (`cited_by:`/`cites:` work filters, so search filters apply server-side) and `SemanticScholarProvider::references_page`/`citations_page`. The latter fetch the list endpoints with their own `reqwest` client and lenient types rather than the `semantic_scholar` crate, whose `Paper` requires a `paperId` that unmatched references lack; their total is the seed's `referenceCount`/`citationCount`, and year/type filters run after retrieval. The library returns the lists per provider; the CLI prints each provider's count, then merges them with `merge_pages` (generic over the error type, so it takes `CitationResults` as well as search results), like `search`.

**`Library`** (`library.rs`) — `research/papers.nix` is Nix syntax, but it's a schema PAX itself invented and fully controls (a flat attrset of `citation_key = { ...fields... };` records; `research/flake.nix` only ever reads `source_url`/`hash` off each entry generically via `builtins.mapAttrs`, so extra fields are harmless to it). `Library::load`/`save` are a small hand-rolled tokenizer/parser and writer scoped to exactly that fixed shape — deliberately not a general Nix-language parser (no `rnix-parser` or similar dependency). `research/papers.nix` is the single source of truth; nothing else shadows it. `add`/`edit`/`remove` all mutate a `Library` today; round-trip correctness is proven by a unit test in `library.rs`. The tokenizer/parser and writers themselves live in `nix_attrs.rs`, shared with `excluded.nix`; `library.rs` only maps entries to `Paper`s.

**`Exclusions`** (`exclusions.rs`) — `research/excluded.nix` is the library's blacklist: papers screened and decided against, keyed by normalized DOI (or `provider:id` for a work without one, see `exclusion_key`), each with a `title`, a required `reason`, and a `stage` (`screening` / `full-text`, the two PRISMA exclusion boxes). Deliberately a separate file: `papers.nix` and the flake are untouched, and a missing file means no exclusions. `exclude_paper` (`pax exclude`) refuses a paper already in `papers.nix`; `excluded_keys(root)` + `exclusion_key(work)` are how callers flag results, mirroring `known_dois`.

**`pax init`** (`nix.rs::init_library`) materializes `research/flake.nix` + an empty `research/papers.nix` from embedded templates (`include_str!` from `templates/flake.nix.template` / `templates/papers.nix.template`). The `research/` directory is gitignored; the top-level `research/flake.nix` / `research/papers.nix` checked into *this* repo are a working example of that generated structure, not the templates themselves.

**A real user's research library is expected to be its own git repo** (`mvp.md` §6: `git clone <research-library>; cd <research-library>; nix build`), and Nix flakes only evaluate files that are tracked by git — an untracked `flake.nix` fails outright with "not tracked by Git" (confirmed: this is stock Nix behavior, not a PAX bug). So in a library that's already a git repo, `research/flake.nix`/`research/papers.nix` need at least `git add` (staged, not necessarily committed) before `pax open`/any other command that calls `nix build` will work — see #7's reproducibility-proof section for how this was found.

## Known gaps

This repo's [Issues](https://github.com/pax-project/pax-core/issues) (grouped
by milestone) and the org-wide [pax Project](https://github.com/orgs/pax-project/projects/2)
are the single, kept-current source of truth for what's planned, in progress,
or done — check them rather than this file, so gap status never has to be
kept in sync in two places, and this repo's own docs never drift into
describing a moving target. The MVP-era checklist and the design gotchas
found while building it live in #7.
