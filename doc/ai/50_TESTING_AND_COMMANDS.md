# Testing And Commands

## Required Tools

- Rust toolchain, stable for normal CI and Rust 1.88 for MSRV check. Inferred from `.github/workflows/ci.yml`.
- `cargo`, `rustfmt`, `clippy`. Inferred from CI.
- `cargo-deny`. Inferred from CI and `deny.toml`.
- `libdbus-1-dev` and `pkg-config` on Ubuntu CI. Inferred from CI install steps.

## Verified During This Documentation Pass

- `git status --short`: passed; showed pre-existing untracked `.hermes/...` and `notes/`.
- `find . -maxdepth 3 -type f | sort | sed 's#^\./##' | head -300`: passed.
- `cargo metadata --no-deps --format-version 1`: passed.
- `rg`/`sed` source and docs inspections: passed.
- `cargo test -p spall-core --no-run`: reported by a discovery subagent as accidentally run with no tracked file changes.

## Fast Check Command

- Inferred: `cargo test -p <crate>`
- Inferred: `cargo check -p <crate>`
- Verified metadata-only alternative: `cargo metadata --no-deps --format-version 1`

## Full Test Command

- Inferred from README and CI: `cargo test --workspace`

## Per-Package Commands

- Inferred: `cargo test -p spall-core`
- Inferred: `cargo test -p spall-config`
- Inferred: `cargo test -p spall-openapi`
- Inferred: `cargo test -p spall-cli`
- Inferred: `cargo test -p spall-openapi --test stream_bound`
- Inferred: `cargo test -p spall-cli chain_e2e`
- Inferred: `cargo test -p spall-cli mcp_e2e`
- Inferred: `cargo test -p spall-cli arazzo_run_e2e`

## Formatting

- Inferred from CI: `cargo fmt --all -- --check`
- Formatting source code writes files if run without `--check`; do not do that in docs-only tasks.

## Linting

- Inferred from CI: `cargo clippy --workspace -- -D warnings`

## Docs

- Inferred from CI: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`
- Inferred: mdBook source lives in `docs/src`; generated output lives in `docs/book`.

## Example And Demo Commands

- Inferred from manifest: `cargo run -p spall-openapi --example embed`
- Inferred from README:
  - `spall api add petstore https://petstore.swagger.io/v2/swagger.json`
  - `spall petstore --help`
  - `spall petstore get-pet-by-id 1`
  - `spall --spall-repeat`

## Benchmark And Performance Commands

- No benches were found in discovery.
- Inferred performance guard: `cargo test -p spall-openapi --test stream_bound`
- Inferred smoke guard: `cargo build --release` then `./e2e/smoke.sh ./target/release/spall`

## CI And Deploy Commands

- Inferred from CI:
  - `cargo test --workspace`
  - `cargo clippy --workspace -- -D warnings`
  - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`
  - `cargo fmt --all -- --check`
  - `cargo deny check`
  - `cargo check --workspace --locked` on Rust 1.88
  - `cargo tree -p spall-openapi -e normal` checked for forbidden dependencies

No deploy pipeline was identified.

## Commands Agents Should Run Before Claiming Success

- Docs-only change: `git diff --check`, targeted markdown/link searches, and `git diff --stat`.
- Source change in one crate: `cargo test -p <crate>` plus any focused e2e tests named by the touched area.
- Boundary-sensitive change: run CI-equivalent commands if feasible.
- `spall-openapi` dependency change: run the dependency purity tree check from CI.
- Streaming change: run `cargo test -p spall-openapi --test stream_bound` and relevant CLI stream/pagination tests.

## Expensive, Flaky, Or Environment-Dependent Commands

- `cargo test --workspace`: may build many dependencies.
- `cargo clippy --workspace -- -D warnings`: can be slower and may require platform libraries.
- `cargo deny check`: can use registry/advisory data and may require network/update state.
- `./e2e/smoke.sh ./target/release/spall`: requires a release build and writes temporary XDG config/cache under `/tmp`.
- Keyring/cloud secret backends: treat as environment-dependent unless tests explicitly use env/file backends.

## Troubleshooting Notes

- If CI-like builds fail for missing `dbus` or `pkg-config`, check platform dependencies first.
- If `spall-openapi` purity fails, inspect normal dependencies only; dev deps should not trip the CI check.
- If cache behavior changes, check `.ir`, `.idx`, `.meta`, `IR_VERSION`, and real-spec cache round-trip tests.
