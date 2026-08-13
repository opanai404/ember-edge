<!--
  EMBER · WebAssembly edge runtime for the component model
  SPDX-License-Identifier: MIT
-->

# Contributing to Ember

Thanks for taking the time to contribute. Ember is a small runtime with a
sharp scope: WASI preview 2, the component model, per-tenant sandboxing, OCI
distribution, and a pluggable HTTP ingress. Contributions that fit that scope
and keep the codebase lean are very welcome.

## Code of conduct

This project follows the Contributor Covenant; by participating you agree to
abide by its terms. See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## Development setup

Prerequisites:

- Rust 1.90+ (edition 2024), installed via [rustup](https://rustup.rs).
  The repo pins `1.90` in `rust-toolchain.toml`.
- `wasm-tools` (for validating WIT and component artifacts):
  `cargo install wasm-tools`.
- `wit-bindgen` (only if you build guest components):
  `cargo install wit-bindgen-cli`.

Build and test:

```sh
make build       # cargo build
make test        # cargo test --all-features
make fmt         # cargo fmt
make lint        # cargo clippy --all-targets -- -D warnings
make wit-check   # wasm-tools component wit wit/
```

The full local loop is exactly what CI runs, so a green local run implies a
green pull request.

## Pull request flow

1. Fork the repository and create a feature branch
   (`git checkout -b feat/wasi-pollable-rw`).
2. Make your change. Keep it focused; one concern per PR.
3. Add or update tests in `tests/`. Every behavior change ships with a test
   that exercises the changed path — see `tests/` for the established style.
4. Run `make fmt-check lint test` locally until green.
5. Open the pull request against `main`. CI runs lint (fmt + clippy) and the
   test suite on Linux, macOS, and Windows, plus a `cargo check` + `cargo doc`
   job and a `cargo-deny` license audit on Linux.

## Code style

- `rustfmt` with the repo defaults (4-space indent, 100-column width). No
  `#[rustfmt::skip]` unless there is a strong reason.
- Clippy runs with `-D warnings`; treat the lint suite as part of the build.
- Prefer small modules over large files; each public item carries a
  doc comment explaining the *contract* it guarantees, not just what it does.
- Error handling goes through `ember::error::Error` (`thiserror`). Map
  foreign errors early at module boundaries.
- Use `tracing` spans for anything that spans an `await` boundary. Do not
  `println!` into the runtime path.
- Dependency additions must be justified in the PR description. We are
  deliberately light on dependencies.

## Conventional commits

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/):

- `feat(registry): add oci referrers fallback`
- `fix(sandbox): enforce memfs quota on overwrite`
- `test(http): cover catch-panic -> 500 mapping`
- `docs(architecture): document instance pooling`
- `refactor(runtime): extract tenant registry`

## Security

Please do not open GitHub issues for security problems. See
[SECURITY.md](SECURITY.md) for the private reporting process and the
supported-versions table.
