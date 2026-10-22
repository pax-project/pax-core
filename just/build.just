# Build matrix — single responsibility: both supported feature
# configurations actually compile, not just the default one. In
# particular, `--no-default-features` proves the library alone doesn't
# pull in the CLI's own deps (clap, dotenvy, and tokio's runtime and
# macros — the library itself uses only tokio's timer).
#
# [no-cd]: see just/fmt.just.

[no-cd]
check:
    cargo build
    cargo build --no-default-features
