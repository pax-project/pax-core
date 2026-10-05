//! Scaffolds a new `research/` library: the `flake.nix` Nix reads to build
//! declared papers, and an initial empty `papers.nix` (see [`crate::library`]
//! for the schema PAX writes into it as papers are declared). Also owns the
//! one place `pax-core` shells out to the `nix` binary — to fetch and hash an
//! artifact, per the project's "PAX understands papers, Nix understands
//! artifacts" split (see `CLAUDE.md`).

use std::fs::{create_dir, write};
use std::io::Error;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;

use crate::error::PaxError;
use crate::process::run_with_timeout;

/// How long `prefetch_file` waits on `nix store prefetch-file` before giving
/// up. That command always re-hits the network (see pax-project/.github's status.md), so a
/// stalled connection or an unreachable substituter can otherwise hang it
/// indefinitely — with no timeout, a caller (e.g. `lazypax`) sees "nothing
/// happens" forever instead of an error it can show and recover from.
const PREFETCH_TIMEOUT: Duration = Duration::from_secs(60);

/// How long `build_package` waits on `nix build`. Normally near-instant
/// (pax-project/.github's status.md measured ~0.5s for an already-fetched artifact), but a
/// first-time flake evaluation can fetch `nixpkgs` over the network, so this
/// is deliberately more generous than `PREFETCH_TIMEOUT`.
const BUILD_TIMEOUT: Duration = Duration::from_secs(180);

/// How long the pre-build `git add research/` staging step is allowed to
/// run — near-instant under normal conditions (no network involved), but
/// still guarded rather than left unbounded, same as every other subprocess
/// call in this module.
const GIT_ADD_TIMEOUT: Duration = Duration::from_secs(10);

const FLAKE_TEMPLATE: &str = include_str!("../templates/flake.nix.template");
const PAPERS_TEMPLATE: &str = include_str!("../templates/papers.nix.template");

/// Where `research/papers.nix` lives under a library root, shared by
/// `init_library` (which writes its initial contents) and `add_candidate`
/// (which reads/writes it as papers are declared).
pub fn papers_path(root: &Path) -> PathBuf {
    root.join("research").join("papers.nix")
}

pub fn init_library(root: &Path) -> Result<(), Error> {
    let research_dir = root.join("research");
    create_dir(&research_dir)?;
    write(research_dir.join("flake.nix"), FLAKE_TEMPLATE)?;
    write(papers_path(root), PAPERS_TEMPLATE)?;
    Ok(())
}

#[derive(Deserialize)]
struct PrefetchOutput {
    hash: String,
}

/// Parses `nix store prefetch-file --json`'s stdout, extracting just the SRI
/// hash (e.g. `"sha256-..."`) — already in the format `research/flake.nix`'s
/// `pkgs.fetchurl { hash = ...; }` expects, so no reformatting is needed.
/// `storePath`, the output's other field, is intentionally discarded: it's
/// fully determined by `url`+`hash`, and Nix's own store is the cache, so
/// nothing needs to persist it in `papers.nix`.
fn parse_prefetch_output(stdout: &[u8]) -> Result<String, PaxError> {
    let parsed: PrefetchOutput =
        serde_json::from_slice(stdout).map_err(|e| PaxError::Fetch(e.to_string()))?;
    Ok(parsed.hash)
}

