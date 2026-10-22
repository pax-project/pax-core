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
