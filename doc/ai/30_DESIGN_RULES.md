# Design Rules

## Core Philosophy

- Verified: Spall dynamically builds CLI commands from OpenAPI specs at runtime instead of generating code. Evidence: `README.md`, `docs/internal/spall-design.md`, `spall-cli/src/command.rs`.
- Verified: Current architecture is a four-crate workspace with explicit responsibility boundaries. Evidence: root `Cargo.toml`, `CLAUDE.md`, crate docs.

## Dependency Direction

- Verified: `spall-cli` may depend on `spall-core`, `spall-config`, and `spall-openapi`.
- Verified: `spall-openapi` depends on `spall-core` but must remain transport-neutral. CI forbids HTTP clients, async runtimes, CLI frameworks, and `*-sys` normal dependencies.
- Verified: `spall-core` is clap-free and does not own URL fetching.
- Verified: `spall-config` should build a lightweight API registry without parsing specs.
- Strong inference: `spall-core` and `spall-openapi` should not depend on `spall-cli`.

## Public API Rules

- Verified: Dynamic CLI command building belongs in `spall-cli::command`.
- Verified: Public neutral request/response types are exported by `spall-openapi`.
- Verified: `spall-core` IR types are cache-visible and must be treated as a stable-ish internal contract.
- Hypothesis: Adding or removing fields in `ResolvedSpec` should be paired with an `IR_VERSION` review and cache tests.

## Error Handling Rules

- Verified: Libraries use `thiserror`.
- Verified: CLI errors use `miette` diagnostics and explicit exit-code mapping.
- Verified: HTTP 4xx/5xx response bodies should be emitted before exit-code mapping.
- Hypothesis: "No `.unwrap()` in library crates" is aspirational or user-input-focused, because current library code contains at least one mutex-lock `unwrap()`.

## State, Ownership, And Concurrency

- Verified: `spall-cli` uses Tokio `current_thread`.
- Verified: `ResponseContext` is passed as mutable state for REPL/chain response flow; do not make it global.
- Verified: Request history uses SQLite through `rusqlite`.
- Verified: MCP stdio stdout must contain only JSON-RPC; diagnostics belong on stderr.

## Serialization And Config Rules

- Verified: All YAML parsing should go through `spall_core::yaml`.
- Verified: IR uses `SpallValue` for cached JSON-shaped fields instead of `serde_json::Value`.
- Verified: `SecretString` must not enter IR/cache types.
- Verified: `AuthConfig` inline secret serialization intentionally redacts/drops secrets.
- Verified: TOML-facing config structs use `deny_unknown_fields`.

## Testing Rules

- Verified: CI runs workspace test, clippy, doc, fmt, deny, MSRV, and `spall-openapi` dependency-purity jobs.
- Strong inference: Use crate-focused tests for local changes before full workspace checks.
- Strong inference: For streaming/pagination changes, run `cargo test -p spall-openapi --test stream_bound` and relevant CLI pagination/stream guard tests.

## Performance Rules

- Verified: Keep registry loading lightweight and spec-parse-free.
- Verified: Preserve two-phase CLI parse so top-level dispatch does not load specs unnecessarily.
- Verified: Preserve bounded-memory streaming in `spall-openapi::stream`.
- Verified: Cache writes use temp files and atomic rename.
- Strong inference: `SpecIndex` exists to support degraded help without loading a full IR.

## Documentation Rules

- Verified: Edit `docs/src` for mdBook source, not generated `docs/book`.
- Verified: Edit `doc/ai` for AI onboarding docs.
- Strong inference: Older planning docs should be cited as historical intent unless source confirms current behavior.

## Never Do This Unless Explicitly Approved

- Add dependencies or modify `Cargo.lock`.
- Push or commit.
- Add `reqwest`, `tokio`, `clap`, `spall-config`, or `*-sys` normal deps to `spall-openapi`.
- Add concrete transport, async runtime, or credentials to `spall-core`.
- Reintroduce `serde_json::Value` into cache-visible IR fields.
- Buffer whole paginated response pages in `ItemStream`.
- Emit non-protocol diagnostics to MCP stdio stdout.
- Change CLI flag names, exit codes, cache format, or secret-handling semantics without tests and human review.

## Ask The Human Before Changing These Areas

- Cache format/versioning and IR layout.
- `spall-openapi` dependency boundary.
- Secret resolution order, persistence, or redaction.
- MCP wire protocol behavior.
- Proxy precedence, because comments and implementation may disagree.
- Config source priority, because older docs and implementation may disagree.
