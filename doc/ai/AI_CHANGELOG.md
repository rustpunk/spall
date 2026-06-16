# AI Changelog

## Purpose

This file is a lightweight architecture and onboarding memory for future agents. It should record major architecture facts, rule changes, and unresolved questions discovered during AI-assisted work.

Do not invent past decisions. Record only facts supported by current files, command output, or explicit human instruction.

## 2026-06-15 - Initial Onboarding Documentation

Created the durable AI onboarding set under `doc/ai/`, root `AGENTS.md`, and high-priority local `AGENTS.md` files.

### Major Architecture Facts Discovered

- Spall is a Rust Cargo workspace with four main packages: `spall-core`, `spall-config`, `spall-openapi`, and `spall-cli`.
- `spall-cli` owns the `spall` binary, dynamic command building, concrete HTTP transport, auth resolution, history, REPL, Arazzo runner, and MCP server.
- `spall-core` owns OpenAPI/Arazzo parsing, resolution, IR, cache, YAML parsing, validation helpers, and `SpallValue`.
- `spall-config` owns config loading, registry construction, profile overlays, auth config, and credential classification.
- `spall-openapi` owns the transport-neutral request/response contract, request builder, auth contributors, pagination, and bounded JSON streaming.
- CI runs workspace tests, clippy, rustdoc warnings, fmt, cargo-deny, MSRV 1.88, and a `spall-openapi` dependency-purity check.

### Open Question Routing

Current unresolved questions are tracked in `doc/ai/80_OPEN_QUESTIONS.md`. Keep this changelog focused on dated evidence, resolved uncertainty, and factual documentation maintenance history.
## Future Update Instructions

When architecture changes:

- Add a dated entry here.
- Link the changed docs or local `AGENTS.md` files.
- Record what was verified and what remains uncertain.
- Keep entries short and factual.
