# Architecture

## What This Project Appears To Be

Verified: Spall is a Rust-native dynamic OpenAPI 3.x CLI. It parses OpenAPI specs at runtime, builds commands dynamically, executes HTTP requests, and supports validation, auth, formatted output, history, pagination, REPL, Arazzo workflows, and MCP exposure.

Evidence: `README.md`, `CLAUDE.md`, `Cargo.toml`, `spall-cli/src/main.rs`, `docs/src/SUMMARY.md`.

## Major Subsystems

- `spall-core`: OpenAPI/Arazzo parsing, OpenAPI-to-IR resolution, YAML parsing, cache serialization, validation helpers.
- `spall-config`: XDG config loading, API registry, profile overlays, auth config, credential classification.
- `spall-openapi`: transport-neutral request/response contract, request builder, auth contributors, status, RFC 5988 pagination, bounded streaming JSON item extraction.
- `spall-cli`: binary crate, dynamic Clap command construction, concrete `reqwest` transport, auth resolution, output/history/retry/pagination, REPL, Arazzo runner, MCP server.
- `docs/src`: mdBook user documentation.
- `e2e` and `spall-cli/tests`: end-to-end and integration test surfaces.

## Data And Control Flow

Verified:

1. `spall-cli` loads `ApiRegistry` from `spall-config`.
2. Phase 1 registers built-ins and API-name stubs without loading specs.
3. Once an API/operation is selected, the CLI loads or fetches raw spec bytes.
4. `spall-core` parses JSON/YAML and resolves OpenAPI into `ResolvedSpec`.
5. `spall-cli::command` builds dynamic operation commands from `ResolvedSpec`.
6. Execution builds or adapts a transport-neutral `HttpRequestSpec`.
7. `spall-cli::transport` sends the request through `reqwest`.
8. Output, validation, pagination, history, filtering, and exit-code mapping are handled in `spall-cli`.

Strong inference: Arazzo and MCP reuse the CLI/programmatic request path to avoid behavior drift. Evidence includes `execute_operation_programmatic`, Arazzo runner notes, and MCP tests.

## Boundaries

- `spall-core` owns OpenAPI/Arazzo parsing and IR, but not CLI command building, HTTP, async runtime, or credentials.
- `spall-openapi` owns neutral request/response types and streaming contracts, but not concrete transport, config, file I/O, stdin, OAuth login, or credential resolution.
- `spall-config` owns config and registry data, but should not parse specs.
- `spall-cli` owns concrete runtime behavior and is allowed to depend on all workspace crates.

## Public API Surfaces And Entry Points

- Binary: `spall-cli` exposes `spall`.
- `spall-core`: `loader`, `resolver`, `cache`, `validator`, `yaml`, `value`, `arazzo`.
- `spall-config`: `ApiRegistry`, `ApiEntry`, config source scanners, auth config types.
- `spall-openapi`: `build_request`, `HttpRequestSpec`, `ResponseStream`, `ItemStream`, `JsonSkimmer`, auth contributors.
- User docs expose `spall api`, dynamic `<api> <operation>`, `spall auth`, `spall history`, `spall repl`, `spall arazzo`, `spall mcp`, and completions.

## Ownership, State, And Concurrency

Verified:

- The CLI uses a current-thread Tokio runtime.
- Request history is stored through SQLite via `rusqlite`.
- `ResponseContext` carries successful operation response state for REPL/chain use and should not be global.
- MCP HTTP has session/body limits and stderr/stdout separation requirements.
- `spall-openapi::ItemStream` is lazy and iterator-based.

## Configuration, Serialization, And Resource Loading

Verified:

- Config loads from XDG config paths and per-API TOML files.
- `spall-config` uses `SecretString` for secret-bearing config and intentionally redacts inline secret serialization.
- YAML parsing goes through `spall_core::yaml` with hard input/structure budgets.
- IR cache uses `postcard`, `sha2`, `.ir`, `.idx`, and `.meta` files, with an `IR_VERSION`.
- `spall-core::loader` rejects URL loading; CLI-owned fetch code handles URLs and passes bytes into core.

## Error Handling Strategy

Verified:

- Library crates use `thiserror`.
- CLI errors use `miette::Diagnostic` and map to explicit exit codes.
- `spall-cli` distinguishes usage, network, spec, HTTP 4xx, HTTP 5xx, and validation exits.

Open question: root `CLAUDE.md` says no `.unwrap()` in library crates, but source includes at least one mutex-lock `unwrap()` in `spall-core::validator`. Clarify whether the rule is strict or mainly about user-input paths.

## Extension And Plugin Boundaries

Verified:

- OpenAPI `x-cli-*` extensions are parsed through `spall-core::extensions`.
- MCP exposes OpenAPI operations as tools.
- Arazzo support exists in `spall-core` models/expressions and `spall-cli` runner.

Hypothesis: future plugin/mock-server ideas in `docs/internal/plan` are not current implemented surfaces unless source confirms them.

## Areas Of Uncertainty

- Exact source priority between per-API files, inline config, and spec directories conflicts with older docs.
- Proxy precedence may conflict between `spall-cli/src/http.rs` comments and implementation.
- Some internal plan documents are stale relative to current code.
