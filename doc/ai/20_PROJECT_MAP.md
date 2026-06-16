# Project Map

## Workspace

Verified: root `Cargo.toml` defines a four-member workspace:

| Path | Package | Purpose | Confidence |
| --- | --- | --- | --- |
| `spall-core/` | `spall-core` | Core OpenAPI/Arazzo parsing, resolver, IR, cache, YAML, validation | High |
| `spall-config/` | `spall-config` | Config loading, registry, auth config, credential classification | High |
| `spall-openapi/` | `spall-openapi` | Transport-neutral request/response contract and streaming | High |
| `spall-cli/` | `spall-cli` | `spall` binary and concrete runtime behavior | High |

Workspace package metadata sets version `0.1.0`, edition `2021`, MSRV `1.88`, and license `MIT OR Apache-2.0`. `spall-openapi` overrides edition to `2024`.

## `spall-core`

- Important files: `src/lib.rs`, `ir.rs`, `loader.rs`, `resolver.rs`, `cache.rs`, `yaml.rs`, `validator.rs`, `value.rs`, `extensions.rs`, `src/arazzo/*`.
- Public APIs: `load_spec_from_bytes`, `resolve_spec`, `load_or_resolve`, `write_cache`, `load_cached_index`, `validate_param`, `validate_body`, `SpallValue`, Arazzo model/expression APIs.
- Architecturally important dependencies: `openapiv3`, `serde-saphyr`, `postcard`, `sha2`, `indexmap`, `regex`, `thiserror`.
- Tests: `tests/resolver_test.rs`, `tests/cache_roundtrip_test.rs`, `tests/arazzo_parse_test.rs`, plus module tests.
- Evidence: `spall-core/Cargo.toml`, `spall-core/src/lib.rs`, explorer source report.

## `spall-config`

- Important files: `src/auth.rs`, `credentials.rs`, `error.rs`, `registry.rs`, `sources.rs`.
- Public APIs: `ApiRegistry`, `ApiEntry`, `ProfileConfig`, `AuthConfig`, `ResolvedAuth`, `default_token_env`, `scan_api_configs`, `scan_spec_dirs`.
- Architecturally important dependencies: `toml`, `dirs`, `secrecy`, `serde`, `thiserror`.
- Tests: `auth_serde_test.rs`, `credentials_test.rs`, `registry_test.rs`, `sources_test.rs`.
- Evidence: `spall-config/Cargo.toml`, tests, examples in `examples/`.

## `spall-openapi`

- Important files: `src/lib.rs`, `builder.rs`, `request.rs`, `response.rs`, `stream.rs`, `paginate.rs`, `datapath.rs`, `links.rs`, `status.rs`, `auth/*`.
- Public APIs: `build_request`, `HttpRequestSpec`, `RequestBody`, `ResponseStream`, `Status`, `DataPath`, `Paginator`, `ItemStream`, `JsonSkimmer`, `StreamLimits`, auth contributor functions.
- Architecturally important dependencies: `spall-core`, `serde_json`, `indexmap`, `secrecy`, `base64`, `thiserror`.
- Tests/examples: module tests, `tests/stream_bound.rs`, `examples/embed.rs`.
- Evidence: crate docs in `src/lib.rs`, CI `openapi-purity` job.

## `spall-cli`

- Important files: `src/main.rs`, `command.rs`, `execute.rs`, `transport.rs`, `http.rs`, `fetch.rs`, `auth/*`, `commands/*`, `mcp/*`, `arazzo_runner*.rs`, `history.rs`, `repeat.rs`, `repl.rs`, `chain.rs`, `output.rs`, `validate.rs`.
- Public entry points: `spall` binary, `run_with_args`, dynamic command builder, `execute_operation_programmatic`, built-in subcommands.
- Architecturally important dependencies: workspace crates, `clap`, `tokio`, `reqwest`, `miette`, `rusqlite`, `jmespath`, `syntect`, `rustyline`, `axum`, `tower-http`, optional `hasp`.
- Tests: many e2e tests under `spall-cli/tests/`.
- Evidence: `spall-cli/Cargo.toml`, `src/main.rs`, tests.

## Documentation

- `README.md`: current user-facing overview and basic commands.
- `CLAUDE.md`: concise existing agent context.
- `docs/src/`: mdBook source.
- `docs/book/`: generated mdBook output; do not treat as the edit source.
- `docs/internal/`: plans, design, and research. Historical planning and research; treat as secondary evidence behind current source, tests, manifests, and CI.
- `doc/ai/`: durable onboarding docs created by this pass.

## Tests, Examples, And Smoke

- `spall-core/tests/`: resolver/cache/Arazzo tests.
- `spall-config/tests/`: config/auth/registry/source tests.
- `spall-openapi/tests/stream_bound.rs`: bounded-memory streaming guard.
- `spall-cli/tests/`: CLI integration/e2e coverage.
- `e2e/smoke.sh`: release-binary smoke test for cache/help/output behavior.
- `examples/config.toml`, `examples/apis/petstore.toml`: config examples.

## CI And Tooling

Verified from `.github/workflows/ci.yml`:

- `cargo test --workspace`
- `cargo clippy --workspace -- -D warnings`
- `RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps`
- `cargo fmt --all -- --check`
- `cargo deny check` through `EmbarkStudios/cargo-deny-action`
- `cargo check --workspace --locked` on Rust `1.88.0`
- special `spall-openapi` dependency-purity check using `cargo tree`

## Other Paths

- `reserve/`: crates.io name reservation placeholder, not implementation.
- `.hermes/`, `.pi/`, `notes/`, root plan files: planning/context directories. Use only as supporting evidence.
