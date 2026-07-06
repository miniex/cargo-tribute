# cargo-tribute

[![crates.io](https://img.shields.io/crates/v/cargo-tribute.svg)](https://crates.io/crates/cargo-tribute)
[![ci](https://github.com/miniex/cargo-tribute/actions/workflows/ci.yml/badge.svg)](https://github.com/miniex/cargo-tribute/actions/workflows/ci.yml)

Generate a REUSE-style `LICENSES/` folder and a per-crate attribution manifest from a Cargo dependency tree, instead of hand-maintaining third-party license notices.

`cargo tribute` walks the normal-dependency closure of your workspace, resolves each crate's SPDX license expression against an accepted list, and writes:

- `LICENSES/<id>.txt` -- one canonical license text per license actually used
- `NOTICES/<crate>-<version>.txt` -- NOTICE files shipped by dependencies (the ones Apache-2.0 section 4(d) asks redistributors to pass along), only when a dependency actually ships one
- `THIRD-PARTY.md` -- dependencies grouped by license with each crate's copyright holders, linking to the texts

It is a policy gate (fails if a dependency's license is not accepted) and, with `--check`, a staleness gate (fails if the committed output no longer matches the dependency tree) -- both suitable for CI.

![cargo tribute writing the attribution files, then --check catching a stale one](assets/preview.gif)

## Install

```
cargo install cargo-tribute
```

Or, for a prebuilt binary via [cargo-binstall](https://github.com/cargo-bins/cargo-binstall):

```
cargo binstall cargo-tribute
```

## Usage

```
cargo tribute                     # write LICENSES/, NOTICES/ and THIRD-PARTY.md
cargo tribute --check             # verify they are current and every license is accepted
cargo tribute --manifest-path P   # run against a specific Cargo.toml (writes at its workspace root)
cargo tribute --locked --check    # forward --locked/--offline/--frozen to cargo metadata (for CI)
cargo tribute --all-features      # forward --features/--all-features/--filter-platform too, to
                                  # attribute optional (feature-gated) or platform-specific deps
cargo tribute --json              # print the resolved attribution as JSON (no files written)
cargo tribute --help
```

## Use in CI

Fail the build when a dependency's license is not accepted, or when the committed `LICENSES/`, `NOTICES/`, and `THIRD-PARTY.md` drift from the dependency tree:

```yaml
# .github/workflows/licenses.yml
name: licenses
on: [push, pull_request]
jobs:
  tribute:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo install cargo-tribute # or: cargo binstall cargo-tribute
      - run: cargo tribute --locked --check
```

## Compared to other tools

All of these are good tools; this is where `cargo-tribute` differs (behavior as of writing -- check each project's latest docs).

|                                | cargo-tribute                               | cargo-about              | cargo-deny            | cargo-license   |
| ------------------------------ | ------------------------------------------- | ------------------------ | --------------------- | --------------- |
| Attribution output             | `THIRD-PARTY.md` + REUSE `LICENSES/` folder | one file from a template | none (license linter) | lists to stdout |
| Copyright lines + NOTICE files | yes                                         | no                       | no                    | authors only    |
| Accepted-license gate          | yes                                         | yes (config)             | yes (its focus)       | no              |
| Per-crate exceptions           | yes (`[[exception]]`)                       | per-crate accepted       | yes (`exceptions`)    | no              |
| Vendored non-crate code        | yes (`[[extra]]`)                           | no                       | no                    | no              |
| Staleness `--check` for CI     | yes                                         | no                       | n/a                   | no              |
| Setup                          | zero-config (optional `tribute.toml`)       | template + `about.toml`  | `deny.toml`           | flags only      |

Want a broad supply-chain linter (advisories, source bans, duplicate detection)? Reach for `cargo-deny`. `cargo-tribute` stays focused on generating and gating the attribution output.

## Configuration

A `tribute.toml` in the project root overrides the defaults (all fields optional):

```toml
accepted = ["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "0BSD", "Zlib", "Unlicense", "Unicode-3.0"]
include-dev = false           # also attribute dev-dependencies
include-build = false         # also attribute build-dependencies
manifest = "THIRD-PARTY.md"   # attribution manifest path
licenses-dir = "LICENSES"     # folder for the canonical license texts
notices-dir = "NOTICES"       # folder for NOTICE files shipped by dependencies

# override a crate's license -- for crates that declare `license-file` instead of
# `license`, or whose `license` field is wrong or non-SPDX. Repeatable.
[[clarify]]
name = "ring"
version = "0.17.8"            # optional semver req (like Cargo); omit to match any version
expression = "MIT AND ISC AND OpenSSL"

# allow extra licenses for one crate only, without widening the global accepted
# set. Repeatable; `version` optional, like [[clarify]].
[[exception]]
name = "unicode-ident"
allow = ["Unicode-DFS-2016"]

# attribute third-party code the crate graph can't see -- C sources vendored in
# a -sys crate, a bundled font. Same accepted policy; url/copyright optional.
[[extra]]
name = "zlib (bundled in libz-sys)"
expression = "Zlib"
url = "https://zlib.net"
copyright = "Copyright (C) 1995-2024 Jean-loup Gailly and Mark Adler"

# local text for a license outside the SPDX corpus, named as LicenseRef-<id> in
# `accepted`, a [[clarify]], or an [[extra]]; copied into the licenses folder.
[[license-text]]
id = "LicenseRef-weird"
file = "licenses-extra/weird.txt"
```

## How a license is chosen

Each crate's SPDX expression is evaluated against `accepted` (which is also the OR preference order): for `A OR B` it picks the preferred accepted license, for `A AND B` it keeps both. Legacy `/`-separated expressions (`MIT/Apache-2.0`) are accepted. A crate whose expression cannot be satisfied from the accepted set is a hard error.

An `accepted` entry can also be a pairing like `"GPL-2.0-only WITH Classpath-exception-2.0"`, which allows exactly that combination without accepting the bare license. A `[[exception]]` entry allows extra licenses for one named crate only; they lose the OR preference to globally accepted ones. When `accepted` is set explicitly, an entry that no dependency's expression references is warned about, so a stale allowlist stays visible.

Code the crate graph can't see -- C sources vendored in a `-sys` crate, a bundled font -- can be attributed with an `[[extra]]` entry: its expression flows through the same accepted policy, and its licenses join `LICENSES/`, `THIRD-PARTY.md`, and the `--json` report. A license outside the SPDX corpus is named with a `LicenseRef-<id>` expression plus a `[[license-text]]` entry pointing at a local text file, which is copied into the licenses folder (and cleaned up, and `--check`ed) like a canonical text.

Only normal (runtime) dependencies are attributed by default -- set `include-dev`/`include-build` to attribute (and gate) dev- and build-dependencies too. By default `cargo metadata` resolves the default feature set, so optional (feature-gated) dependencies are not attributed unless you enable them with `--features`/`--all-features`. Canonical license texts (and `WITH` exception texts) come from the [`spdx`](https://crates.io/crates/spdx) crate, so every SPDX license and exception is covered with no texts to hand-maintain.

A crate with no `license` field (it declares `license-file` instead), or a wrong or non-SPDX one, is a hard error until you give it an SPDX expression with a `[[clarify]]` entry; the clarified expression then flows through the same accepted-set policy.

## Copyright lines and NOTICE files

A canonical license text alone is not complete attribution: the MIT/BSD family asks for the copyright notice itself to be reproduced, and Apache-2.0 section 4(d) asks redistributors to pass NOTICE files along. So each dependency's local sources (the same files cargo builds from -- nothing is downloaded) are scanned:

- `Copyright ...` lines found in the crate's bundled license/notice files appear beside the crate in `THIRD-PARTY.md`; a crate that ships none falls back to its `authors` metadata.
- A `NOTICE` file is bundled into `NOTICES/<crate>-<version>.txt` and linked from the crate's entry. The folder only exists while a dependency actually ships one, and stale files are cleaned up (and flagged by `--check`) like license texts.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