/// Downloads `url` into the Nix store and returns its content hash, by
/// shelling out to `nix store prefetch-file`. This is the only fetching or
/// hashing pax-core does — both are otherwise Nix's job.
///
/// `name` must be the paper's citation key, matching the `name` `mkPaper`
/// gives `pkgs.fetchurl` in `research/flake.nix` — a fixed-output
/// derivation's store path is derived from `(name, hash)`, not from `url` or
/// content alone, so a mismatched name makes `build_package`'s later `nix
/// build` compute a *different* expected path than this call just
/// populated, forcing it to redundantly re-fetch (over the network, for an
/// http(s) source) instead of reusing what's already in the store. For a
/// `file://` source this isn't just slower: `nix build` runs `fetchurl`
/// inside a sandboxed builder that can't see arbitrary filesystem paths, so
/// a re-fetch attempt fails outright — matching names is what lets a local
/// file resolve as "already in the store" and skip the builder entirely.
pub fn prefetch_file(url: &str, name: &str) -> Result<String, PaxError> {
    let output = run_with_timeout(
        Command::new("nix").args(["store", "prefetch-file", "--json", "--name", name, url]),
        PREFETCH_TIMEOUT,
        PaxError::Fetch,
    )?;
    if !output.status.success() {
        return Err(PaxError::Fetch(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    parse_prefetch_output(&output.stdout)
}

/// Stages `research/`'s own contents so `nix build`'s flake evaluation
/// doesn't fail with "not tracked by Git" on a paper's first fetch/open —
/// `nix build` refuses to evaluate any file that isn't tracked *at all* (a
/// merely *dirty* tracked file only gets a warning; an untracked one is a
/// hard error), and it's easy to forget this one-time `git add` yourself.
///
/// Runs with `research/` itself as the working directory, not `root` — Nix
/// resolves "tracked by Git" against whichever repo *actually contains*
/// `research/flake.nix`, found by walking up from there to the nearest
/// `.git`. That's normally `root`'s own repo, but `research/` is just as
/// often its own independent nested repo (common for a scratch/test library
/// kept out of a surrounding project's history — `git init` inside
/// `research/` itself, deliberately not a submodule). Adding from inside
/// `research/` lets git resolve the correct owning repo either way, instead
/// of this function having to guess which one Nix will check.
///
/// Best-effort: any failure here (no `.git` at all, `git` missing, ...) is
/// swallowed and left for `nix build` itself to report — staging is a
/// convenience this function provides, not a requirement it enforces.
fn stage_research_dir(root: &Path) {
    let research_dir = root.join("research");
    let _ = run_with_timeout(
        Command::new("git")
            .arg("-C")
            .arg(&research_dir)
            .arg("add")
            .arg("-A"),
        GIT_ADD_TIMEOUT,
        PaxError::Build,
    );
}

/// Resolves a declared paper's materialized artifact to its Nix store path,
/// by building `research/flake.nix`'s `<citation_key>` package output.
/// `--no-link` is required — without it, `nix build` drops a `./result`
/// symlink in the caller's CWD, which would otherwise leak into a user's
/// project on every `pax open`. Fast (no network) when the hash is already
/// in the local store, since the derivation's hash is fixed upfront; the
/// caller must ensure the paper actually has a hash before calling this, or
/// flake evaluation fails on the `null`.
pub fn build_package(root: &Path, citation_key: &str) -> Result<PathBuf, PaxError> {
    stage_research_dir(root);
    let research_dir = root.join("research");
    let flake_ref = format!("{}#{citation_key}", research_dir.display());
    let output = run_with_timeout(
        Command::new("nix").args(["build", "--no-link", "--print-out-paths", &flake_ref]),
        BUILD_TIMEOUT,
        PaxError::Build,
    )?;
    if !output.status.success() {
        return Err(PaxError::Build(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_prefetch_output_extracts_hash() {
        let stdout = br#"{"hash":"sha256-vfqmjYmE8NwCvqylJ7dvIH2ZtmbTHR2nKO4HKBgt9pc=","storePath":"/nix/store/ym0pjqsz6b2nxwbr4xp3zpln46b71sd0-1706.03762v7"}"#;
        assert_eq!(
            parse_prefetch_output(stdout).unwrap(),
            "sha256-vfqmjYmE8NwCvqylJ7dvIH2ZtmbTHR2nKO4HKBgt9pc="
        );
    }

    #[test]
    fn parse_prefetch_output_rejects_malformed_json() {
        assert!(parse_prefetch_output(b"not json").is_err());
    }
}
