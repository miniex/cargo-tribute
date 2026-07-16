# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Dates are KST (UTC+9).

## [Unreleased]

### Added

- `notices-file` in tribute.toml writes the `--format text` document as a committed artifact (e.g. `THIRD-PARTY-NOTICES`) alongside the usual outputs, gated by `--check` like the manifest.
- `[[extra]]` entries take an optional free-text `notes` field, reproduced in the notices document under "Additional requirements / notices" -- provenance, vendored paths, Apache 4(b) change notices.

### Changed

- `--format text` now emits the flat THIRD-PARTY-NOTICES shape big Rust products ship: part I is a self-contained entry per package (source URL, the chosen license beside the full upstream expression, copyright holders, the crate's NOTICE reproduced in place), part I (continued) covers `[[extra]]` entries, and part II holds every referenced license text once. Replaces the previous grouped-by-license layout.

## [0.8.0] - 2026-07-07

### Changed

- `-p` is now report-only: it must be paired with `--json`/`--format`/`--audit`, and is rejected on the write path and with `--check`. Those outputs cover the whole workspace, so a scoped `-p` run would otherwise overwrite the shared `THIRD-PARTY.md` with a partial one (and misread other members' `LICENSES/`/`NOTICES/` files as orphans). The unused-accepted/license-text warnings are also silenced under `-p`, since a scoped tree is partial.
- `--json` and `--format cyclonedx` now report the tree even when the license policy fails: the failures become stderr warnings, and a policy-failed crate appears in the report without resolved licenses. `--format text`, the write path, and `--check` stay gated.
- A relative `--from-deny` path resolves against the workspace root, like tribute.toml.
- Clarify/exception no-match warnings consider every package in the graph, so an entry for a crate excluded by `-p` or a skip option no longer warns.
- A tree with nothing to attribute no longer leaves an empty `LICENSES/` folder behind.

### Fixed

- A hand-added NOTICES file like `meeting-1.0.0-agenda.txt` is no longer deleted as an orphan: a `-<semver>` stem only counts as ours when its pre-release part carries a digit (`-rc.1`), not a bare word. The rare digitless pre-release notice lingers instead of risking someone's file.
- `cargo tribute init` forwards `--locked`/`--offline`/`--frozen` to `cargo metadata` instead of ignoring them.
- A `WITH` exception on a `LicenseRef-*` base license now ships its exception text too.
- Two packages sharing a name and version (a registry dep plus a git fork) merge their NOTICE bodies instead of one silently overwriting the other.
- The `--format=F`, `--manifest-path=P`, `--package=N`, and `--from-deny=P` spellings are accepted, like the forwarded cargo flags.

## [0.7.0] - 2026-07-06

### Added

- Each crate's copyright holders now appear in `THIRD-PARTY.md`: `Copyright ...` lines are harvested from the license/notice files in the crate's local sources (nothing is downloaded), falling back to the `authors` metadata, so the canonical license texts are completed by the attribution the MIT/BSD family asks to reproduce. `--json` gains per-crate `authors`, `copyrights`, and `notice` fields.
- NOTICE files shipped by dependencies -- which Apache-2.0 section 4(d) asks redistributors to pass along -- are bundled into a `NOTICES/` folder (`notices-dir` in tribute.toml) and linked from `THIRD-PARTY.md`. They are covered by `--check` and orphan-cleaned like license texts; the folder only exists while a dependency actually ships one.
- `[[exception]]` in tribute.toml allows extra licenses for one crate only (optional semver `version`, like `[[clarify]]`), without widening the global accepted set; exception licenses lose the OR preference to globally accepted ones.
- An `accepted` entry can now be a `license WITH exception` pairing (e.g. `"GPL-2.0-only WITH Classpath-exception-2.0"`), accepting exactly that combination without accepting the bare license.
- `include-dev` / `include-build` in tribute.toml attribute (and gate) dev- and build-dependencies too; both remain skipped by default.
- A warning when an explicitly configured `accepted` entry is referenced by no dependency's expression, and when an `[[exception]]` entry matches no crate, so stale policy entries are visible.
- `[[extra]]` in tribute.toml attributes third-party code the crate graph can't see (C sources vendored in a `-sys` crate, bundled assets): the entry's SPDX expression flows through the same accepted policy, and it joins `LICENSES/`, `THIRD-PARTY.md` (with optional `url` and `copyright`), and the `--json` report under `extras`.
- `LicenseRef-<id>` expressions now work end to end: the id is matched against `accepted`/`allow` entries by name, and a `[[license-text]]` entry maps it to a local text file that is copied into the licenses folder, `--check`ed, and orphan-cleaned like a canonical text.
- `--format text` prints one flat plain-text document -- the attribution list, the full license and exception texts, then the NOTICE bodies -- for an "open source licenses" screen; nothing is written.
- `--format cyclonedx` prints a CycloneDX 1.6 SBOM whose components carry the full license texts, a per-component `copyright`, a `pkg:cargo` purl (registry crates only), and the effective SPDX expression as a property. `serialNumber` and `timestamp` are omitted so the output stays deterministic. `--json` is now shorthand for `--format json`.
- `--audit` scans each dependency's bundled license files against the SPDX corpus and reports files whose best match is not covered by the declared expression (mislabeled crates.io metadata). Advisory only; near-identical corpus texts are not reported when the declared license matches about as well. It is behind the opt-in `audit` cargo feature, since it pulls in text-detection dependencies (regex); prebuilt release binaries ship with it, and `cargo install` needs `--features audit`.
- `--from-deny <path>` takes the accepted list from a cargo-deny deny.toml `[licenses].allow` and maps its `exceptions` onto `[[exception]]` entries, so the allowlist lives in one place; combining it with an explicit `accepted` in tribute.toml is an error.
- `-p`/`--package <name>` (repeatable) attributes only the named workspace members' dependency closure instead of every member's.
- `cargo tribute init` scaffolds a fully commented tribute.toml at the workspace root; it refuses to overwrite an existing one.
- `skip-private` in tribute.toml drops path/git/non-crates.io dependencies from the attribution (their crates.io deps are still walked); `skip-proc-macros` drops proc-macro crates together with their compile-time subtree.
- `-q`/`--quiet` suppresses the success summary; warnings and reports still print.

### Changed

- Exit codes now distinguish the failure: 1 license policy failed, 2 output out of date (`--check`), 3 anything else.
- A failed `--check` names the first line where THIRD-PARTY.md drifted.
- `THIRD-PARTY.md` entries now carry copyright/NOTICE information; re-run `cargo tribute` to update it.

## [0.6.0] - 2026-07-06

### Added

- License and exception texts now come from the `spdx` crate, so every SPDX license and `WITH` exception is covered with nothing to hand-maintain. When a crate is attributed under `A WITH exception`, the exception body is written too (e.g. `LICENSES/LLVM-exception.txt`).
- `--json` prints the resolved attribution -- the licenses and exceptions used, and each crate's effective expression -- as JSON, without writing any files.
- A warning when an `accepted` entry is not a known SPDX id, so a typo like `Apache2.0` that would silently reject a license is caught.

### Changed

- `LICENSES/<id>.txt` bodies are now the SPDX-official texts, which can differ from the previously hand-copied ones; re-run `cargo tribute` to update them.
- The published crate no longer ships the README preview gif.

## [0.5.1] - 2026-07-05

### Fixed

- A `tribute.toml` that exists but cannot be read (bad permissions or a non-UTF-8 encoding) now errors instead of silently falling back to the built-in defaults, so a misencoded config can no longer disable the accepted-license policy without warning.

## [0.5.0] - 2026-07-05

### Added

- Forward `--features`, `--all-features`, `--no-default-features`, and `--filter-platform` to `cargo metadata`, so feature-gated (optional) and platform-specific dependencies can be attributed. Without them, `cargo metadata` resolves only the default feature set and silently omits optional dependencies.

### Changed

- In `THIRD-PARTY.md`, the separator between a crate and its SPDX expression is now `--` instead of an em-dash, so the generated file is plain ASCII. Re-run `cargo tribute` to update it.

### Fixed

- `--check` no longer reports the output as stale after a CRLF checkout (git `autocrlf`) of the LF-generated files; line-ending style is ignored when comparing. This also stops the endless rewrite churn on Windows.
- Reject an empty or `"."` `manifest`/`licenses-dir`, which resolve to the project root and would make orphan-cleanup scan the whole tree.
- A missing package in the resolve graph now surfaces as an error instead of panicking.

## [0.4.0] - 2026-07-05

### Added

- Forward `--locked`, `--offline`, and `--frozen` to `cargo metadata`, so `--check` in CI can resolve the dependency tree deterministically and without network access.

### Changed

- A `[[clarify]]` `version` is now a semver requirement, matching Cargo, so `version = "1.0"` covers `1.0.0`; write `=1.0.0` for an exact match.
- A failed file write names the path it was writing, and `--manifest-path` with no value is rejected instead of silently ignored.

### Fixed

- Only remove `LICENSES/*.txt` files cargo-tribute wrote itself; a hand-added license text is now left in place instead of deleted.
- Reject an absolute or `..` `manifest`/`licenses-dir`, which would otherwise write and delete files outside the project.

## [0.3.1] - 2026-07-05

### Changed

- Warn when a `[[clarify]]` entry matches no dependency, so a typo in its name or version is caught instead of silently ignored.

### Fixed

- Create the manifest's parent directory before writing, so a `manifest` path pointing into a subdirectory no longer fails; it now matches how `licenses-dir` is handled.

## [0.3.0] - 2026-07-05

### Added

- `[[clarify]]` in `tribute.toml` overrides a crate's license by name (and optional exact version), so crates that declare `license-file` instead of `license`, or carry a wrong or non-SPDX `license`, no longer hard-fail. The clarified expression flows through the accepted-set policy and is shown in `THIRD-PARTY.md`.
- CI workflow running `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, and `cargo tribute --check` on pushes and pull requests.

## [0.2.0] - 2026-07-04

### Added

- Show each crate's declared SPDX expression in `THIRD-PARTY.md` when it differs from the section license, so `WITH` exceptions and dual-license choices stay visible.

### Changed

- Resolve `tribute.toml` and the output paths against the workspace root instead of the current directory, so `--manifest-path` against a crate elsewhere reads and writes beside that crate.

### Fixed

- `--check` now reports orphaned `LICENSES/<id>.txt` files that a plain run would delete, so it no longer passes while stale files remain.

## [0.1.0] - 2026-07-04

First release.

### Added

- Write a REUSE-style `LICENSES/<id>.txt` folder and a `THIRD-PARTY.md` manifest from the Cargo dependency tree.
- SPDX license resolution with an accepted-list policy gate: `A OR B` picks the preferred license, `A AND B` keeps both, legacy `/` is treated as OR.
- `--check`: fail if the output is stale or a dependency's license is not accepted (for CI).
- `tribute.toml` config: `accepted`, `manifest`, `licenses-dir`.
- Flags: `--manifest-path`, `--help`, `--version`.
- Bundled canonical texts: MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, 0BSD, Zlib, Unlicense, Unicode-3.0.

[unreleased]: https://github.com/miniex/cargo-tribute/compare/v0.8.0...HEAD
[0.8.0]: https://github.com/miniex/cargo-tribute/releases/tag/v0.8.0
[0.7.0]: https://github.com/miniex/cargo-tribute/releases/tag/v0.7.0
[0.6.0]: https://github.com/miniex/cargo-tribute/releases/tag/v0.6.0
[0.5.1]: https://github.com/miniex/cargo-tribute/releases/tag/v0.5.1
[0.5.0]: https://github.com/miniex/cargo-tribute/releases/tag/v0.5.0
[0.4.0]: https://github.com/miniex/cargo-tribute/releases/tag/v0.4.0
[0.3.1]: https://github.com/miniex/cargo-tribute/releases/tag/v0.3.1
[0.3.0]: https://github.com/miniex/cargo-tribute/releases/tag/v0.3.0
[0.2.0]: https://github.com/miniex/cargo-tribute/releases/tag/v0.2.0
[0.1.0]: https://github.com/miniex/cargo-tribute/releases/tag/v0.1.0
