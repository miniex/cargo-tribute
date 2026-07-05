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

## Allowing a license

License and exception texts come from the `spdx` crate, so there are no text files to add. To allow a license out of the box, add its SPDX id to `DEFAULT_ACCEPTED` (`src/main.rs`); the text resolves automatically. Per project, users can add it to `accepted` in `tribute.toml` instead.

## License resolution

`choose`/`combine` (`src/main.rs`) evaluate the SPDX expression in postfix over an accepted-license set. When you touch them, add a case to the `tests` module.

## Pull requests

- Keep `cargo fmt` and `cargo clippy --all-targets` clean.
- One focused change per pull request.

## License

By contributing, you agree that your contributions are dual licensed under [MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE), matching the project.
