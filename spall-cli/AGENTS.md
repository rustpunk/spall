# AGENTS.md

## Purpose

`spall-cli` owns the `spall` binary and all concrete runtime behavior.

## Responsibilities

- Two-phase Clap parsing and dynamic operation command construction.
- API management, spec fetch/cache refresh, auth resolution, request execution.
- Concrete `reqwest` transport, output, history, repeat, filters, retries, pagination, chaining, REPL.
- Arazzo runner and MCP stdio/HTTP server.

## Public APIs And Entry Points

- Binary target: `spall`
- `main`, `run_with_args`, `handle_api_operation`
- `command::{build_operations_cmd, build_operations_cmd_from_index}`
- `execute::{execute_operation_programmatic, execute_operation, ProgrammaticArgs, OperationResult}`
- Built-in command handlers in `src/commands/`

## Internal Module Map

- `main.rs`: dispatch, exit codes, global flags, two-phase parse.
- `command.rs`: OpenAPI operation/parameter to Clap command/arg.
- `execute.rs`: request assembly, validation, history, pagination, follow, retry, filter.
- `transport.rs`: `HttpRequestSpec` to `reqwest`.
- `http.rs`: client/proxy/TLS/mTLS config.
- `fetch.rs`: raw spec loading and cache.
- `auth/`: credential resolution and OAuth2.
- `mcp/`: MCP server, schema conversion, verbose redaction.
- `arazzo_runner*.rs`: workflow runner.

## Dependency Rules

- This crate may depend on all workspace crates.
- Do not move concrete runtime behavior into `spall-core` or `spall-openapi`.
- Keep optional/heavy credential backends behind features as currently modeled.

## Invariants

- Internal flags use `--spall-*`.
- Phase 2 values override Phase 1 through `MergedMatches`.
- `--spall-chain` must be operation-local and remains within one API.
- Emit 4xx/5xx response bodies before mapping to exit codes.
- MCP stdio stdout must be JSON-RPC only; diagnostics go to stderr.
- Auth is applied before caller headers so caller headers can override.
- Multipart retries require special handling because forms are consumed.

## Common Mistakes

- Adding a parallel request execution path instead of reusing the shared programmatic path.
- Treating path/query/header/cookie params as one namespace.
- Making `ResponseContext` global.
- Printing logs to stdout from MCP paths.
- Refactoring `execute_legacy_path` without parity tests.

## Local Commands

- `cargo test -p spall-cli`
- `cargo test -p spall-cli chain_e2e`
- `cargo test -p spall-cli mcp_e2e`
- `cargo test -p spall-cli arazzo_run_e2e`
- `cargo build --release` then `./e2e/smoke.sh ./target/release/spall`

## Documentation Updates

Update `../doc/ai/10_ARCHITECTURE.md`, `../doc/ai/30_DESIGN_RULES.md`, `../doc/ai/50_TESTING_AND_COMMANDS.md`, and user docs under `../docs/src/` for CLI behavior changes.

## Unclear / Ask Human

- Proxy precedence may be inconsistent between comments and implementation.
- Long-term role of `execute_legacy_path`.
- MCP HTTP server deployment assumptions such as TLS/auth via reverse proxy.

## Evidence

`src/main.rs`, `src/command.rs`, `src/execute.rs`, `src/transport.rs`, `src/mcp/*`, `tests/*_e2e.rs`, `../e2e/smoke.sh`.
