# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Dates are KST (UTC+9).

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

[0.4.0]: https://github.com/miniex/cargo-tribute/releases/tag/v0.4.0
[0.3.1]: https://github.com/miniex/cargo-tribute/releases/tag/v0.3.1
[0.3.0]: https://github.com/miniex/cargo-tribute/releases/tag/v0.3.0
[0.2.0]: https://github.com/miniex/cargo-tribute/releases/tag/v0.2.0
[0.1.0]: https://github.com/miniex/cargo-tribute/releases/tag/v0.1.0
