# Contributing

Thanks for your interest in cargo-tribute.

## Development

```
cargo build
cargo test                     # unit tests + end-to-end tests
cargo clippy --all-targets     # must be clean
cargo fmt
```

Run the tool against a real tree while developing:

```
cargo run -- --manifest-path /path/to/some/Cargo.toml
```

## Adding a license

Canonical texts live in `assets/licenses/<SPDX-id>.txt` (use the SPDX-official text). To support a new license:

1. Add `assets/licenses/<id>.txt`.
2. Add the matching `include_str!` arm in `canonical_text` (`src/main.rs`).
3. Add the id to `DEFAULT_ACCEPTED` if it should be allowed out of the box.

## License resolution

`choose`/`combine` (`src/main.rs`) evaluate the SPDX expression in postfix over an accepted-license set. When you touch them, add a case to the `tests` module.

## Pull requests

- Keep `cargo fmt` and `cargo clippy --all-targets` clean.
- One focused change per pull request.

## License

By contributing, you agree that your contributions are dual licensed under [MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE), matching the project.
