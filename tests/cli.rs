//! Runs the built `pax` binary in a temporary directory and checks its exit
//! code and which stream its output goes to. Only cases that need no network.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

/// A fresh, empty directory to run `pax` in.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pax-cli-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn pax(dir: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

/// A library with one declared paper, DOI 10.1145/359545.359563.
fn library(name: &str) -> PathBuf {
    let dir = scratch(name);
    assert_eq!(pax(&dir, &["init"]).status.code(), Some(0));
    fs::write(
        dir.join("research").join("papers.nix"),
        r#"{
  lamport1978 = {
    doi = "10.1145/359545.359563";
    title = "Time, Clocks, and the Ordering of Events in a Distributed System";
    authors = [ "Leslie Lamport" ];
    year = 1978;
    source_url = null;
    hash = null;
    tags = [ ];
    notes = null;
  };
}
"#,
    )
    .unwrap();
    dir
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn success_exits_0_with_results_on_stdout() {
    let dir = library("success");
    let output = pax(&dir, &["list"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).contains("lamport1978"));
    assert!(stderr(&output).is_empty());
}

#[test]
fn a_closed_stdout_exits_0_quietly() {
    let dir = library("closed-stdout");
    // The read end is closed before pax starts, so its first write fails
    // with EPIPE, as when `pax list | head` stops reading.
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .arg("list")
        .current_dir(&dir)
        .stdout(writer)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(stderr(&output).is_empty());
}

#[test]
fn wrong_arguments_exit_2() {
    let dir = scratch("usage");
    assert_eq!(pax(&dir, &["no-such-command"]).status.code(), Some(2));
    let output = pax(&dir, &["search"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("specify exactly one of"));
}

#[test]
fn with_abstracts_is_offered_on_search_refs_and_cited_by() {
    let dir = scratch("with-abstracts-help");
    for command in ["search", "refs", "cited-by"] {
        let output = pax(&dir, &[command, "--help"]);
        assert_eq!(output.status.code(), Some(0), "pax {command} --help");
        assert!(
            stdout(&output).contains("--with-abstracts"),
            "pax {command}"
        );
    }
}

#[test]
fn with_abstracts_on_a_local_or_doi_search_is_a_usage_error() {
    let dir = scratch("with-abstracts-usage");
    for mode in ["--local", "--doi"] {
        let output = pax(&dir, &["search", mode, "x", "--with-abstracts"]);
        assert_eq!(output.status.code(), Some(2), "{mode}");
        assert!(stdout(&output).is_empty());
        assert!(stderr(&output).contains("--with-abstracts only apply to"));
    }
}

#[test]
fn an_unknown_citation_key_exits_4() {
    let dir = library("not-found");
    let output = pax(&dir, &["show", "bogus2000"]);
    assert_eq!(output.status.code(), Some(4));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("bogus2000"));
    assert_eq!(pax(&dir, &["remove", "bogus2000"]).status.code(), Some(4));
}

#[test]
fn excluding_a_declared_paper_is_refused_with_5() {
    let dir = library("refused");
    let output = pax(
        &dir,
        &[
            "exclude",
            "10.1145/359545.359563",
            "--reason",
            "not empirical",
            "--stage",
            "screening",
            "--title",
            "Time, Clocks",
        ],
    );
    assert_eq!(output.status.code(), Some(5));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("lamport1978"));
}

