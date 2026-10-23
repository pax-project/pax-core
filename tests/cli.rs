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
fn wrong_arguments_exit_2() {
    let dir = scratch("usage");
    assert_eq!(pax(&dir, &["no-such-command"]).status.code(), Some(2));
    let output = pax(&dir, &["search"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("specify exactly one of"));
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
fn refs_appends_a_line_per_provider_to_the_search_log() {
    let dir = library_without_ids("search-log");
    let log = dir.join("research").join("search-log.jsonl");
    for (run, command) in ["refs", "cited-by"].into_iter().enumerate() {
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
            assert!(!line.contains(r#""keys""#), "{line}");
        }
    }
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
