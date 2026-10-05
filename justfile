# Facade: the one calling interface quality tools and CI go through. Each
# check's own rules live in just/<check>.just (single responsibility per
# file); this file only composes them.

mod fmt 'just/fmt.just'
mod clippy 'just/clippy.just'
mod build 'just/build.just'
mod test 'just/test.just'

default:
    @just --list

# Full quality gate — the one thing CI runs.
ci: fmt::check clippy::check build::check test::run

# Auto-fix entry point: clippy's machine-applicable fixes first, then
# reformat the result.
fix: clippy::fix fmt::fix
