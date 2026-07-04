# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Dates are KST (UTC+9).

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

[0.2.0]: https://github.com/miniex/cargo-tribute/releases/tag/v0.2.0
[0.1.0]: https://github.com/miniex/cargo-tribute/releases/tag/v0.1.0
