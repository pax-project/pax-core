[![Release](https://img.shields.io/github/v/tag/pax-project/pax-core?label=release)](https://github.com/pax-project/pax-core/releases/tag/1.1.0)

**PAX** is a Rust CLI for discovering, declaring, managing, and reproducibly acquiring academic papers using Nix as the artifact backend.

The name pax comes from Latin pax, meaning peace. The idea is to bring order to the otherwise messy process of finding papers, resolving metadata, obtaining PDFs, managing references, and maintaining a reproducible research library.

The central principle is:

**PAX understands papers. Nix understands artifacts.**

PAX handles:
- academic search
- metadata
- source resolution
- paper declarations
- local organization
- bibliography export

Nix handles:
- fetching
- hashing
- verification
- caching
- storage
- reproducibility

The intended workflow is:

```Text
Search → Select → Declare → Fetch → Manage → Reproduce
```

## Installation

**Prebuilt binary (x86_64 Linux):** download `pax-1.1.0-x86_64-linux.tar.gz` from the [1.1.0 release](https://github.com/pax-project/pax-core/releases/tag/1.1.0), extract it, and put `pax` on your `PATH`:

```Bash
curl -LO https://github.com/pax-project/pax-core/releases/download/1.1.0/pax-1.1.0-x86_64-linux.tar.gz
tar xzf pax-1.1.0-x86_64-linux.tar.gz
./pax --version
```

**From source:** with the Nix devshell (`direnv allow`, or `nix develop`), then:

```Bash
cargo build --release
./target/release/pax --version
```

`pax` still needs a `nix` binary on `PATH` at runtime — `fetch`/`check`/`sync`/`open` all shell out to it — so the prebuilt binary alone isn't enough without Nix installed separately.

**Nix flake:** this repo is itself a flake. `nix build github:pax-project/pax-core` builds the `pax` binary as `packages.x86_64-linux.default`; `nix develop` gives the same devshell used for `cargo build` above.

home-manager users can install it via the flake's `homeModules.default` instead of hand-rolling `home.packages`:

```Nix
{
  inputs.pax.url = "github:pax-project/pax-core";

  # in your home-manager configuration:
  imports = [ pax.homeModules.default ];
  programs.pax.enable = true;
  # programs.pax.settings.ARXIV_CONTACT = "you@example.com";
}
```

```Bash
pax init
pax search "actor model"
pax add <result>
pax fetch <paper>
pax open <paper>
pax export bibtex
```

`pax search` queries OpenAlex, Crossref, Semantic Scholar, and arXiv, reports how much of each provider's result set it retrieved (`retrieved 50 of 1284`; `--limit N` or `--all` for more), and merges records that are the same paper into one row listing every provider's reference (`merged: 112 → 97 (15 duplicates, 2 unmatched)`). Records count as the same paper when they share a DOI or arXiv id, or an exact title and year without conflicting DOIs.

### Search filters

`pax search` (a query or `--author`) can narrow results by publication year, document type, and language:

```Bash
pax search "actor model" --year 2015-2020 --type conference-paper --lang en
```

- `--year`: `2020`, an inclusive range `2015-2020`, or an open range `2015-` / `-2020`.
- `--type`: `journal-article`, `conference-paper`, `preprint`, `review`, `book`, `book-chapter`, `thesis`, or `dataset`.
- `--lang`: an ISO 639-1 code, e.g. `en`.

Each provider applies a filter in its own query where its API supports it, so its reported total already reflects it. Where it can't, but its records carry the field, pax filters after retrieval — the output says so, because the provider's total then counts records the filter dropped. Where a provider has no such data at all, the output says the filter wasn't applied there.

| Filter | OpenAlex | Crossref | Semantic Scholar | arXiv |
| --- | --- | --- | --- | --- |
| year | server | server | server | server (submission date) |
| type | server | server (no `review` type: not applied) | server (no `preprint`/`thesis` type: not applied) | every record is a preprint; any other type matches nothing |
| language | server | after retrieval (records with no language are kept, marked `[language unknown]`) | not applied (no language data) | not applied (no language data) |

Crossref records often carry no language (proceedings papers never do), so its language filter can't check them. Rather than silently dropping every conference paper, pax keeps them, marks each such row `[language unknown]`, and says under Crossref's count how many it kept that way.

### Citation graph

`pax refs` lists the papers a paper cites; `pax cited-by` lists the papers citing it. Both read real citation edges from OpenAlex and Semantic Scholar (Crossref's publisher-deposited reference lists are often incomplete, and it has no forward citations):

```Bash
pax refs openalex:W3137220996
pax cited-by arxiv:1706.03762 --limit 200
pax cited-by vaswani2017 --year 2023- --type journal-article
```

The paper can be any `provider:id` reference from `search`, or the citation key of a paper in your library. pax looks it up in each provider by that provider's own id when it has one, otherwise by DOI, otherwise by arXiv id (for a library paper without a DOI, read from an arxiv.org source URL); a provider it can't be found in says so, and the other still answers.

Each provider's list is shown separately, with the same `retrieved N of M` counts as `search`, and papers already in your library are marked `[in library]`. They take the same `--limit`, `--all`, `--year`, `--type` and `--lang` flags. OpenAlex applies the filters server-side; Semantic Scholar's citation lists take no filters, so year and type are applied there after retrieval, and language not at all. Semantic Scholar serves at most 9,999 entries of a list, and skips references it couldn't match to a paper.

The project is designed to be:
- local-first
- reproducible
- scriptable
- Unix-friendly
- lazy

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
