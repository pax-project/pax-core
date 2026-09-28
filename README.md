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

```Bash
pax init
pax search "actor model"
pax add <result>
pax fetch <paper>
pax open <paper>
pax export bibtex
```

The project is designed to be:
- local-first
- reproducible
- scriptable
- Unix-friendly
- lazy

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
