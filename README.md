[![Release](https://img.shields.io/github/v/tag/pax-project/pax-core?label=release)](https://github.com/pax-project/pax-core/releases/latest)

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

**Prebuilt binary (x86_64 Linux):** download `pax-v2.0.0-x86_64-linux.tar.gz` from the [v2.0.0 release](https://github.com/pax-project/pax-core/releases/tag/v2.0.0), extract it, and put `pax` on your `PATH`:

```Bash
curl -LO https://github.com/pax-project/pax-core/releases/download/v2.0.0/pax-v2.0.0-x86_64-linux.tar.gz
tar xzf pax-v2.0.0-x86_64-linux.tar.gz
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

`pax add <result>` declares a search result in `research/papers.nix`, recording its open PDF link as `source_url` for `pax fetch`. If the resolved record has no PDF but has a DOI, `add` looks the DOI up in the other providers and takes a PDF link from them: an arXiv-hosted PDF if any provider has one, otherwise the first PDF link it finds (OpenAlex, then Crossref, then Semantic Scholar). If none has one, or the lookup fails, `source_url` stays unset and can be set later with `pax edit`. A paper already in the library, by DOI or by arXiv id in any version, isn't added again: `add` refuses it and names its citation key. Nor is a paper you've [excluded](#excluding-papers): `add` names the exclusion's key, stage and reason, and `pax unexclude` reverses it.

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

Like `search`, each provider reports its `retrieved N of M` count, then the two lists are merged into one, with the same `merged: X → Y (Z duplicates, U unmatched)` line and one row per paper listing every provider's reference. Papers already in your library are marked `[in library]`, and excluded ones `[excluded]` (see [Excluding papers](#excluding-papers)). They take the same `--limit`, `--all`, `--year`, `--type`, `--lang` and `--label` flags. OpenAlex applies the filters server-side; Semantic Scholar's citation lists take no filters, so year and type are applied there after retrieval, and language not at all. Semantic Scholar serves at most 9,999 entries of a list, and skips references it couldn't match to a paper.

A list that isn't the paper's whole list says why, next to that provider's count, so a review never mistakes a partial list for a complete one (and knows to fall back to the paper's own bibliography):

```text
openalex: retrieved 50 of 212 [truncated]
semanticscholar: no list [withheld]
Error: semanticscholar: Semantic Scholar withholds this paper's references at the publisher's request
```

| Reason | When |
| --- | --- |
| `withheld` | The provider holds the list back, e.g. Semantic Scholar withholding a paper's references at the publisher's request. |
| `failed` | The request failed: network, rate limit, an unreadable response. |
| `unresolved` | The paper can't be looked up in this provider: no id, DOI or arXiv id is known for it there, or the provider has no record of it. |
| `truncated` | `--limit` stopped retrieval before the provider's total. |

Any `withheld`, `failed` or `unresolved` list makes the command exit 3 while another provider returned a list (4 when no provider knows the paper at all); `truncated` alone exits 0, since you asked for that limit. In the library, `pax_core::incomplete_lists` returns the same reasons per provider.

#### Several seeds

A snowballing round starts from a set of papers, so `refs` and `cited-by` take several seeds at once, each a `provider:id` reference or a citation key:

```Bash
pax refs vaswani2017 he2016 openalex:W2194775991
pax cited-by vaswani2017 arxiv:1810.04805 --year 2020-
```

Each seed's provider counts print under a `seed <seed>:` line, with the same reasons as for one seed. Then one list is merged across every seed and provider, and each row gets a `seeds:` line naming the seeds whose lists returned it. `--limit`, `--all` and the filters apply per seed per provider, as for one seed. Seeds are looked up one after another, not all at once, because Semantic Scholar's unauthenticated rate limit is tight.

Two seeds that are the same paper (the same string, or sharing a provider id, DOI or arXiv id, e.g. a citation key and the `openalex:` id of the same work) are looked up once. The later one prints `same paper as <seed>`, and its lists count as the earlier one's. A seed that doesn't resolve (a malformed reference, or a citation key not in the library) fails only itself, and the other seeds still answer. The command exits 4 only when no seed resolves, and exits 3 when some seeds fail while others return lists.

With one seed, the output is exactly as above. In the library, `pax_core::references_many` / `cited_by_many` (and `references_many_logged` / `cited_by_many_logged`) return a `SeedResults`: each seed's per-provider lists and `incomplete()` reasons, and `merge()`, the merged list whose rows report their `seeds()` and `providers()`.

### Related works

`pax related` lists the papers the providers themselves recommend as related to a paper. Wohlin (2014) recommends a diverse start set for snowballing; a paper's related works reach outside its own citation neighbourhood, which `refs` and `cited-by` can't:

```Bash
pax related vaswani2017
pax related openalex:W2741809807 arxiv:1810.04805 --year 2020-
```

- **OpenAlex:** the work's `related_works`, chosen algorithmically from shared concepts (usually 10 to 20). pax fetches them as works, so `--year`, `--type` and `--lang` apply server-side.
- **Semantic Scholar:** its [Recommendations API](https://api.semanticscholar.org/api-docs/recommendations), up to 500 papers, drawn from its default `recent` pool: recently published papers from every field. The other pool, `all-cs`, covers computer science only, so it isn't used. A paper with no recent related work gets an empty list. Year and type are applied after retrieval, and language not at all.

Both come from the providers' current state, so the same paper gives the same list until a provider updates it. Embedding-based similarity is a different method, and isn't part of pax-core.

`related` takes seeds, flags, and multi-seed behaviour like `refs` and `cited-by` (see [Several seeds](#several-seeds)). Seeds are resolved the same way, results are merged and flagged `[in library]` / `[excluded]`, and each run is [logged](#search-log) with command `related`. Each seed's recommendations are fetched separately, so every row can name the seeds it came from. Semantic Scholar's multi-paper endpoint returns one list for the whole set, which can't be split by seed, so pax doesn't use it.

Neither provider says how many related papers there are, so each count reads `retrieved N (total not reported)`. A related list is never shown as complete, and isn't marked `truncated`, since no total was cut short. A provider that returned no list is marked `failed` or `unresolved` as in `refs`, and exit codes follow the same rules. In `pax stats`, related-works records get their own box (see [PRISMA counts](#prisma-counts)). In the library, `pax_core::related` / `related_many` (and `related_logged` / `related_many_logged`) return the same `CitationResults` / `SeedResults` as `references`.

### Search log

Inside a library, every `search` (a query or `--author`), `refs`, `cited-by` and `related` appends to `research/search-log.jsonl`: the record of every source a review searched, when, and with what search string, filters and limit — what PRISMA 2020 asks a review to report — plus every record retrieved, so duplicates can be counted across the whole review, not just one command. It's always on, created on the first search, and append-only: pax never rewrites a line, so commit it with the library. `search --doi` and `search --local` aren't logged: one resolves a known paper and the other reads your own library, neither searches a database. Outside a library a search still works, isn't logged, and says so on stderr. If the log can't be written, the results still print with a warning on stderr, and a search that otherwise succeeded exits 1.

`--label <text>` on `search`, `refs` or `cited-by` tags every line the command writes, e.g. `pax refs lamport1978 --label R2-backward`. It's free text for your own bookkeeping, typically a screening round, and never changes what's searched; `pax stats --by-label` counts records per label. Lines without one, including every line written before labels existed, are unlabelled.

Each line is one JSON object for one provider's part in one command, so a command writes a line per provider it asked, including a provider that failed:

```JSON
{"v":1,"command":"search","query":"actor model","provider":"openalex","date":"2026-10-22T22:49:14Z","limit":5,"filters":[{"filter":"year","value":"2015-2020","applied":"server-side"}],"total":246125,"retrieved":2,"keys":[["doi:10.1086/686221","title:empirical evidence for the devoted actor model|2016"],[]],"merge":{"before":7,"after":6,"unmatched":1}}
```

| Field | Meaning |
| --- | --- |
| `v` | Schema version, `1`. It changes only when a field changes meaning or is removed; new fields can appear without it changing. |
| `command` | `search`, `search-author` (`search --author`), `refs`, `cited-by` or `related`. |
| `query` | The query, or the author name for `search-author`. Only on `search` / `search-author`. |
| `seed` | The paper as given: a `provider:id` reference or a citation key. Only on `refs` / `cited-by` / `related`. |
| `provider` | `openalex`, `crossref`, `semanticscholar` or `arxiv`. |
| `date` | When the command ran, ISO 8601 in UTC to the second. Every line of one command has the same date. |
| `run` | An opaque id shared by every line one command wrote, and by no other command's. It ties together the lines of a `refs` / `cited-by` / `related` run over several seeds. Lines written before pax recorded it don't have it. |
| `label` | The `--label` given, e.g. `R2-backward`; the same on every line of one command. Absent without one. |
| `limit` | The `--limit` per provider, or `"all"` for `--all`. |
| `filters` | One object per `--year` / `--type` / `--lang` given: `filter` (`year`, `type`, `language`), `value` as given, and `applied`: `server-side` (in the provider's query, so `total` reflects it), `post-retrieval` (by pax on the retrieved records), or `not-applied` (the provider has no such data). `applied` is absent when the provider failed. Empty without filters. |
| `total` | How many records the provider reported matching; `null` when it doesn't say (never on `related`), or failed. |
| `retrieved` | How many records it returned, after any post-retrieval filter. Absent when the provider failed. |
| `keys` | One array per retrieved record, in order: the record's identity keys, strongest first — `doi:<doi>`, `arxiv:<id>` (no version), `title:<normalized title>\|<year>` — the same keys `search` merges by. An empty array is a record with none, which can't be matched. Absent when the provider failed. |
| `error` | Why the provider returned nothing: a failed request, or not knowing the `refs` / `cited-by` / `related` seed. Only present when it failed. |
| `reason` | What kind of failure `error` is: `withheld`, `failed` or `unresolved` (see [Citation graph](#citation-graph)). Only present when it failed. |
| `merge` | The command's merge counts (with several seeds, that seed's) across every provider that answered: `before` (records), `after` (distinct papers) and `unmatched` (records with no key), as `search` prints them. |

The merge counts sit on every line of a command rather than on a separate summary line, so every line has the same shape and a reader never has to tell two kinds apart; to read a command's counts, take them from any one of its lines. Keys are kept per record rather than pooled, because whether two records are the same paper depends on all of each one's keys (a title match never joins two different DOIs).

A `refs`, `cited-by` or `related` with several seeds writes one line per seed per provider. Each line has `seed` set to its own seed and `merge` set to that seed's own counts, exactly as if each seed had been run alone, so `pax stats` gives the same numbers either way. A seed that's the same paper as an earlier one repeats that seed's lines under its own `seed`, as running it alone would. An invalid seed writes no lines. The cross-seed merge isn't logged, but the shared `run` id lets you recompute it from the lines.

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
pax exclude 10.1145/3474085.3475385 --reason "wrong domain" --stage full-text --criterion X3 --round R1
```

The reference is a DOI or a `search` result. pax looks up the title, and keys the entry by the paper's DOI when it has one (so the exclusion matches it from any provider), otherwise by the `provider:id`. Keys are version-less: `arxiv:2301.01234v2` is keyed as `arxiv:2301.01234`, and every version of an excluded paper counts as excluded. `--stage` is `screening` (title/abstract) or `full-text`, the two exclusion stages a PRISMA flow diagram reports. `--title` sets the title yourself, and is required when the reference can't be resolved. The entry also records the publication year when the provider gives one; `--year YYYY` sets it yourself, like `--title`. `--criterion` records the protocol's exclusion criterion (e.g. `X3`) and `--round` the screening round (e.g. `R1`, the same label you'd give that round's searches). Both are optional free text that pax doesn't check against your protocol, and `--reason` is still required. Excluding the same paper again replaces its reason, stage, year, criterion and round; a paper already in `papers.nix` can't be excluded until it's removed.

```Nix
{
  "10.1145/3474085.3475385" = {
    title = "...";
    year = 2021;
    reason = "wrong domain";
    stage = "full-text";
    criterion = "X3";
    round = "R1";
  };
}
```

`search`, `refs`, `cited-by` and `related` mark excluded papers `[excluded]`. A row is flagged when any of its records shares a DOI or arXiv id with an entry, so every arXiv version of an excluded paper is flagged, and so is the same paper from any provider. `[in library]` works the same way: a paper you added as a preprint is marked when its published version comes up carrying the same arXiv id.

A paper can't be both included and excluded, so `add` refuses an excluded paper, matched by the same DOI, arXiv id (any version) or `provider:id` key that flags it `[excluded]`, before anything is written:

```
$ pax add crossref:10.1145/3474085.3475385
Error: crossref:10.1145/3474085.3475385 was excluded at full-text ("wrong domain", key 10.1145/3474085.3475385); run `pax unexclude 10.1145/3474085.3475385` first to reverse that decision
```

Reversing a screening decision is its own explicit step, so nothing reverses one by accident (`add` has no `--force`). `pax unexclude` takes the exclusion key `add` names, a DOI, or a `search` result, removes that one entry from `excluded.nix` and prints what it removed; after it, `add` accepts the paper. A `search` result that doesn't match an entry by its own id is resolved, so a paper excluded under its DOI can be unexcluded from any provider's reference to it. Unexcluding something that isn't excluded exits 4.

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
  Records awaiting screening                  1
  Reports sought for retrieval                2
  Reports not retrieved                       1
  Reports assessed for eligibility            1
  Reports excluded                            1
    not empirical                             1
Included
  Studies included in review                  0
```

| Box | Source |
| --- | --- |
| Records identified from databases | The [search log](#search-log)'s `search` and `search-author` lines: records retrieved, summed per provider. |
| Records identified from citation searching | The log's `refs` and `cited-by` lines, per provider: PRISMA's "other methods". |
| Records identified from related works | The log's `related` lines, per provider: also "other methods", but a source of its own (see below). Printed only once `related` has been run. |
| Duplicate records removed | Every record identified, minus the distinct papers among them across the *whole* log, not per command: records are the same paper when they share a key, by the same rules `search` merges by (a title+year match never joins two different DOIs). A record with no key is a paper of its own. |
| Records screened | Those distinct papers. |
| Records excluded | `excluded.nix` entries with `stage = "screening"`. |
| Records awaiting screening | Records screened that have no decision yet: neither excluded nor declared. Not a PRISMA box: it's how far screening has got, and 0 once every paper has a decision. A decision is matched to the record it was made on by identity key, by the same rules: a declared paper by its DOI, arXiv id or title+year, an exclusion by its key's DOI or arXiv id, or by its title and year when it records one. |
| Reports sought for retrieval | The papers that passed screening: those declared in `papers.nix`, plus those excluded at full text (a declared paper can't be excluded, so a full-text exclusion passed screening first). Papers still awaiting screening aren't counted, so the count is right mid-round. |
| Reports not retrieved | Declared papers with no `source_url` or no `hash` — what `pax check` reports as not fetched. Decided from `papers.nix` alone, without the network, so a recorded source that has since gone away isn't counted; `pax check` finds those. |
| Reports assessed for eligibility | Reports sought minus reports not retrieved. |
| Reports excluded | `excluded.nix` entries with `stage = "full-text"`, then a count per `reason` (exact text, sorted). |
| …by criterion | Under both excluded boxes, a count per `criterion` (exact text, sorted, `no criterion` last). Listed only once some exclusion records a criterion; `--json` always has it. |
| Studies included in review | The papers declared in `papers.nix` that have a fetchable artifact. A declared paper without one counts only as not retrieved, never as both. |

Records from [`pax related`](#related-works) are counted in their own box, not with citation searching. PRISMA 2020 puts both under identification via other methods, and asks for each such source to be reported separately. A provider's recommendation isn't a citation edge, so counting it as citation searching would overstate what reference lists and forward citations found. In the diagram, report the box as its own "other methods" source (e.g. "related-works recommendations (OpenAlex, Semantic Scholar)"). Records found by both are still one paper when duplicates are removed, since that count runs across the whole log.

The boxes balance, as a PRISMA diagram must, when every declared or excluded paper was found by a logged search: records screened = records excluded + reports sought + records awaiting screening. A paper added or excluded without one, say from a DOI you already knew, is still counted in its box, so the arithmetic between boxes can be off by those papers; `stats` warns on stderr how many there are (`decided_outside_log` in JSON). An exclusion keyed by an opaque `provider:id` (a paper with no DOI or arXiv id) is matched to its record by title and year, so it needs a year: `pax exclude` records the one the provider gives. Without one it can't be matched, so it counts among those, and the record it was made on stays awaiting screening. That happens to exclusions recorded before pax stored years: re-run `pax exclude <ref> --reason … --stage … --year YYYY`, which replaces the entry, to fix one.

A library without a search log or `excluded.nix` counts them as empty. A log line whose provider failed retrieved nothing: it's skipped, with a warning on stderr saying how many. A log line pax can't read stops `stats` with an error naming its line number, exit 7.

`pax stats --by-label` adds a table of the records identified under each [search-log label](#search-log), labels in the order they first appear in the log, and the unlabelled lines as one more row:

```
By label
  Label         Identified  Distinct  New  Duplicates
  R1-search             40        35   35           5
  R2-backward           60        52   30           8
  (unlabelled)           2         2    1           0
```

| Column | Meaning |
| --- | --- |
| Identified | Records retrieved by the label's lines. |
| Distinct | Distinct papers among them, by the same identity keys as duplicate counting. |
| New | Distinct papers no earlier label identified. Summed over every label, it's the records screened. |
| Duplicates | Records identified minus distinct: duplicates within the label. |

`pax stats --json` prints the same counts as one JSON object, for scripts and agents, the per-label counts always included as `by_label` (`label` is `null` for the unlabelled lines). Providers are keyed by their log tag and sorted, as are the reasons:

```JSON
{
  "identified": {
    "databases": { "arxiv": 1, "openalex": 2 },
    "citation_searching": { "openalex": 2 },
    "related_works": {},
    "total": 5
  },
  "duplicates_removed": 2,
  "records_screened": 3,
  "excluded_screening": 1,
  "awaiting_screening": 1,
  "reports_sought": 2,
  "not_retrieved": 1,
  "reports_assessed": 1,
  "excluded_full_text": { "total": 1, "reasons": [{ "reason": "not empirical", "count": 1 }] },
  "excluded_by_criterion": {
    "screening": [{ "criterion": null, "count": 1 }],
    "full_text": [{ "criterion": null, "count": 1 }]
  },
  "included": 0,
  "skipped_log_lines": 1,
  "decided_outside_log": 1,
  "by_label": [{ "label": null, "identified": 5, "distinct": 3, "new_distinct": 3, "duplicates": 2 }]
}
```

## Scripting

`pax` writes results to stdout, and errors and warnings to stderr: a provider that failed or was rate limited, a filter a provider couldn't apply, skipped entries, records kept as `[language unknown]`, a hint that more results are available. So `pax search "actor model" > results.txt` captures only the results, while the notes still reach the terminal.

Every command exits with a code a script can branch on:

| Code | Meaning |
| --- | --- |
| 0 | Success. |
| 1 | Any other failure, e.g. `nix` failing to fetch or build; also `check` or `sync` finding problems in the library, or a `search`, `refs`, `cited-by` or `related` that succeeded but couldn't write the [search log](#search-log). |
| 2 | Wrong arguments: an unknown command or flag, a malformed `provider:id` or citation key, or flags that can't be combined. |
| 3 | Partial: in `search`, `refs`, `cited-by` or `related`, at least one provider failed (network, rate limit, provider error) while others answered, so the results are incomplete. In `refs`, `cited-by` and `related` that includes a list withheld by the provider or a provider that can't look the seed up, since its list is still missing; a list cut short by `--limit` isn't (see [Citation graph](#citation-graph)). In `search --doi`, a provider that has no record of the DOI isn't a failure: it answered, so when the others returned results `pax` exits 0 and notes it on stderr. |
| 4 | Not found: a reference, seed or citation key doesn't resolve, e.g. no provider has a record of the `refs` / `cited-by` / `related` seed (with several seeds, of any of them) or the `search --doi` DOI, or `unexclude` names a paper that isn't excluded. |
| 5 | Refused: the library rejected the operation, e.g. adding or excluding a paper already declared, adding an excluded paper, renaming to a citation key in use, fetching a paper with no PDF source, or `init` where a library exists. |
| 6 | Every provider failed (network, rate limit, provider error). |
| 7 | The library is missing, not initialized, or unreadable. A command that needs a library checks for `research/papers.nix` before doing anything else, so `add` fails before any network call, and so do a citation key given to `show`, `refs`, `cited-by` or `related` and `search --local`; a `provider:id` reference and a plain `search` work without one. |

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