#[test]
fn exclude_records_a_given_year_offline() {
    let dir = library("exclude-year");
    let output = pax_offline(
        &dir,
        &[
            "exclude",
            "10.1000/report",
            "--reason",
            "not empirical",
            "--stage",
            "screening",
            "--title",
            "A grey-literature report",
            "--year",
            "2019",
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let excluded = std::fs::read_to_string(dir.join("research").join("excluded.nix")).unwrap();
    assert!(excluded.contains("    year = 2019;\n"), "{excluded}");
}

#[test]
fn initializing_an_existing_library_is_refused_with_5() {
    let dir = library("init-twice");
    assert_eq!(pax(&dir, &["init"]).status.code(), Some(5));
}

#[test]
fn a_command_outside_a_library_exits_7() {
    let dir = scratch("no-library");
    for args in [
        &["list"][..],
        &["export", "bibtex"],
        &["check"],
        &[
            "exclude",
            "10.1000/x",
            "--reason",
            "r",
            "--stage",
            "screening",
            "--title",
            "T",
        ],
    ] {
        let output = pax(&dir, args);
        assert_eq!(output.status.code(), Some(7), "pax {args:?}");
        assert!(stdout(&output).is_empty(), "pax {args:?}");
        assert!(stderr(&output).starts_with("Error: "), "pax {args:?}");
    }
}

/// Runs `pax` with every proxy pointed at a closed port, so a network call
/// fails at once instead of reaching a provider.
fn pax_offline(dir: &PathBuf, args: &[&str]) -> Output {
    let dead = "http://127.0.0.1:9";
    Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(args)
        .current_dir(dir)
        .env("HTTP_PROXY", dead)
        .env("HTTPS_PROXY", dead)
        .env("ALL_PROXY", dead)
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .output()
        .unwrap()
}

#[test]
fn adding_a_declared_paper_is_refused_with_5() {
    let dir = library("add-declared");
    for reference in [
        // The DOI lamport1978 was added with, from a reference that names it.
        "crossref:10.1145/359545.359563",
        "crossref:https://doi.org/10.1145/359545.359563",
    ] {
        let output = pax_offline(&dir, &["add", reference]);
        assert_eq!(output.status.code(), Some(5), "pax add {reference}");
        assert!(stdout(&output).is_empty());
        assert!(stderr(&output).contains("\"lamport1978\""));
    }
    let papers = fs::read_to_string(dir.join("research").join("papers.nix")).unwrap();
    assert_eq!(papers.matches("doi = ").count(), 1, "nothing was added");
}

/// A library with `lamport1978` declared and one paper excluded under its
/// DOI.
fn library_with_exclusion(name: &str) -> PathBuf {
    let dir = library(name);
    fs::write(
        dir.join("research").join("excluded.nix"),
        r#"{
  "10.1/abc" = { title = "A"; reason = "not empirical"; stage = "screening"; };
}
"#,
    )
    .unwrap();
    dir
}

#[test]
fn adding_an_excluded_paper_is_refused_with_5_until_it_is_unexcluded() {
    let dir = library_with_exclusion("add-excluded");
    let papers = fs::read_to_string(dir.join("research").join("papers.nix")).unwrap();

    let output = pax_offline(&dir, &["add", "crossref:10.1/ABC"]);
    assert_eq!(output.status.code(), Some(5), "{}", stderr(&output));
    assert!(stdout(&output).is_empty());
    assert_eq!(
        stderr(&output).trim_end(),
        "Error: crossref:10.1/ABC was excluded at screening (\"not empirical\", key 10.1/abc); \
         run `pax unexclude 10.1/abc` first to reverse that decision"
    );
    assert_eq!(
        fs::read_to_string(dir.join("research").join("papers.nix")).unwrap(),
        papers
    );

    let output = pax(&dir, &["unexclude", "10.1/abc"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "Unexcluded 10.1/abc (screening, \"not empirical\"): A\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("research").join("excluded.nix")).unwrap(),
        "{\n}\n"
    );

    // No longer refused: offline, resolving the reference is what fails now.
    let output = pax_offline(&dir, &["add", "crossref:10.1/ABC"]);
    assert_eq!(output.status.code(), Some(6), "{}", stderr(&output));
}

#[test]
fn unexcluding_what_isnt_excluded_exits_4() {
    let dir = library_with_exclusion("unexclude-missing");
    for reference in ["10.1/other", "crossref:10.1/other"] {
        let output = pax_offline(&dir, &["unexclude", reference]);
        assert_eq!(output.status.code(), Some(4), "pax unexclude {reference}");
        assert!(stdout(&output).is_empty());
        assert!(stderr(&output).contains("isn't excluded"));
    }
    assert!(
        fs::read_to_string(dir.join("research").join("excluded.nix"))
            .unwrap()
            .contains("10.1/abc")
    );
}

/// The same paper added twice: the second `add` names the first one's key.
#[test]
#[ignore = "needs the network"]
fn adding_a_paper_twice_is_refused_with_5() {
    let dir = scratch("add-twice");
    assert_eq!(pax(&dir, &["init"]).status.code(), Some(0));
    let first = pax(&dir, &["add", "crossref:10.1109/CVPR.2016.90"]);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    let key = stdout(&first)
        .trim()
        .strip_prefix("Added ")
        .unwrap()
        .to_string();
    for again in [
        "crossref:10.1109/CVPR.2016.90",
        // The same paper, as OpenAlex knows it.
        "openalex:W2194775991",
    ] {
        let output = pax(&dir, &["add", again]);
        assert_eq!(output.status.code(), Some(5), "pax add {again}");
        assert!(stderr(&output).contains(&format!("{key:?}")));
    }
}

/// `add --label` declares the paper and logs it as another source, so
/// `stats` doesn't count it as decided outside the log.
#[test]
#[ignore = "needs the network"]
fn adding_with_a_label_logs_the_paper_as_another_source() {
    let dir = scratch("add-label");
    assert_eq!(pax(&dir, &["init"]).status.code(), Some(0));
    let output = pax(
        &dir,
        &["add", "crossref:10.1109/CVPR.2016.90", "--label", "seeds"],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).starts_with("Added "));
    let log = fs::read_to_string(dir.join("research").join("search-log.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 1);
    let line: serde_json::Value = serde_json::from_str(log.trim_end()).unwrap();
    assert_eq!(line["command"], "manual");
    assert_eq!(line["label"], "seeds");
    assert_eq!(line["reference"], "crossref:10.1109/CVPR.2016.90");
    let output = pax(&dir, &["stats", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["identified"]["other_sources"],
        serde_json::json!({ "seeds": 1 })
    );
    assert_eq!(json["decided_outside_log"], 0);
    assert_eq!(json["reports_sought"], 1);
}

const NO_LIBRARY: &str =
    "Error: no pax library here (research/papers.nix not found); run pax init first";

#[test]
fn a_citation_key_outside_a_library_exits_7() {
    let dir = scratch("no-library-key");
    for args in [
        &["show", "turing1936"][..],
        &["refs", "turing1936"],
        &["cited-by", "turing1936"],
        &["search", "--local", "turing"],
    ] {
        let output = pax_offline(&dir, args);
        assert_eq!(output.status.code(), Some(7), "pax {args:?}");
        assert!(stdout(&output).is_empty(), "pax {args:?}");
        assert_eq!(stderr(&output).trim_end(), NO_LIBRARY, "pax {args:?}");
    }
}

#[test]
fn adding_outside_a_library_exits_7_before_any_network_call() {
    let dir = scratch("no-library-add");
    let started = std::time::Instant::now();
    // Resolving this reference needs OpenAlex; offline, that would exit 6.
    let output = pax_offline(&dir, &["add", "openalex:W2194775991"]);
    assert_eq!(output.status.code(), Some(7), "{}", stderr(&output));
    assert_eq!(stderr(&output).trim_end(), NO_LIBRARY);
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(!dir.join("research").exists());
}

/// A library whose one paper has no DOI and no arXiv source, so `refs` can
/// look it up in neither provider — and so needs no network.
fn library_without_ids(name: &str) -> PathBuf {
    let dir = scratch(name);
    assert_eq!(pax(&dir, &["init"]).status.code(), Some(0));
    fs::write(
        dir.join("research").join("papers.nix"),
        r#"{
  hoare1978 = {
    doi = null;
    title = "Communicating Sequential Processes";
    authors = [ "C. A. R. Hoare" ];
    year = 1978;
    source_url = null;
    hash = null;
    tags = [ ];
    notes = null;
  };
}
"#,
    )
    .unwrap();
    dir
}

#[test]
fn refs_cited_by_and_related_append_a_line_per_provider_to_the_search_log() {
    let dir = library_without_ids("search-log");
    let log = dir.join("research").join("search-log.jsonl");
    for (run, command) in ["refs", "cited-by", "related"].into_iter().enumerate() {
        let output = pax_offline(&dir, &[command, "hoare1978", "--year", "1990-"]);
        assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
        let lines: Vec<String> = fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(lines.len(), 2 * (run + 1), "pax {command}");
        for (line, provider) in lines[2 * run..].iter().zip(["openalex", "semanticscholar"]) {
            assert!(
                line.starts_with(&format!(
                    r#"{{"v":1,"command":"{command}","seed":"hoare1978","provider":"{provider}","#
                )),
                "{line}"
            );
            assert!(line.contains(r#""filters":[{"filter":"year","value":"1990-"}]"#));
            assert!(
                line.contains(r#""error":"can't look the seed up"#),
                "{line}"
            );
            assert!(line.contains(r#""reason":"unresolved""#), "{line}");
            assert!(!line.contains(r#""keys""#), "{line}");
        }
    }
}

#[test]
fn refs_prints_each_provider_s_reason_next_to_its_count() {
    let dir = library_without_ids("refs-reasons");
    let output = pax_offline(&dir, &["refs", "hoare1978"]);
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "openalex: no list [unresolved]\nsemanticscholar: no list [unresolved]\n"
    );
    for provider in ["openalex", "semanticscholar"] {
        assert!(
            stderr(&output).contains(&format!("Error: {provider}: can't look the seed up")),
            "{}",
            stderr(&output)
        );
    }
}

/// [`library_without_ids`] with a second such paper, `milner1980`.
fn library_with_two_seeds(name: &str) -> PathBuf {
    let dir = library_without_ids(name);
    let papers = dir.join("research").join("papers.nix");
    let text = fs::read_to_string(&papers).unwrap();
    let second = r#"  milner1980 = {
    doi = null;
    title = "A Calculus of Communicating Systems";
    authors = [ "Robin Milner" ];
    year = 1980;
    source_url = null;
    hash = null;
    tags = [ ];
    notes = null;
  };
}
"#;
    fs::write(
        &papers,
        text.trim_end().strip_suffix('}').unwrap().to_string() + second,
    )
    .unwrap();
    dir
}

#[test]
fn refs_takes_several_seeds_and_prints_each_one_s_counts() {
    let dir = library_with_two_seeds("refs-many");
    let output = pax_offline(&dir, &["refs", "hoare1978", "milner1980", "hoare1978"]);
    // No seed resolves in any provider: not found.
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "seed hoare1978:\nopenalex: no list [unresolved]\nsemanticscholar: no list [unresolved]\n\
         seed milner1980:\nopenalex: no list [unresolved]\nsemanticscholar: no list [unresolved]\n\
         seed hoare1978: same paper as hoare1978\nopenalex: no list [unresolved]\nsemanticscholar: no list [unresolved]\n"
    );
    assert!(
        stderr(&output).contains("(seed milner1980)"),
        "{}",
        stderr(&output)
    );
    // One line per seed per provider, all sharing the command's run id.
    let log = fs::read_to_string(dir.join("research").join("search-log.jsonl")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 6);
    let run = |line: &str| {
        let start = line.find(r#""run":""#).expect(line) + 7;
        line[start..start + line[start..].find('"').unwrap()].to_string()
    };
    assert!(lines.iter().all(|line| run(line) == run(lines[0])));
}

#[test]
fn a_seed_missing_from_the_library_fails_only_itself() {
    let dir = library_with_two_seeds("refs-many-invalid");
    let output = pax_offline(&dir, &["cited-by", "nosuchkey2000", "hoare1978"]);
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    assert!(stdout(&output).contains("seed hoare1978:\nopenalex: no list [unresolved]"));
    assert!(
        stderr(&output).contains("(seed nosuchkey2000)"),
        "{}",
        stderr(&output)
    );
    let log = fs::read_to_string(dir.join("research").join("search-log.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 2, "only hoare1978 is logged");
}

#[test]
fn related_takes_several_seeds_like_refs() {
    let dir = library_with_two_seeds("related-many");
    let output = pax_offline(
        &dir,
        &["related", "hoare1978", "milner1980", "nosuchkey2000"],
    );
    // No seed resolves in any provider: not found.
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "seed hoare1978:\nopenalex: no list [unresolved]\nsemanticscholar: no list [unresolved]\n\
         seed milner1980:\nopenalex: no list [unresolved]\nsemanticscholar: no list [unresolved]\n\
         seed nosuchkey2000:\n"
    );
    assert!(
        stderr(&output).contains("(seed nosuchkey2000)"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn related_with_a_citation_key_outside_a_library_exits_7() {
    let dir = scratch("related-no-library");
    let output = pax_offline(&dir, &["related", "hoare1978"]);
    assert_eq!(output.status.code(), Some(7), "{}", stderr(&output));
}

#[test]
fn refs_cited_by_and_related_need_a_seed() {
    let dir = library_with_two_seeds("refs-no-seed");
    for command in ["refs", "cited-by", "related"] {
        assert_eq!(pax_offline(&dir, &[command]).status.code(), Some(2));
    }
}

#[test]
fn several_seeds_in_one_command_give_the_same_stats_as_one_at_a_time() {
    let together = library_with_two_seeds("stats-together");
    pax_offline(&together, &["refs", "hoare1978", "milner1980"]);
    let apart = library_with_two_seeds("stats-apart");
    pax_offline(&apart, &["refs", "hoare1978"]);
    pax_offline(&apart, &["refs", "milner1980"]);
    let stats = |dir: &PathBuf| stdout(&pax(dir, &["stats", "--json"]));
    assert_eq!(stats(&together), stats(&apart));
}

#[test]
fn an_unwritable_search_log_is_a_warning() {
    let dir = library_without_ids("search-log-unwritable");
    fs::create_dir(dir.join("research").join("search-log.jsonl")).unwrap();
    let output = pax_offline(&dir, &["refs", "hoare1978"]);
    // Every provider said it doesn't know the seed: that code stands.
    assert_eq!(output.status.code(), Some(4));
    assert!(
        stderr(&output).contains("Warning: couldn't write ./research/search-log.jsonl"),
        "{}",
        stderr(&output)
    );
}

#[test]
#[ignore = "slow: offline, the providers retry for about 40s before failing"]
fn a_search_outside_a_library_notes_it_isnt_logged() {
    let dir = scratch("search-log-no-library");
    let output = pax_offline(&dir, &["search", "actor model", "--limit", "1"]);
    assert!(
        stderr(&output).contains("isn't recorded in a search log"),
        "{}",
        stderr(&output)
    );
    assert!(!dir.join("research").exists());
}

/// A library with `lamport1978` declared, a search log of four lines (one
/// failed) and an excluded.nix: `lamport1978` matches no logged record, and
/// one logged paper has no decision.
fn library_with_history(name: &str) -> PathBuf {
    let dir = library(name);
    let line = |command: &str, provider: &str, outcome: &str| {
        format!(
            r#"{{"v":1,"command":"{command}","query":"q","provider":"{provider}","date":"2026-01-01T00:00:00Z","limit":50,"filters":[],"total":null,{outcome},"merge":{{"before":0,"after":0,"unmatched":0}}}}"#
        )
    };
    let log = [
        line(
            "search",
            "openalex",
            r#""retrieved":2,"keys":[["doi:10.1/a","title:a|2000"],["doi:10.1/b"]]"#,
        ),
        line(
            "search",
            "arxiv",
            r#""retrieved":1,"keys":[["arxiv:2001.00001","title:a|2000"]]"#,
        ),
        line("search", "semanticscholar", r#""error":"rate limited""#),
        line(
            "cited-by",
            "openalex",
            r#""retrieved":2,"keys":[["doi:10.1/b"],["doi:10.1/c"]]"#,
        ),
    ];
    fs::write(
        dir.join("research").join("search-log.jsonl"),
        log.join("\n") + "\n",
    )
    .unwrap();
    fs::write(
        dir.join("research").join("excluded.nix"),
        r#"{
  "10.1/c" = { title = "C"; reason = "off topic"; stage = "screening"; };
  "10.1/b" = { title = "B"; reason = "not empirical"; stage = "full-text"; };
}
"#,
    )
    .unwrap();
    dir
}

#[test]
fn stats_prints_the_prisma_counts() {
    let dir = library_with_history("stats");
    let output = pax(&dir, &["stats"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "\
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
"
    );
    assert_eq!(
        stderr(&output),
        "\
Warning: skipped 1 search-log line(s) whose provider failed, so retrieved no records
Warning: 1 declared or excluded paper(s) match no search-log record: counted in their box, but not among the records screened
"
    );
}

#[test]
fn stats_counts_related_works_in_their_own_box() {
    let dir = library_with_history("stats-related");
    let log = dir.join("research").join("search-log.jsonl");
    let mut text = fs::read_to_string(&log).unwrap();
    text.push_str(r#"{"v":1,"command":"related","seed":"lamport1978","provider":"semanticscholar","date":"2026-01-02T00:00:00Z","limit":50,"filters":[],"total":null,"retrieved":2,"keys":[["doi:10.1/a"],["doi:10.1/d"]],"merge":{"before":2,"after":2,"unmatched":0}}"#);
    text.push('\n');
    fs::write(&log, text).unwrap();
    let output = pax(&dir, &["stats"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains(
            "\
  Records identified from citation searching  2
    openalex                                  2
  Records identified from related works       2
    semanticscholar                           2
  Duplicate records removed                   3
"
        ),
        "{stdout}"
    );
    let output = pax(&dir, &["stats", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["identified"]["related_works"],
        serde_json::json!({ "semanticscholar": 2 })
    );
    assert_eq!(json["identified"]["total"], 7);
}

/// The line `pax add crossref:10.1145/359545.359563 --label <label>` logs
/// for `lamport1978`.
fn lamport_added_by_hand(label: &str) -> String {
    format!(
        r#"{{"v":1,"command":"manual","reference":"crossref:10.1145/359545.359563","provider":"crossref","date":"2026-01-03T00:00:00Z","run":"r1","label":"{label}","limit":1,"filters":[],"total":null,"retrieved":1,"keys":[["doi:10.1145/359545.359563","title:time clocks and the ordering of events in a distributed system|1978"]],"merge":{{"before":1,"after":1,"unmatched":0}}}}"#
    ) + "\n"
}

#[test]
fn stats_counts_a_paper_added_with_a_label_as_another_source() {
    let dir = library_with_history("stats-manual");
    let log = dir.join("research").join("search-log.jsonl");
    let mut text = fs::read_to_string(&log).unwrap();
    text.push_str(&lamport_added_by_hand("seeds"));
    fs::write(&log, text).unwrap();
    let output = pax(&dir, &["stats"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains(
            "\
  Records identified from citation searching  2
    openalex                                  2
  Records identified from other sources       1
    seeds                                     1
  Duplicate records removed                   2
Screening
  Records screened                            4
  Records excluded                            1
  Records awaiting screening                  1
  Reports sought for retrieval                2
"
        ),
        "{stdout}"
    );
    // lamport1978 now matches a logged record.
    assert!(
        !stderr(&output).contains("match no search-log record"),
        "{}",
        stderr(&output)
    );
    let output = pax(&dir, &["stats", "--json", "--by-label"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["identified"]["other_sources"],
        serde_json::json!({ "seeds": 1 })
    );
    assert_eq!(json["identified"]["total"], 6);
    assert_eq!(json["decided_outside_log"], 0);
    assert_eq!(
        json["by_label"][1],
        serde_json::json!({ "label": "seeds", "identified": 1, "distinct": 1, "new_distinct": 1, "duplicates": 0 })
    );
}

#[test]
fn a_seeds_only_review_gives_prisma_counts_that_add_up() {
    let dir = library("stats-seeds-only");
    fs::write(
        dir.join("research").join("search-log.jsonl"),
        lamport_added_by_hand("seeds"),
    )
    .unwrap();
    let output = pax(&dir, &["stats", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["identified"]["databases"], serde_json::json!({}));
    assert_eq!(json["identified"]["total"], 1);
    assert_eq!(json["decided_outside_log"], 0);
    let count = |field: &str| json[field].as_u64().unwrap();
    assert_eq!(count("records_screened"), 1);
    assert_eq!(
        count("records_screened"),
        count("excluded_screening") + count("reports_sought") + count("awaiting_screening")
    );
}

#[test]
fn a_refused_or_failed_add_with_a_label_logs_nothing() {
    let dir = library_with_exclusion("add-label-refused");
    let log = dir.join("research").join("search-log.jsonl");
    // Already declared, already excluded, and (offline) unresolvable.
    for (reference, code) in [
        ("crossref:10.1145/359545.359563", 5),
        ("crossref:10.1/ABC", 5),
        ("crossref:10.1/unknown", 6),
    ] {
        let output = pax_offline(&dir, &["add", reference, "--label", "seeds"]);
        assert_eq!(output.status.code(), Some(code), "pax add {reference}");
        assert!(stdout(&output).is_empty());
        assert!(!log.exists(), "pax add {reference} logged");
    }
}

#[test]
fn stats_json_is_one_object_for_scripts() {
    let dir = library_with_history("stats-json");
    let output = pax(&dir, &["stats", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stderr(&output).is_empty());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
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
            "excluded_full_text": {
                "total": 1,
                "reasons": [{ "reason": "not empirical", "count": 1 }]
            },
            "excluded_by_criterion": {
                "screening": [{ "criterion": null, "count": 1 }],
                "full_text": [{ "criterion": null, "count": 1 }]
            },
            "included": 0,
            "skipped_log_lines": 1,
            "decided_outside_log": 1,
            "by_label": [{
                "label": null,
                "identified": 5,
                "distinct": 3,
                "new_distinct": 3,
                "duplicates": 2
            }]
        })
    );
}

#[test]
fn exclude_records_a_criterion_and_round_and_stats_groups_by_criterion() {
    let dir = library_with_history("stats-criteria");
    let output = pax(
        &dir,
        &[
            "exclude",
            "10.1/d",
            "--reason",
            "not empirical",
            "--stage",
            "full-text",
            "--title",
            "D",
            "--criterion",
            "X3",
            "--round",
            "R1",
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "Excluded 10.1/d (full-text, criterion X3, round R1): D\n"
    );
    let excluded = fs::read_to_string(dir.join("research").join("excluded.nix")).unwrap();
    assert!(
        excluded.contains("    criterion = \"X3\";\n    round = \"R1\";\n"),
        "{excluded}"
    );

    let output = pax(&dir, &["stats"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains(
            "  Records excluded                            1\n    no criterion                              1\n"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "  Reports excluded                            2\n    not empirical                             2\n    criterion X3                              1\n    no criterion                              1\n"
        ),
        "{text}"
    );

    let output = pax(&dir, &["stats", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["excluded_by_criterion"],
        serde_json::json!({
            "screening": [{ "criterion": null, "count": 1 }],
            "full_text": [
                { "criterion": "X3", "count": 1 },
                { "criterion": null, "count": 1 }
            ]
        })
    );
}

#[test]
fn stats_on_a_fresh_library_is_zeros() {
    let dir = scratch("stats-fresh");
    assert_eq!(pax(&dir, &["init"]).status.code(), Some(0));
    let output = pax(&dir, &["stats", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["identified"]["total"], 0);
    assert_eq!(json["included"], 0);
}

#[test]
fn stats_outside_a_library_exits_7() {
    let dir = scratch("stats-no-library");
    let output = pax(&dir, &["stats"]);
    assert_eq!(output.status.code(), Some(7));
    assert!(stdout(&output).is_empty());
    assert_eq!(stderr(&output).trim_end(), NO_LIBRARY);

    let output = pax(&dir, &["stats", "--json"]);
    assert_eq!(output.status.code(), Some(7));
    assert!(stdout(&output).is_empty());
    assert_eq!(
        json_error(&output),
        serde_json::json!({ "error": {
            "code": 7,
            "message": NO_LIBRARY.strip_prefix("Error: ").unwrap()
        } })
    );
}

#[test]
fn stats_names_a_malformed_log_line() {
    let dir = library("stats-malformed");
    fs::write(
        dir.join("research").join("search-log.jsonl"),
        "\n{\"v\":1,\n",
    )
    .unwrap();
    let output = pax(&dir, &["stats"]);
    assert_eq!(output.status.code(), Some(7));
    assert!(stdout(&output).is_empty());
    assert!(
        stderr(&output).starts_with("Error: failed to parse research/search-log.jsonl, line 2:"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn stats_by_label_prints_a_row_per_label_in_log_order() {
    let dir = library("stats-by-label");
    let line = |label: &str, keys: &str| {
        let label = if label.is_empty() {
            String::new()
        } else {
            format!(r#""label":"{label}","#)
        };
        format!(
            r#"{{"v":1,{label}"command":"refs","seed":"lamport1978","provider":"openalex","date":"2026-01-01T00:00:00Z","limit":50,"filters":[],"total":null,"retrieved":0,"keys":{keys},"merge":{{"before":0,"after":0,"unmatched":0}}}}"#
        )
    };
    let log = [
        line("", r#"[["doi:10.1/a"]]"#),
        line("R1", r#"[["doi:10.1/a"],["doi:10.1/b"],["doi:10.1/b"]]"#),
        line("R2-backward", r#"[["doi:10.1/b"],["doi:10.1/c"]]"#),
    ];
    fs::write(
        dir.join("research").join("search-log.jsonl"),
        log.join("\n") + "\n",
    )
    .unwrap();

    let output = pax(&dir, &["stats", "--by-label"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.ends_with(
            "\n
By label
  Label         Identified  Distinct  New  Duplicates
  (unlabelled)           1         1    1           0
  R1                     3         2    1           1
  R2-backward            2         2    1           0
"
        ),
        "{stdout}"
    );
    // Without the flag, no table.
    assert!(!String::from_utf8_lossy(&pax(&dir, &["stats"]).stdout).contains("By label"));

    let output = pax(&dir, &["stats", "--by-label", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["by_label"],
        serde_json::json!([
            { "label": null, "identified": 1, "distinct": 1, "new_distinct": 1, "duplicates": 0 },
            { "label": "R1", "identified": 3, "distinct": 2, "new_distinct": 1, "duplicates": 1 },
            { "label": "R2-backward", "identified": 2, "distinct": 2, "new_distinct": 1, "duplicates": 0 }
        ])
    );
    assert_eq!(json["records_screened"], 3);
}

#[test]
fn label_is_accepted_by_logged_searches_only() {
    let dir = scratch("label-flag");
    // Parsed, then refused for want of a library before any network call.
    for command in ["refs", "cited-by"] {
        let output = pax(&dir, &[command, "lamport1978", "--label", "R2"]);
        assert_eq!(output.status.code(), Some(7), "pax {command}");
    }
    let output = pax(&dir, &["search", "--doi", "10.1/a", "--label", "R1"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("--label"), "{}", stderr(&output));
}

#[test]
fn an_alias_names_a_paper_in_every_command_offline() {
    let dir = library("aliases");
    let ok = |args: &[&str]| {
        let output = pax_offline(&dir, args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            stderr(&output)
        );
        stdout(&output)
    };
    ok(&["edit", "lamport1978", "--alias", "P17"]);
    assert!(ok(&["list"]).contains("P17  lamport1978\n"));
    assert!(ok(&["show", "P17"]).contains("Alias:        P17\n"));
    assert!(!ok(&["export", "bibtex"]).contains("P17"));
    ok(&["edit", "P17", "--notes", "via alias"]);

    let excluded = ok(&[
        "exclude",
        "10.1000/report",
        "--reason",
        "not empirical",
        "--stage",
        "full-text",
        "--title",
        "A grey-literature report",
        "--alias",
        "P18",
    ]);
    assert!(excluded.contains("alias P18"), "{excluded}");
    let shown = ok(&["show", "P18"]);
    assert!(shown.contains("Excluded:     10.1000/report\n"), "{shown}");
    assert!(shown.contains("Stage:        full-text\n"), "{shown}");

    // A command that needs a declared paper refuses an excluded alias.
    let output = pax_offline(&dir, &["fetch", "P18"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(stderr(&output).contains("P18 is excluded, not in the library"));
    // A taken alias is refused with 5, a malformed one with 2.
    let output = pax_offline(&dir, &["edit", "P17", "--alias", "P18"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(stderr(&output).contains("10.1000/report"));
    let output = pax_offline(&dir, &["edit", "P17", "--alias", "P 18"]);
    assert_eq!(output.status.code(), Some(2));

    let unexcluded = ok(&["unexclude", "P18"]);
    assert!(unexcluded.contains("Freed alias P18"), "{unexcluded}");
    let removed = ok(&["remove", "P17"]);
    assert!(removed.contains("Removed lamport1978\n"), "{removed}");
    assert!(removed.contains("Freed alias P17"), "{removed}");
}

#[test]
fn edit_clears_an_alias_with_an_empty_value() {
    let dir = library("alias-clear");
    assert_eq!(
        pax(&dir, &["edit", "lamport1978", "--alias", "P17"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        pax(&dir, &["edit", "P17", "--alias", ""]).status.code(),
        Some(0)
    );
    assert_eq!(pax(&dir, &["show", "P17"]).status.code(), Some(4));
    let papers = std::fs::read_to_string(dir.join("research").join("papers.nix")).unwrap();
    assert!(!papers.contains("alias"), "{papers}");
}

#[test]
fn provider_names_the_providers_a_search_asks_and_logs() {
    let dir = library("search-provider");
    let output = pax_offline(
        &dir,
        &[
            "search",
            "actor model",
            "--provider",
            "dblp",
            "--limit",
            "1",
        ],
    );
    assert!(
        stderr(&output).contains("Error: dblp: "),
        "{}",
        stderr(&output)
    );
    let log = fs::read_to_string(dir.join("research").join("search-log.jsonl")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 1, "{log}");
    assert!(
        lines[0]
            .starts_with(r#"{"v":1,"command":"search","query":"actor model","provider":"dblp","#),
        "{}",
        lines[0]
    );
}

#[test]
fn provider_is_a_usage_error_outside_query_and_author_searches() {
    let dir = scratch("search-provider-usage");
    for mode in ["--local", "--doi"] {
        let output = pax(&dir, &["search", mode, "x", "--provider", "dblp"]);
        assert_eq!(output.status.code(), Some(2), "{mode}");
        assert!(stderr(&output).contains("--provider only applies to"));
    }
    let output = pax(&dir, &["search", "x", "--provider", "nope"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("unknown provider"),
        "{}",
        stderr(&output)
    );
}

/// Records `hash` on `papers.nix`'s one paper, `lamport1978`, which has no
/// `source_url`: a local artifact, as `fetch --file` leaves it.
fn record_hash(dir: &std::path::Path, hash: &str) {
    let papers = dir.join("research").join("papers.nix");
    let text = fs::read_to_string(&papers).unwrap();
    fs::write(
        &papers,
        text.replace("hash = null;", &format!("hash = {hash:?};")),
    )
    .unwrap();
}

/// A valid SRI sha256 hash no file in a Nix store has.
const ABSENT_HASH: &str = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

#[test]
fn fetch_force_needs_file() {
    let dir = library("fetch-force");
    let output = pax_offline(&dir, &["fetch", "lamport1978", "--force"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("--file"), "{}", stderr(&output));
}

#[test]
fn fetch_file_refuses_a_path_that_isnt_a_file_with_2() {
    let dir = library("fetch-file-missing");
    for path in ["no-such.pdf", "research"] {
        let output = pax_offline(&dir, &["fetch", "lamport1978", "--file", path]);
        assert_eq!(output.status.code(), Some(2), "{path}");
        assert!(
            stderr(&output).contains("is not a file"),
            "{}",
            stderr(&output)
        );
    }
    let papers = fs::read_to_string(dir.join("research").join("papers.nix")).unwrap();
    assert!(papers.contains("hash = null;"), "{papers}");
}

#[test]
fn fetch_file_needs_a_declared_paper() {
    let dir = library("fetch-file-unknown");
    fs::write(dir.join("paper.pdf"), "%PDF").unwrap();
    let output = pax_offline(&dir, &["fetch", "nosuchkey", "--file", "paper.pdf"]);
    assert_eq!(output.status.code(), Some(4));
}

#[test]
fn stats_counts_a_local_artifact_as_included() {
    let dir = library("stats-local");
    record_hash(&dir, ABSENT_HASH);
    let output = pax(&dir, &["stats", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains(r#""not_retrieved": 0"#), "{out}");
    assert!(out.contains(r#""included": 1"#), "{out}");
}

#[test]
fn fetch_without_a_source_suggests_fetch_file() {
    let dir = library("fetch-no-source");
    let output = pax_offline(&dir, &["fetch", "lamport1978"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(
        stderr(&output).contains("pax fetch lamport1978 --file <path>"),
        "{}",
        stderr(&output)
    );
}

#[test]
#[ignore = "needs nix"]
fn fetch_file_stores_the_file_and_records_its_hash() {
    let dir = library("fetch-file");
    // Unique bytes, so the store path is this run's own.
    let content = format!(
        "%PDF-1.4 pax fetch --file test {:?}",
        std::time::SystemTime::now()
    );
    fs::write(dir.join("paper.pdf"), &content).unwrap();
    fs::write(dir.join("other.pdf"), format!("{content} (another file)")).unwrap();

    let output = pax(&dir, &["fetch", "lamport1978", "--file", "paper.pdf"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).starts_with("Fetched lamport1978 (sha256-"));
    // `pax init` wrote the current flake, which builds local artifacts.
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
    let papers = fs::read_to_string(dir.join("research").join("papers.nix")).unwrap();
    assert!(papers.contains("source_url = null;"), "{papers}");
    assert!(papers.contains("hash = \"sha256-"), "{papers}");

    let output = pax(&dir, &["stats", "--json"]);
    assert!(stdout(&output).contains(r#""included": 1"#));
    let output = pax(&dir, &["check"]);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert!(stdout(&output).contains("lamport1978: reproducible"));
    let output = pax(&dir, &["sync"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("Already fetched lamport1978"));

    // `open` finds it in the store without the flake, so a flake from
    // before `fetch --file` opens it too.
    fs::write(
        dir.join("research").join("flake.nix"),
        "{ outputs = { self, nixpkgs }: { }; }\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["open", "lamport1978"])
        .current_dir(&dir)
        .env("PAX_PDF_VIEWER", "cat")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), content);

    // With --json, the viewer's output goes to stderr, leaving stdout to
    // the document.
    pax(&dir, &["edit", "lamport1978", "--alias", "P1"]);
    let output = Command::new(env!("CARGO_BIN_EXE_pax"))
        .args(["--json", "open", "P1"])
        .current_dir(&dir)
        .env("PAX_PDF_VIEWER", "cat")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let opened = json(&output);
    assert_eq!(opened["citation_key"], "lamport1978");
    assert!(opened["path"].as_str().unwrap().starts_with("/nix/store/"));
    assert_eq!(stderr(&output), content);
    let output = pax(&dir, &["--json", "fetch", "P1"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let fetched = json(&output);
    assert_eq!(fetched["citation_key"], "lamport1978");
    assert_eq!(fetched["status"], "already-fetched");
    let output = pax(&dir, &["--json", "check"]);
    assert_eq!(json(&output)["papers"][0]["status"], "reproducible");

    // The same file again is fine, with a warning that this flake can't
    // build it.
    let output = pax(&dir, &["fetch", "lamport1978", "--file", "paper.pdf"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("research/flake.nix predates `fetch --file`"),
        "{}",
        stderr(&output)
    );

    // A different file is refused until --force.
    let papers = dir.join("research").join("papers.nix");
    let before = fs::read_to_string(&papers).unwrap();
    let output = pax(&dir, &["fetch", "lamport1978", "--file", "other.pdf"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(stderr(&output).contains("--force"), "{}", stderr(&output));
    assert_eq!(fs::read_to_string(&papers).unwrap(), before);
    let output = pax(
        &dir,
        &["fetch", "lamport1978", "--file", "other.pdf", "--force"],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_ne!(fs::read_to_string(&papers).unwrap(), before);
}

#[test]
#[ignore = "needs nix"]
fn a_local_artifact_missing_from_the_store_exits_8() {
    let dir = library("local-missing");
    record_hash(&dir, ABSENT_HASH);
    let hint = "pax fetch lamport1978 --file <path>";

    let output = pax(&dir, &["check"]);
    assert_eq!(output.status.code(), Some(8), "{}", stdout(&output));
    assert!(stdout(&output).contains("lamport1978: MISSING"));
    assert!(stdout(&output).contains(hint));
    assert!(stdout(&output).contains("1 missing"));

    for args in [
        &["sync"][..],
        &["fetch", "lamport1978"],
        &["open", "lamport1978"],
    ] {
        let output = pax(&dir, args);
        assert_eq!(output.status.code(), Some(8), "{args:?}");
        assert!(
            stderr(&output).contains(hint),
            "{args:?}: {}",
            stderr(&output)
        );
    }
}

/// stdout parsed as the one JSON document `--json` prints.
fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {}", stdout(output)))
}

/// stderr's last line parsed: with `--json`, a failure's error object.
fn json_error(output: &Output) -> serde_json::Value {
    let stderr = stderr(output);
    let line = stderr.lines().last().unwrap_or_default();
    serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {stderr}"))
}

#[test]
fn list_json_is_the_declared_papers() {
    let dir = library("list-json");
    for args in [&["--json", "list"][..], &["list", "--json"]] {
        let output = pax(&dir, args);
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        assert!(stderr(&output).is_empty(), "{args:?}");
        assert_eq!(
            json(&output),
            serde_json::json!({
                "papers": [{
                    "citation_key": "lamport1978",
                    "alias": null,
                    "title": "Time, Clocks, and the Ordering of Events in a Distributed System",
                    "authors": ["Leslie Lamport"],
                    "year": 1978,
                    "doi": "10.1145/359545.359563",
                    "venue": null,
                    "tags": [],
                    "notes": null,
                    "fetched": false,
                    "source_url": null,
                    "hash": null
                }]
            }),
            "{args:?}"
        );
    }
    let output = pax(&dir, &["list", "--json", "--year", "2000"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(json(&output), serde_json::json!({ "papers": [] }));

    let empty = scratch("list-json-empty");
    assert_eq!(pax(&empty, &["init"]).status.code(), Some(0));
    let output = pax(&empty, &["list", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(json(&output), serde_json::json!({ "papers": [] }));
}

#[test]
fn show_json_is_the_declared_or_excluded_paper() {
    let dir = library("show-json");
    fs::write(
        dir.join("research").join("excluded.nix"),
        r#"{
  "10.1/abc" = { title = "A"; reason = "not empirical"; stage = "screening"; alias = "P2"; };
}
"#,
    )
    .unwrap();
    let output = pax(&dir, &["show", "lamport1978", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stderr(&output).is_empty());
    let shown = json(&output);
    assert_eq!(shown["declared"]["citation_key"], "lamport1978");
    assert_eq!(shown["declared"]["fetched"], false);
    assert_eq!(shown["excluded"], serde_json::Value::Null);
    assert_eq!(shown["candidate"], serde_json::Value::Null);

    let output = pax(&dir, &["--json", "show", "P2"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        json(&output),
        serde_json::json!({
            "declared": null,
            "excluded": {
                "key": "10.1/abc",
                "title": "A",
                "reason": "not empirical",
                "stage": "screening",
                "year": null,
                "criterion": null,
                "round": null,
                "alias": "P2"
            },
            "candidate": null
        })
    );
}

#[test]
fn a_json_failure_is_an_error_object_on_stderr_with_the_same_exit_code() {
    let dir = library("json-errors");
    for (args, code) in [
        (&["show", "bogus2000"][..], 4),
        (&["remove", "bogus2000"], 4),
        (&["search"], 2),
        (&["no-such-command"], 2),
        (&["list", "--year", "abc"], 2),
    ] {
        let text = pax(&dir, args);
        let output = pax(&dir, &[args, &["--json"]].concat());
        assert_eq!(text.status.code(), Some(code), "{args:?}");
        assert_eq!(output.status.code(), Some(code), "{args:?} --json");
        assert!(stdout(&output).is_empty(), "{args:?}");
        let error = json_error(&output);
        assert_eq!(error["error"]["code"], code, "{args:?}");
        assert!(error["error"]["message"].is_string(), "{args:?}");
    }
    let outside = scratch("json-errors-outside");
    let output = pax(&outside, &["--json", "list"]);
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(json_error(&output)["error"]["code"], 7);
}

#[test]
fn refs_cited_by_and_related_json_report_each_seed() {
    let dir = library_with_two_seeds("citations-json");
    for command in ["refs", "cited-by", "related"] {
        let args = [command, "nosuchkey2000", "hoare1978", "--json"];
        let text = pax_offline(&dir, &args[..3]);
        let output = pax_offline(&dir, &args);
        assert_eq!(output.status.code(), text.status.code(), "pax {command}");
        assert_eq!(output.status.code(), Some(4), "pax {command}");
        let json = json(&output);
        let seeds = json["seeds"].as_array().unwrap();
        assert_eq!(seeds.len(), 2);
        assert_eq!(seeds[0]["seed"], "nosuchkey2000");
        assert!(seeds[0]["error"].is_string());
        assert_eq!(seeds[1]["seed"], "hoare1978");
        assert_eq!(seeds[1]["error"], serde_json::Value::Null);
        assert_eq!(seeds[1]["complete"], false);
        assert_eq!(
            seeds[1]["incomplete"],
            serde_json::json!([
                { "provider": "openalex", "reason": "unresolved" },
                { "provider": "semanticscholar", "reason": "unresolved" }
            ])
        );
        assert_eq!(seeds[1]["providers"][0]["provider"], "openalex");
        assert!(seeds[1]["providers"][0]["error"].is_string());
        assert_eq!(json["complete"], false);
        assert_eq!(json["results"], serde_json::json!([]));
        assert_eq!(json["log"]["logged"], true);
        // Nothing but JSON on stderr: no text error lines.
        for line in stderr(&output).lines() {
            serde_json::from_str::<serde_json::Value>(line).unwrap();
        }
    }
}

#[test]
fn search_local_json_is_the_matching_papers() {
    let dir = library("search-local-json");
    let output = pax(&dir, &["--json", "search", "--local", "clocks"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let json = json(&output);
    assert_eq!(json["papers"][0]["citation_key"], "lamport1978");
    let output = pax(&dir, &["--json", "search", "--local", "nothing like it"]);
    assert_eq!(json_papers(&output), 0);
}

fn json_papers(output: &Output) -> usize {
    json(output)["papers"].as_array().unwrap().len()
}

#[test]
#[ignore = "slow: offline, the providers retry for about 40s before failing"]
fn search_json_reports_every_provider_s_failure_with_the_same_exit_code() {
    let dir = library("search-json");
    let output = pax_offline(&dir, &["search", "actor model", "--limit", "1", "--json"]);
    assert_eq!(output.status.code(), Some(6), "{}", stderr(&output));
    let doc = json(&output);
    assert_eq!(doc["providers"].as_array().unwrap().len(), 4);
    assert!(doc["providers"][0]["error"].is_string());
    assert_eq!(doc["results"], serde_json::json!([]));
    assert_eq!(doc["log"]["logged"], true);

    let output = pax_offline(&dir, &["search", "--doi", "10.1/x", "--json"]);
    assert_eq!(output.status.code(), Some(6), "{}", stderr(&output));
    let doc = json(&output);
    assert_eq!(doc["doi"], "10.1/x");
    assert_eq!(doc["providers"][0]["found"], false);
}

#[test]
fn init_json_names_the_library() {
    let dir = scratch("init-json");
    let output = pax(&dir, &["init", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(json(&output), serde_json::json!({ "path": "./research" }));
    let output = pax(&dir, &["init", "--json"]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(json_error(&output)["error"]["code"], 5);
}

#[test]
fn edit_and_remove_json_are_the_paper() {
    let dir = library("edit-json");
    let output = pax(
        &dir,
        &[
            "--json",
            "edit",
            "lamport1978",
            "--rename",
            "lamport78",
            "--alias",
            "P1",
            "--add-tag",
            "core",
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let paper = json(&output);
    assert_eq!(paper["citation_key"], "lamport78");
    assert_eq!(paper["alias"], "P1");
    assert_eq!(paper["tags"], serde_json::json!(["core"]));

    let output = pax(&dir, &["--json", "edit", "P1"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json_error(&output)["error"]["code"], 2);

    let output = pax(&dir, &["--json", "remove", "P1"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stderr(&output).is_empty());
    let paper = json(&output);
    assert_eq!(paper["citation_key"], "lamport78");
    assert_eq!(paper["alias"], "P1");
}

#[test]
fn exclude_and_unexclude_json_are_the_exclusion() {
    let dir = library("exclude-json");
    let args = [
        "exclude",
        "10.1000/report",
        "--reason",
        "not empirical",
        "--stage",
        "full-text",
        "--title",
        "A report",
        "--year",
        "2019",
        "--criterion",
        "X3",
        "--alias",
        "P9",
        "--json",
    ];
    let output = pax_offline(&dir, &args);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let expected = serde_json::json!({
        "key": "10.1000/report",
        "title": "A report",
        "reason": "not empirical",
        "stage": "full-text",
        "year": 2019,
        "criterion": "X3",
        "round": null,
        "alias": "P9"
    });
    assert_eq!(json(&output), expected);

    let output = pax_offline(&dir, &["unexclude", "P9", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(json(&output), expected);

    let output = pax_offline(&dir, &["unexclude", "P9", "--json"]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(json_error(&output)["error"]["code"], 4);
}

#[test]
fn a_refused_add_json_is_an_error_object() {
    let dir = library("add-json");
    let output = pax_offline(&dir, &["--json", "add", "crossref:10.1145/359545.359563"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(stdout(&output).is_empty());
    let error = json_error(&output);
    assert_eq!(error["error"]["code"], 5);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("lamport1978")
    );
    let output = pax_offline(&dir, &["--json", "add", "not-a-reference"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json_error(&output)["error"]["code"], 2);
}

#[test]
fn export_json_is_the_bibtex_text() {
    let dir = library("export-json");
    let text = pax(&dir, &["export", "bibtex"]);
    let output = pax(&dir, &["export", "bibtex", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let exported = json(&output);
    assert_eq!(
        exported["citation_keys"],
        serde_json::json!(["lamport1978"])
    );
    assert_eq!(
        format!("{}\n", exported["bibtex"].as_str().unwrap()),
        stdout(&text)
    );
}

#[test]
fn check_sync_fetch_and_open_json_without_the_network() {
    let dir = library("artifacts-json");
    // No hash: not fetched, which check decides without nix.
    let output = pax(&dir, &["--json", "check"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        json(&output),
        serde_json::json!({
            "papers": [{
                "citation_key": "lamport1978",
                "status": "not-fetched",
                "expected": null,
                "actual": null,
                "error": null
            }],
            "checked": 1,
            "reproducible": 0,
            "not_fetched": 1,
            "mismatched": 0,
            "errored": 0,
            "missing": 0
        })
    );

    // No source_url: nothing to fetch.
    let text = pax_offline(&dir, &["sync"]);
    let output = pax_offline(&dir, &["--json", "sync"]);
    assert_eq!(output.status.code(), text.status.code());
    assert_eq!(output.status.code(), Some(1));
    let synced = json(&output);
    assert_eq!(synced["papers"][0]["status"], "no-source-url");
    assert!(synced["papers"][0]["reason"].is_string());

    for args in [&["fetch", "lamport1978"][..], &["open", "lamport1978"]] {
        let text = pax_offline(&dir, args);
        let output = pax_offline(&dir, &[args, &["--json"]].concat());
        assert_eq!(output.status.code(), text.status.code(), "{args:?}");
        assert_eq!(output.status.code(), Some(5), "{args:?}");
        assert!(stdout(&output).is_empty(), "{args:?}");
        let error = json_error(&output);
        assert_eq!(error["error"]["code"], 5, "{args:?}");
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains("--file"),
            "{args:?}: {error}"
        );
    }

    let empty = scratch("artifacts-json-empty");
    assert_eq!(pax(&empty, &["init"]).status.code(), Some(0));
    for command in ["check", "sync"] {
        let output = pax(&empty, &[command, "--json"]);
        assert_eq!(output.status.code(), Some(0), "{command}");
        assert_eq!(json(&output)["papers"], serde_json::json!([]), "{command}");
    }
}
