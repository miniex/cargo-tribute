# cargo-tribute

Generate a REUSE-style `LICENSES/` folder and a per-crate attribution manifest from a Cargo dependency tree, instead of hand-maintaining third-party license notices.

`cargo tribute` walks the normal-dependency closure of your workspace, resolves each crate's SPDX license expression against an accepted list, and writes:

- `LICENSES/<id>.txt` -- one canonical license text per license actually used
- `THIRD-PARTY.md` -- dependencies grouped by license, linking to the texts

It is a policy gate (fails if a dependency's license is not accepted) and, with `--check`, a staleness gate (fails if the committed output no longer matches the dependency tree) -- both suitable for CI.

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
cargo tribute                     # write LICENSES/ and THIRD-PARTY.md
cargo tribute --check             # verify they are current and every license is accepted
cargo tribute --manifest-path P   # run against a specific Cargo.toml
cargo tribute --help
```

## Configuration

A `tribute.toml` in the project root overrides the defaults (all fields optional):

```toml
accepted = ["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "0BSD", "Zlib", "Unlicense", "Unicode-3.0"]
manifest = "THIRD-PARTY.md"   # attribution manifest path
licenses-dir = "LICENSES"     # folder for the canonical license texts
```

## How a license is chosen

Each crate's SPDX expression is evaluated against `accepted` (which is also the OR preference order): for `A OR B` it picks the preferred accepted license, for `A AND B` it keeps both. Legacy `/`-separated expressions (`MIT/Apache-2.0`) are accepted. A crate whose expression cannot be satisfied from the accepted set is a hard error.

Only normal (runtime) dependencies are attributed -- dev- and build-dependencies are skipped. Canonical license texts are bundled under `assets/licenses/`; the common permissive set ships, and using a license without a bundled text errors with the file to add.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
