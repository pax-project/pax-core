# Build matrix — single responsibility: both supported feature
# configurations actually compile, not just the default one. In
# particular, `--no-default-features` proves the library alone doesn't
# pull in the CLI's own deps (clap, dotenvy, tokio).
#
# [no-cd]: see just/fmt.just.

[no-cd]
check:
    cargo build
    cargo build --no-default-features
