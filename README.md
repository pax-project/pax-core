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

`pax search` queries OpenAlex, Crossref, Semantic Scholar, and arXiv, reports how much of each provider's result set it retrieved (`retrieved 50 of 1284`; `--limit N` or `--all` for more), and merges records that are the same paper into one row listing every provider's reference (`merged: 112 → 97 (15 duplicates, 2 unmatched)`). Records count as the same paper when they share a DOI or arXiv id, or an exact title and year without conflicting DOIs. Each merged row shows the published version when there is one (a record with a venue, or a DOI that isn't an arXiv DOI) rather than a preprint, then the most complete record, so its reference is the one to cite and `pax add`.

`pax add <result>` declares a search result in `research/papers.nix`, recording its open PDF link as `source_url` for `pax fetch`. If the resolved record has no PDF but has a DOI, `add` looks the DOI up in the other providers and takes a PDF link from them: an arXiv-hosted PDF if any provider has one, otherwise the first PDF link it finds (OpenAlex, then Crossref, then Semantic Scholar). If none has one, or the lookup fails, `source_url` stays unset and can be set later with `pax edit`. A paper already in the library, by DOI or by arXiv id in any version, isn't added again: `add` refuses it and names its citation key.

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

Like `search`, each provider reports its `retrieved N of M` count, then the two lists are merged into one, with the same `merged: X → Y (Z duplicates, U unmatched)` line and one row per paper listing every provider's reference. Papers already in your library are marked `[in library]`, and excluded ones `[excluded]` (see [Excluding papers](#excluding-papers)). They take the same `--limit`, `--all`, `--year`, `--type` and `--lang` flags. OpenAlex applies the filters server-side; Semantic Scholar's citation lists take no filters, so year and type are applied there after retrieval, and language not at all. Semantic Scholar serves at most 9,999 entries of a list, and skips references it couldn't match to a paper.

### Search log

Inside a library, every `search` (a query or `--author`), `refs` and `cited-by` appends to `research/search-log.jsonl`: the record of every source a review searched, when, and with what search string, filters and limit — what PRISMA 2020 asks a review to report — plus every record retrieved, so duplicates can be counted across the whole review, not just one command. It's always on, created on the first search, and append-only: pax never rewrites a line, so commit it with the library. `search --doi` and `search --local` aren't logged: one resolves a known paper and the other reads your own library, neither searches a database. Outside a library a search still works, isn't logged, and says so on stderr. If the log can't be written, the results still print with a warning on stderr, and a search that otherwise succeeded exits 1.

Each line is one JSON object for one provider's part in one command, so a command writes a line per provider it asked, including a provider that failed:

```JSON
{"v":1,"command":"search","query":"actor model","provider":"openalex","date":"2026-10-22T22:49:14Z","limit":5,"filters":[{"filter":"year","value":"2015-2020","applied":"server-side"}],"total":246125,"retrieved":2,"keys":[["doi:10.1086/686221","title:empirical evidence for the devoted actor model|2016"],[]],"merge":{"before":7,"after":6,"unmatched":1}}
```

| Field | Meaning |
| --- | --- |
| `v` | Schema version, `1`. It changes only when a field changes meaning or is removed; new fields can appear without it changing. |
| `command` | `search`, `search-author` (`search --author`), `refs` or `cited-by`. |
| `query` | The query, or the author name for `search-author`. Only on `search` / `search-author`. |
| `seed` | The paper as given: a `provider:id` reference or a citation key. Only on `refs` / `cited-by`. |
| `provider` | `openalex`, `crossref`, `semanticscholar` or `arxiv`. |
| `date` | When the command ran, ISO 8601 in UTC to the second. Every line of one command has the same date. |
| `limit` | The `--limit` per provider, or `"all"` for `--all`. |
| `filters` | One object per `--year` / `--type` / `--lang` given: `filter` (`year`, `type`, `language`), `value` as given, and `applied`: `server-side` (in the provider's query, so `total` reflects it), `post-retrieval` (by pax on the retrieved records), or `not-applied` (the provider has no such data). `applied` is absent when the provider failed. Empty without filters. |
| `total` | How many records the provider reported matching; `null` when it doesn't say, or failed. |
| `retrieved` | How many records it returned, after any post-retrieval filter. Absent when the provider failed. |
| `keys` | One array per retrieved record, in order: the record's identity keys, strongest first — `doi:<doi>`, `arxiv:<id>` (no version), `title:<normalized title>\|<year>` — the same keys `search` merges by. An empty array is a record with none, which can't be matched. Absent when the provider failed. |
| `error` | Why the provider returned nothing: a failed request, or not knowing the `refs` / `cited-by` seed. Only present when it failed. |
| `merge` | The command's merge counts across every provider that answered: `before` (records), `after` (distinct papers) and `unmatched` (records with no key), as `search` prints them. |

The merge counts sit on every line of a command rather than on a separate summary line, so every line has the same shape and a reader never has to tell two kinds apart; to read a command's counts, take them from any one of its lines. Keys are kept per record rather than pooled, because whether two records are the same paper depends on all of each one's keys (a title match never joins two different DOIs).

The project is designed to be:
- local-first
- reproducible
- scriptable
- Unix-friendly
- lazy

## Excluding papers

Papers you've screened and decided against go in `research/excluded.nix`, next to `papers.nix`:

```Bash
pax exclude openalex:W2072794470 --reason "not empirical" --stage screening
pax exclude 10.1145/3474085.3475385 --reason "wrong domain" --stage full-text
```

The reference is a DOI or a `search` result. pax looks up the title, and keys the entry by the paper's DOI when it has one (so the exclusion matches it from any provider), otherwise by the `provider:id`. Keys are version-less: `arxiv:2301.01234v2` is keyed as `arxiv:2301.01234`, and every version of an excluded paper counts as excluded. `--stage` is `screening` (title/abstract) or `full-text`, the two exclusion stages a PRISMA flow diagram reports. `--title` sets the title yourself, and is required when the reference can't be resolved. Excluding the same paper again updates its reason and stage; a paper already in `papers.nix` can't be excluded until it's removed.

```Nix
{
  "10.1145/3474085.3475385" = {
    title = "...";
    reason = "wrong domain";
    stage = "full-text";
  };
}
```

`search`, `refs` and `cited-by` mark excluded papers `[excluded]`. A row is flagged when any of its records shares a DOI or arXiv id with an entry, so every arXiv version of an excluded paper is flagged, and so is the same paper from any provider. `[in library]` works the same way: a paper you added as a preprint is marked when its published version comes up carrying the same arXiv id.

The flake never reads `excluded.nix`, and a library without it simply has no exclusions.

## PRISMA counts

`pax stats` prints the numbers a PRISMA 2020 flow diagram asks for, computed from the files the library already holds — nothing new is stored:

```
Identification
  Records identified from databases           3
    arxiv                                     1
    openalex                                  2
  Records identified from citation searching  2
    openalex                                  2
  Duplicate records removed                   2
Screening
  Records screened                            3
  Records excluded                            1
  Reports not retrieved                       1
  Reports excluded                            1
    not empirical                             1
Included
  Studies included in review                  1
```

| Box | Source |
| --- | --- |
| Records identified from databases | The [search log](#search-log)'s `search` and `search-author` lines: records retrieved, summed per provider. |
| Records identified from citation searching | The log's `refs` and `cited-by` lines, per provider: PRISMA's "other methods". |
| Duplicate records removed | Every record identified, minus the distinct papers among them across the *whole* log, not per command: records are the same paper when they share a key, by the same rules `search` merges by (a title+year match never joins two different DOIs). A record with no key is a paper of its own. |
| Records screened | Those distinct papers. |
| Records excluded | `excluded.nix` entries with `stage = "screening"`. |
| Reports not retrieved | Declared papers with no `source_url` or no `hash` — what `pax check` reports as not fetched. Decided from `papers.nix` alone, without the network, so a recorded source that has since gone away isn't counted; `pax check` finds those. |
| Reports excluded | `excluded.nix` entries with `stage = "full-text"`, then a count per `reason` (exact text, sorted). |
| Studies included in review | The papers declared in `papers.nix`. |

A library without a search log or `excluded.nix` counts them as empty. A log line whose provider failed retrieved nothing: it's skipped, with a warning on stderr saying how many. A log line pax can't read stops `stats` with an error naming its line number, exit 7.

`pax stats --json` prints the same counts as one JSON object, for scripts and agents. Providers are keyed by their log tag, and an object's keys are sorted:

```JSON
{
  "identified": {
    "databases": { "arxiv": 1, "openalex": 2 },
    "citation_searching": { "openalex": 2 },
    "total": 5
  },
  "duplicates_removed": 2,
  "records_screened": 3,
  "excluded_screening": 1,
  "excluded_full_text": { "total": 1, "reasons": [{ "reason": "not empirical", "count": 1 }] },
  "not_retrieved": 1,
  "included": 1,
  "skipped_log_lines": 1
}
```

## Scripting

`pax` writes results to stdout, and errors and warnings to stderr: a provider that failed or was rate limited, a filter a provider couldn't apply, skipped entries, records kept as `[language unknown]`, a hint that more results are available. So `pax search "actor model" > results.txt` captures only the results, while the notes still reach the terminal.

Every command exits with a code a script can branch on:

| Code | Meaning |
| --- | --- |
| 0 | Success. |
| 1 | Any other failure, e.g. `nix` failing to fetch or build; also `check` or `sync` finding problems in the library, or a `search`, `refs` or `cited-by` that succeeded but couldn't write the [search log](#search-log). |
| 2 | Wrong arguments: an unknown command or flag, a malformed `provider:id` or citation key, or flags that can't be combined. |
| 3 | Partial: in `search`, `refs` or `cited-by`, at least one provider failed (network, rate limit, provider error) while others answered, so the results are incomplete. A provider that has no record of the seed or DOI isn't a failure: it answered, so when the others returned results `pax` exits 0 and notes it on stderr. |
| 4 | Not found: a reference, seed or citation key doesn't resolve, e.g. no provider has a record of the `refs` / `cited-by` seed or the `search --doi` DOI. |
| 5 | Refused: the library rejected the operation, e.g. adding or excluding a paper already declared, renaming to a citation key in use, fetching a paper with no PDF source, or `init` where a library exists. |
| 6 | Every provider failed (network, rate limit, provider error). |
| 7 | The library is missing, not initialized, or unreadable. A command that needs a library checks for `research/papers.nix` before doing anything else, so `add` fails before any network call, and so do a citation key given to `show`, `refs` or `cited-by` and `search --local`; a `provider:id` reference and a plain `search` work without one. |

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
