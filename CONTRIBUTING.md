# Contributing

Thanks for your interest in cargo-tribute.

## Development

```
cargo build
cargo test --all-features                    # --all-features covers the opt-in --audit code
cargo clippy --all-targets --all-features    # must be clean
cargo fmt
```

Run the tool against a real tree while developing:

```
cargo run -- --manifest-path /path/to/some/Cargo.toml
```

## Allowing a license

License and exception texts come from the `spdx` crate, so there are no text files to add. To allow a license out of the box, add its SPDX id to `DEFAULT_ACCEPTED` (`src/config.rs`); the text resolves automatically. Per project, users can add it to `accepted` in `tribute.toml` instead.

## Code layout

- `src/main.rs` -- CLI args and the `run()` orchestration
- `src/config.rs` -- tribute.toml: accepted list, clarify/exception entries, output paths
- `src/policy.rs` -- SPDX expression evaluation against the accepted set
- `src/harvest.rs` -- copyright lines and NOTICE bodies from the crate sources
- `src/output.rs` -- rendering (THIRD-PARTY.md, json, text, cyclonedx) and the `--check` staleness scan
- `src/audit.rs` -- `--audit`: declared licenses vs the license files crates ship

Unit tests live in each module's `tests` submodule; the end-to-end tests in `tests/cli.rs`.

## License resolution

`choose`/`combine` (`src/policy.rs`) evaluate the SPDX expression in postfix over an accepted-license set. When you touch them, add a case to that module's tests.

## Pull requests

- Keep `cargo fmt` and `cargo clippy --all-targets --all-features` clean.
- One focused change per pull request.

## License

By contributing, you agree that your contributions are dual licensed under [MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE), matching the project.
