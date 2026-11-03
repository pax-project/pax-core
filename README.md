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

`--provider` asks only the providers named, repeated for several: `openalex`, `crossref`, `semanticscholar`, `arxiv` or `dblp`. [dblp](https://dblp.org), the computer science bibliography, isn't asked by default, since it covers computer science only; name it to search it, alone or beside the others:

```Bash
pax search "actor model" --provider dblp
pax search "actor model" --provider openalex --provider dblp
```

dblp results page and report totals like the others, merge with them by DOI, arXiv id or title and year, and are logged as their own `dblp` database in the [search log](#search-log) and [PRISMA counts](#prisma-counts). Its references are dblp keys (`dblp:conf/icse/Smith20`), which `pax add` and `pax show` take. dblp has no abstracts and no citation graph, so `refs`, `cited-by` and `related` don't ask it; a `dblp:` seed is looked up in the other providers by its DOI. pax asks dblp through its [SPARQL endpoint](https://sparql.dblp.org), since dblp.org's own search API answers automated clients with a bot check instead of results. A search returns the publications whose title has every word of the query (a word of four or more letters also matches as a prefix, so `model` matches `models`), newest first; `--author` returns those with an author or editor whose name has every word given. Should dblp answer with a web page instead of data, pax reports that as dblp's error, and the other providers still answer.

`pax add <result>` declares a search result in `research/papers.nix`, recording its open PDF link as `source_url` for `pax fetch`. If the resolved record has no PDF but has a DOI, `add` looks the DOI up in the other providers and takes a PDF link from them: an arXiv-hosted PDF if any provider has one, otherwise the first PDF link it finds (OpenAlex, then Crossref, then Semantic Scholar). If none has one, or the lookup fails, `source_url` stays unset and can be set later with `pax edit`. A paper already in the library, by DOI or by arXiv id in any version, isn't added again: `add` refuses it and names its citation key. Nor is a paper you've [excluded](#excluding-papers): `add` names the exclusion's key, stage and reason, and `pax unexclude` reverses it.

`pax add <result> --label <source>` also records the add in the [search log](#search-log), for a paper you didn't find by a logged search: one you already knew, say from a list of seed papers (`pax add crossref:10.1145/359545.359563 --label seeds`). `pax stats` then counts it as identified from that source, under [other sources](#prisma-counts), rather than as decided outside the log. The label names the source and changes nothing about what is added. Without `--label`, `add` logs nothing; a refused or failed add logs nothing either. If the log can't be written, the paper is still added, with a warning on stderr, and `add` exits 1.

### Search filters

`pax search` (a query or `--author`) can narrow results by publication year, document type, and language:

```Bash
pax search "actor model" --year 2015-2020 --type conference-paper --lang en
```

- `--year`: `2020`, an inclusive range `2015-2020`, or an open range `2015-` / `-2020`.
- `--type`: `journal-article`, `conference-paper`, `preprint`, `review`, `book`, `book-chapter`, `thesis`, or `dataset`.
- `--lang`: an ISO 639-1 code, e.g. `en`.

Each provider applies a filter in its own query where its API supports it, so its reported total already reflects it. Where it can't, but its records carry the field, pax filters after retrieval — the output says so, because the provider's total then counts records the filter dropped. Where a provider has no such data at all, the output says the filter wasn't applied there.

| Filter | OpenAlex | Crossref | Semantic Scholar | arXiv | dblp |
| --- | --- | --- | --- | --- | --- |
| year | server | server | server | server (submission date) | server |
| type | server | server (no `review` type: not applied) | server (no `preprint`/`thesis` type: not applied) | every record is a preprint; any other type matches nothing | server (books and theses share one type, and there's no `review` type: not applied for those) |
| language | server | after retrieval (records with no language are kept, marked `[language unknown]`) | not applied (no language data) | not applied (no language data) | not applied (no language data) |

Crossref records often carry no language (proceedings papers never do), so its language filter can't check them. Rather than silently dropping every conference paper, pax keeps them, marks each such row `[language unknown]`, and says under Crossref's count how many it kept that way.

### Citation graph

`pax refs` lists the papers a paper cites; `pax cited-by` lists the papers citing it. Both read real citation edges from OpenAlex and Semantic Scholar (Crossref's publisher-deposited reference lists are often incomplete, and it has no forward citations):

```Bash
pax refs openalex:W3137220996
pax cited-by arxiv:1706.03762 --limit 200
pax cited-by vaswani2017 --year 2023- --type journal-article
```

The paper can be any `provider:id` reference from `search`, the citation key or [alias](#aliases) of a paper in your library, or an excluded paper's alias. pax looks it up in each provider by that provider's own id when it has one, otherwise by DOI, otherwise by arXiv id (for a library paper without a DOI, read from an arxiv.org source URL); a provider it can't be found in says so, and the other still answers.

Like `search`, each provider reports its `retrieved N of M` count, then the two lists are merged into one, with the same `merged: X → Y (Z duplicates, U unmatched)` line and one row per paper listing every provider's reference. Papers already in your library are marked `[in library]`, and excluded ones `[excluded]` (see [Excluding papers](#excluding-papers)). They take the same `--limit`, `--all`, `--year`, `--type`, `--lang`, `--label` and `--with-abstracts` flags. OpenAlex applies the filters server-side; Semantic Scholar's citation lists take no filters, so year and type are applied there after retrieval, and language not at all. Semantic Scholar serves at most 9,999 entries of a list, and skips references it couldn't match to a paper.

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

A snowballing round starts from a set of papers, so `refs` and `cited-by` take several seeds at once, each a `provider:id` reference, a citation key or an alias:

```Bash
pax refs vaswani2017 he2016 openalex:W2194775991
pax cited-by vaswani2017 arxiv:1810.04805 --year 2020-
```

Each seed's provider counts print under a `seed <seed>:` line, with the same reasons as for one seed. Then one list is merged across every seed and provider, and each row gets a `seeds:` line naming the seeds whose lists returned it. `--limit`, `--all` and the filters apply per seed per provider, as for one seed. Seeds are looked up one after another, not all at once, because Semantic Scholar's unauthenticated rate limit is tight.

pax also spaces its Semantic Scholar requests at least a second apart across the whole run, rather than sending them as fast as it can and retrying when one is refused, so a long run doesn't use up its retries and come back with lists marked `failed`. A `refs` or `cited-by` seed takes at least two requests (the seed's counts, then its list), so 20 seeds take about 40 seconds or more; when a run will wait 10 seconds or more, pax says so on stderr before it starts. Without an API key, every unauthenticated caller shares one Semantic Scholar rate limit, so it can still refuse a request and pax waits as long as it's told (`Retry-After`) before trying again. Setting `SEMANTIC_SCHOLAR_API_KEY` (in the environment or `.env`; `Config::semantic_scholar_api_key` in the library) gives pax a limit of its own and removes most of that waiting. In the library, `SemanticScholarProvider::with_min_interval` sets another spacing, e.g. for a key granted a higher rate.

Two seeds that are the same paper (the same string, or sharing a provider id, DOI or arXiv id, e.g. a citation key and the `openalex:` id of the same work) are looked up once. The later one prints `same paper as <seed>`, and its lists count as the earlier one's. A seed that doesn't resolve (a malformed reference, or a citation key or alias not in the library) fails only itself, and the other seeds still answer. The command exits 4 only when no seed resolves, and exits 3 when some seeds fail while others return lists.

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

### Abstracts

Title-and-abstract screening reads the abstract, so `search` (a query or `--author`), `refs`, `cited-by` and `related` can print it under each result instead of a `pax show` per candidate:

```Bash
pax search "actor model" --with-abstracts
pax cited-by vaswani2017 --year 2023- --with-abstracts
```

Each row's abstract is wrapped, labelled `Abstract:`, and is the first one any of its records has, taking providers in the order OpenAlex, Crossref, Semantic Scholar, arXiv (dblp has none); a row none of them has one for says `(no abstract)`. OpenAlex, Crossref and arXiv return abstracts anyway; Semantic Scholar only sends them when asked, so `--with-abstracts` adds them to its requests, which makes its responses several times larger (mind its unauthenticated rate limit). Crossref's abstracts come as JATS XML; pax strips the markup to plain text, in `pax show` too. Without the flag, requests and output are as before.

### Search log

Inside a library, every `search` (a query or `--author`), `refs`, `cited-by` and `related` appends to `research/search-log.jsonl`: the record of every source a review searched, when, and with what search string, filters and limit — what PRISMA 2020 asks a review to report — plus every record retrieved, so duplicates can be counted across the whole review, not just one command. It's always on, created on the first search, and append-only: pax never rewrites a line, so commit it with the library. `search --doi` and `search --local` aren't logged: one resolves a known paper and the other reads your own library, neither searches a database. Outside a library a search still works, isn't logged, and says so on stderr. If the log can't be written, the results still print with a warning on stderr, and a search that otherwise succeeded exits 1.

`--label <text>` on `search`, `refs` or `cited-by` tags every line the command writes, e.g. `pax refs lamport1978 --label R2-backward`. It's free text for your own bookkeeping, typically a screening round, and never changes what's searched; `pax stats --by-label` counts records per label. Lines without one, including every line written before labels existed, are unlabelled.

`pax add <result> --label <source>` appends one `manual` line for the paper added, with `--label` as its source: `reference` is the result given, `provider` the one it was resolved from, and `keys` the one record's identity keys (`limit` 1, `retrieved` 1, `total` null). A plain `pax add` writes nothing.

Each line is one JSON object for one provider's part in one command, so a command writes a line per provider it asked, including a provider that failed:

```JSON
{"v":1,"command":"search","query":"actor model","provider":"openalex","date":"2026-10-22T22:49:14Z","limit":5,"filters":[{"filter":"year","value":"2015-2020","applied":"server-side"}],"total":246125,"retrieved":2,"keys":[["doi:10.1086/686221","title:empirical evidence for the devoted actor model|2016"],[]],"merge":{"before":7,"after":6,"unmatched":1}}
```

| Field | Meaning |
| --- | --- |
| `v` | Schema version, `1`. It changes only when a field changes meaning or is removed; new fields can appear without it changing. |
| `command` | `search`, `search-author` (`search --author`), `refs`, `cited-by`, `related` or `manual` (`add --label`). |
| `query` | The query, or the author name for `search-author`. Only on `search` / `search-author`. |
| `seed` | The paper as given: a `provider:id` reference, a citation key or an alias. Only on `refs` / `cited-by` / `related`. |
| `reference` | The `provider:id` reference added. Only on `manual`. |
| `provider` | `openalex`, `crossref`, `semanticscholar`, `arxiv` or `dblp`. |
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

## Papers you already have

A paper behind a paywall has no URL `pax fetch` can download, even when you have the PDF through a university login. Give pax the file instead:

```Bash
pax fetch doe2020 --file ~/Downloads/doe2020.pdf
```

The file is added to the Nix store, and its hash recorded in `papers.nix` as a download's would be, so the paper counts as included in [`pax stats`](#prisma-counts) and `pax open` opens it. A paper with no `source_url` keeps none: its artifact exists only where you added the file. If a hash is already recorded and the file's differs, `fetch --file` refuses it (exit 5) and changes nothing; `--force` replaces the hash.

On another machine, or after the file is garbage-collected from the store, nothing can download it again. `pax check`, `pax sync`, `pax fetch` and `pax open` then say so for that paper, `provide it again with pax fetch doe2020 --file <path>`, and exit 8 (`check` and `sync` exit 8 only when that's every problem they found). Giving the same file again restores it.

`nix build research#doe2020` builds such a paper from the store, or fails with the same instruction, with a `research/flake.nix` written by this version's `pax init`. A flake from an earlier version passes every paper to `fetchurl`, which can't evaluate one without a URL: `fetch --file` warns when your flake is one of those. pax itself doesn't need the flake for these papers, so `pax open` works either way; to make `nix build` work, replace `research/flake.nix` with the current [template](templates/flake.nix.template).

## Excluding papers

Papers you've screened and decided against go in `research/excluded.nix`, next to `papers.nix`:

```Bash
pax exclude openalex:W2072794470 --reason "not empirical" --stage screening
pax exclude 10.1145/3474085.3475385 --reason "wrong domain" --stage full-text --criterion X3 --round R1
```

The reference is a DOI or a `search` result. pax looks up the title, and keys the entry by the paper's DOI when it has one (so the exclusion matches it from any provider), otherwise by the `provider:id`. Keys are version-less: `arxiv:2301.01234v2` is keyed as `arxiv:2301.01234`, and every version of an excluded paper counts as excluded. `--stage` is `screening` (title/abstract) or `full-text`, the two exclusion stages a PRISMA flow diagram reports. `--title` sets the title yourself, and is required when the reference can't be resolved. The entry also records the publication year when the provider gives one; `--year YYYY` sets it yourself, like `--title`. `--criterion` records the protocol's exclusion criterion (e.g. `X3`) and `--round` the screening round (e.g. `R1`, the same label you'd give that round's searches). Both are optional free text that pax doesn't check against your protocol, and `--reason` is still required. `--alias P18` gives the paper an [alias](#aliases). Excluding the same paper again replaces its reason, stage, year, criterion and round, and keeps its alias unless `--alias` is given again; a paper already in `papers.nix` can't be excluded until it's removed.

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

`pax list --excluded` lists the excluded papers instead of the declared ones: each entry's alias and key, title, year, stage, criterion, round and reason, so a script can compare them with your per-round screening files without parsing the Nix file. `--year`, `--criterion X3` and `--round R1` narrow the list, and combine; criterion and round match exactly. `--author` and `--tag` don't apply, since an exclusion records neither, and `--criterion` / `--round` need `--excluded`; either mix is an argument error (exit 2).

```
$ pax list --excluded --round R1
P18  10.1145/3474085.3475385
	...
	2021
	full-text, criterion X3, round R1: wrong domain
```

`search`, `refs`, `cited-by` and `related` mark excluded papers `[excluded]`. A row is flagged when any of its records shares a DOI or arXiv id with an entry, so every arXiv version of an excluded paper is flagged, and so is the same paper from any provider. `[in library]` works the same way: a paper you added as a preprint is marked when its published version comes up carrying the same arXiv id.

A paper can't be both included and excluded, so `add` refuses an excluded paper, matched by the same DOI, arXiv id (any version) or `provider:id` key that flags it `[excluded]`, before anything is written:

```
$ pax add crossref:10.1145/3474085.3475385
Error: crossref:10.1145/3474085.3475385 was excluded at full-text ("wrong domain", key 10.1145/3474085.3475385); run `pax unexclude 10.1145/3474085.3475385` first to reverse that decision
```

Reversing a screening decision is its own explicit step, so nothing reverses one by accident (`add` has no `--force`). `pax unexclude` takes the exclusion key `add` names, the entry's alias, a DOI, or a `search` result, removes that one entry from `excluded.nix` and prints what it removed; after it, `add` accepts the paper. A `search` result that doesn't match an entry by its own id is resolved, so a paper excluded under its DOI can be unexcluded from any provider's reference to it. Unexcluding something that isn't excluded exits 4.

The flake never reads `excluded.nix`, and a library without it simply has no exclusions.

## Aliases

A systematic review numbers its studies (P1…Pn) in its protocol and data-extraction sheets, including the ones it excludes. An alias puts that number on the paper itself, so you can use it instead of the citation key:

```Bash
pax edit turing1936 --alias P17     # on a declared paper
pax exclude 10.1/abc --stage screening --reason "not empirical" --alias P18   # on an excluded paper
pax show P17                        # same as: pax show turing1936
pax fetch P17 / pax open P17 / pax remove P17 / pax refs P17 / pax cited-by P17
pax edit P17 --alias ""             # clear it
```

A paper has up to three names: its `provider:id` reference from `search`, its citation key (what `\cite{}` uses, generated by `add`, changed by `edit --rename`), and its alias (yours, and only you change it). An alias is stored as `alias = "P17";` on the paper's entry in `papers.nix` or `excluded.nix`, and only when set. It's letters, digits, `_`, `.` and `-`: no `:`, which marks a `provider:id` reference, and no spaces, so it needs no quoting. `export bibtex` leaves it out.

Citation keys and aliases in both files are one namespace, so every name resolves to exactly one paper. `edit --alias` and `exclude --alias` refuse a name another paper already has, as an alias or a citation key, and name that paper (exit 5); `edit --rename` refuses one too, and `add` never generates a citation key equal to an alias. A name is looked up as a citation key first, then as an alias, case-sensitively.

`list` prints the alias before the citation key and `show` prints an `Alias:` line. On an excluded paper, `pax show P18` prints the exclusion record (key, title, stage, reason, criterion, round), `refs` / `cited-by` / `related` take it as a seed through its exclusion key (a DOI or a `provider:id`), and `pax unexclude P18` reverses the exclusion; `fetch`, `open`, `edit` and `remove` refuse it with `P18 is excluded, not in the library` (exit 5).

An alias survives `edit --rename`. `remove` and `unexclude` free it and print it, so you can give it back on the other side: after `pax unexclude P18` and `pax add`, `pax edit <key> --alias P18`; or, to exclude a declared paper at full text, `pax remove P17` then `pax exclude <ref> … --alias P17`. `exclude` replaces an earlier exclusion of the same paper, so re-running it with `--alias` is also how to add or change an exclusion's alias, and `--alias ""` clears it. Unlike `--criterion` and `--round`, an alias not given again is kept, so correcting a reason never loses the paper's ID.

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
| Records identified from other sources | The log's `manual` lines (papers added with `pax add --label`), per label rather than per provider: each label is its own "other methods" source, e.g. `seeds`. Printed only once such a paper has been added. |
| Duplicate records removed | Every record identified, minus the distinct papers among them across the *whole* log, not per command: records are the same paper when they share a key, by the same rules `search` merges by (a title+year match never joins two different DOIs). A record with no key is a paper of its own. |
| Records screened | Those distinct papers. |
| Records excluded | `excluded.nix` entries with `stage = "screening"`. |
| Records awaiting screening | Records screened that have no decision yet: neither excluded nor declared. Not a PRISMA box: it's how far screening has got, and 0 once every paper has a decision. A decision is matched to the record it was made on by identity key, by the same rules: a declared paper by its DOI, arXiv id or title+year, an exclusion by its key's DOI or arXiv id, or by its title and year when it records one. |
| Reports sought for retrieval | The papers that passed screening: those declared in `papers.nix`, plus those excluded at full text (a declared paper can't be excluded, so a full-text exclusion passed screening first). Papers still awaiting screening aren't counted, so the count is right mid-round. |
| Reports not retrieved | Declared papers with no `hash` — what `pax check` reports as not fetched. A paper added with [`fetch --file`](#papers-you-already-have) has one, so it's retrieved. Decided from `papers.nix` alone, without the network, so a recorded source that has since gone away isn't counted; `pax check` finds those. |
| Reports assessed for eligibility | Reports sought minus reports not retrieved. |
| Reports excluded | `excluded.nix` entries with `stage = "full-text"`, then a count per `reason` (exact text, sorted). |
| …by criterion | Under both excluded boxes, a count per `criterion` (exact text, sorted, `no criterion` last). Listed only once some exclusion records a criterion; `--json` always has it. |
| Studies included in review | The papers declared in `papers.nix` that have an artifact (a `hash`), downloaded or added from a file. A declared paper without one counts only as not retrieved, never as both. |

Records from [`pax related`](#related-works) are counted in their own box, not with citation searching. PRISMA 2020 puts both under identification via other methods, and asks for each such source to be reported separately. A provider's recommendation isn't a citation edge, so counting it as citation searching would overstate what reference lists and forward citations found. In the diagram, report the box as its own "other methods" source (e.g. "related-works recommendations (OpenAlex, Semantic Scholar)"). Records found by both are still one paper when duplicates are removed, since that count runs across the whole log.

The boxes balance, as a PRISMA diagram must, when every declared or excluded paper was found by a logged search: records screened = records excluded + reports sought + records awaiting screening. A paper added or excluded without one, say from a DOI you already knew, is still counted in its box, so the arithmetic between boxes can be off by those papers; `stats` warns on stderr how many there are (`decided_outside_log` in JSON). Add such a paper with `pax add <result> --label <source>` and it counts as identified from that source instead, so the boxes balance, even for a review whose only source is a list of seed papers. An exclusion keyed by an opaque `provider:id` (a paper with no DOI or arXiv id) is matched to its record by title and year, so it needs a year: `pax exclude` records the one the provider gives. Without one it can't be matched, so it counts among those, and the record it was made on stays awaiting screening. That happens to exclusions recorded before pax stored years: re-run `pax exclude <ref> --reason … --stage … --year YYYY`, which replaces the entry, to fix one.

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

`pax stats --json` (see [JSON output](#json-output)) prints the same counts as one JSON object, the per-label counts always included as `by_label` (`label` is `null` for the unlabelled lines). Providers are keyed by their log tag and sorted, as are the reasons:

```JSON
{
  "identified": {
    "databases": { "arxiv": 1, "openalex": 2 },
    "citation_searching": { "openalex": 2 },
    "related_works": {},
    "other_sources": {},
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
| 2 | Wrong arguments: an unknown command or flag, a malformed `provider:id`, citation key or alias, a `fetch --file` path that isn't a file, or flags that can't be combined. |
| 3 | Partial: in `search`, `refs`, `cited-by` or `related`, at least one provider failed (network, rate limit, provider error) while others answered, so the results are incomplete. In `refs`, `cited-by` and `related` that includes a list withheld by the provider or a provider that can't look the seed up, since its list is still missing; a list cut short by `--limit` isn't (see [Citation graph](#citation-graph)). In `search --doi`, a provider that has no record of the DOI isn't a failure: it answered, so when the others returned results `pax` exits 0 and notes it on stderr. |
| 4 | Not found: a reference, seed, citation key or alias doesn't resolve, e.g. no provider has a record of the `refs` / `cited-by` / `related` seed (with several seeds, of any of them) or the `search --doi` DOI, or `unexclude` names a paper that isn't excluded. |
| 5 | Refused: the library rejected the operation, e.g. adding or excluding a paper already declared, adding an excluded paper, renaming to a citation key in use, giving a paper an alias or citation key another paper has, giving `fetch` / `open` / `edit` / `remove` an excluded paper's alias, fetching a paper with no PDF source, a `fetch --file` whose hash differs from the recorded one without `--force`, or `init` where a library exists. |
| 6 | Every provider failed (network, rate limit, provider error). |
| 7 | The library is missing, not initialized, or unreadable. A command that needs a library checks for `research/papers.nix` before doing anything else, so `add` fails before any network call, and so do a citation key or alias given to `show`, `refs`, `cited-by` or `related` and `search --local`; a `provider:id` reference and a plain `search` work without one. |
| 8 | A file to provide again: a paper added with [`fetch --file`](#papers-you-already-have) isn't in this machine's Nix store, and nothing can download it. `fetch` and `open` exit 8 for that paper; `check` and `sync` exit 8 when every problem they found is one of these, and 1 when there's any other. |

### JSON output

`--json`, before or after the command (`pax --json list`, `pax list --json`), makes any command print its result as one JSON document on stdout, for scripts and agents that shouldn't parse the text. The exit codes are the same as without it. stderr becomes one JSON object per line: the failure that ends a command as `{"error": {"code": 4, "message": "…"}}`, `code` being the exit code (an argument error clap rejects included), and anything else, such as a provider note or a search that couldn't be logged, as `{"warning": {"message": "…"}}`, with `"provider"` when one is concerned. On a failure stdout is empty.

| Command | Document |
| --- | --- |
| `list`, `search --local` | `{"papers": [paper, …]}`, empty when nothing matches or the library is empty. |
| `list --excluded` | `{"exclusions": [exclusion, …]}`, each as `exclude` gives it; empty when nothing matches or nothing is excluded. |
| `show` | `{"declared": paper, "excluded": exclusion, "candidate": record}`, exactly one of them not `null`: a provider record also has its `pdf_url` and `abstract`. |
| `search` | `{"providers": [...], "merge": {...}, "results": [...], "log": {...}}`: each provider's `retrieved`, `total`, `complete`, the filters it applied and its `error`; the merge counts; each merged paper with its `ids`, `providers`, `in_library` / `excluded` and, with `--with-abstracts`, its `abstract`; whether the search was logged. |
| `search --doi` | `{"doi": …, "providers": [{"provider", "found", "error"}], "results": [...]}`, the records merged as in `search`. |
| `refs`, `cited-by`, `related` | As `search`, with `seeds` in place of `providers`: per seed, its providers, its `incomplete` lists with why, whether it's `complete`, the earlier seed it's the `same_as`, and its `error` when the seed itself is invalid. Each result names the `seeds` that returned it. |
| `add` | `{"citation_key": …}`, and `log` with `--label`. |
| `remove`, `edit` | The paper (as it is after the edit). |
| `exclude`, `unexclude` | The exclusion: `key`, `title`, `reason`, `stage`, `year`, `criterion`, `round`, `alias`. |
| `export bibtex` | `{"citation_keys": [...], "bibtex": "…"}`. |
| `fetch` | `{"citation_key", "status": "fetched" \| "already-fetched", "hash", …}`. |
| `sync` | `{"papers": [...]}`, one `fetch` document per paper; one that failed has a `status` such as `no-source-url`, `fetch-failed` or `file-needed` and a `reason`. |
| `check` | `{"papers": [{"citation_key", "status", "expected", "actual", "error"}], "checked", "reproducible", "not_fetched", "mismatched", "errored", "missing"}`. |
| `open` | `{"citation_key", "path"}`, once the viewer started; the viewer's own output goes to stderr. |
| `init` | `{"path": "./research"}`. |
| `stats` | The [PRISMA counts](#prisma-counts). |

A paper is `{"citation_key", "alias", "title", "authors", "year", "doi", "venue", "tags", "notes", "fetched", "source_url", "hash"}`:

```JSON
{
  "papers": [
    {
      "citation_key": "lamport1978",
      "alias": "P1",
      "title": "Time, Clocks, and the Ordering of Events in a Distributed System",
      "authors": ["Leslie Lamport"],
      "year": 1978,
      "doi": "10.1145/359545.359563",
      "venue": null,
      "tags": ["core"],
      "notes": null,
      "fetched": false,
      "source_url": null,
      "hash": null
    }
  ]
}
```

The documents are `pax_core::output` types, so another client of the library can return the same shapes; the `schemars` feature derives their JSON Schema.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
