# AGENTS.md

## Project Summary

Spall is a Rust Cargo workspace for a dynamic OpenAPI 3.x CLI. It loads API specs at runtime, builds command-line operations without code generation, executes requests, and exposes related workflows such as Arazzo and MCP.

## Read First

- Start with [doc/ai/00_READ_THIS_FIRST.md](doc/ai/00_READ_THIS_FIRST.md).
- Use [doc/ai/10_ARCHITECTURE.md](doc/ai/10_ARCHITECTURE.md) for subsystem boundaries.
- Use [doc/ai/50_TESTING_AND_COMMANDS.md](doc/ai/50_TESTING_AND_COMMANDS.md) before claiming verification.
- Check local `AGENTS.md` files before editing a crate directory.

## Repository Layout

- `spall-core/`: OpenAPI/Arazzo parsing, resolution, validation, YAML chokepoint, IR, and cache.
- `spall-config/`: config files, API registry, auth config, credential classification.
- `spall-openapi/`: transport-neutral request/response contract, auth contributors, pagination, bounded JSON streaming.
- `spall-cli/`: `spall` binary, dynamic Clap command tree, concrete HTTP transport, auth resolution, history, REPL, Arazzo, MCP.
- `docs/src/`: mdBook source for user documentation.
- `docs/internal/`: design notes, plans, and research. Treat older plans as evidence, not current truth.
- `e2e/`, `examples/`: smoke tests, fixtures, and example config.
- `doc/ai/`: durable onboarding docs for agents and contributors.

## Design Rules

- Keep `spall-core` free of `clap`, HTTP clients, async runtimes, and credential resolution.
- Keep `spall-openapi` transport-neutral: no `reqwest`, `tokio`, `clap`, `spall-config`, or `*-sys` dependency.
- Keep concrete transport, runtime, and CLI behavior in `spall-cli`.
- Use `--spall-*` for internal CLI flags.
- Route YAML parsing through `spall_core::yaml::from_str`.
- Do not put secrets or `SecretString` into IR/cache types.
- Preserve bounded-memory streaming behavior in `spall-openapi::stream`.
- Treat operation-level OpenAPI parameters/security as overriding inherited path/root values where current code does so.

## Commands

- Metadata: `cargo metadata --no-deps --format-version 1`
- Build: `cargo build --workspace`
- Test: `cargo test --workspace`
- Focused test: `cargo test -p <crate>`
- Format check: `cargo fmt --all -- --check`
- Lint: `cargo clippy --workspace -- -D warnings`
- Docs: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`
- MSRV check in CI: Rust 1.88 with `cargo check --workspace --locked`
- Supply-chain check: `cargo deny check`
- Smoke: `cargo build --release` then `./e2e/smoke.sh ./target/release/spall`

## Safety Rules

- Do not modify application/source code for documentation-only tasks.
- Do not modify `Cargo.lock`, add dependencies, push, or commit without explicit human approval.
- Do not treat generated `docs/book/` as the primary source; edit `docs/src/` for user docs and `doc/ai/` for onboarding docs.
- Do not hide uncertainty. Add open questions to [doc/ai/80_OPEN_QUESTIONS.md](doc/ai/80_OPEN_QUESTIONS.md).
- Ask before changing dependency boundaries, cache format/versioning, secret handling, MCP wire behavior, or CLI flag semantics.

## Coding Conventions

- Rust 2021 workspace, except `spall-openapi` is edition 2024. Workspace MSRV is 1.88.
- Libraries use `thiserror`; CLI diagnostics use `miette`.
- Prefer `IndexMap` where order affects UX or cache-visible behavior.
- Use dynamic Clap builder APIs rather than derive for OpenAPI-derived commands.

## Documentation Updates

- Update `doc/ai/AI_CHANGELOG.md` when architecture facts or boundaries change.
- Update `doc/ai/30_DESIGN_RULES.md` when a rule changes.
- Update `doc/ai/40_COMMON_PATTERNS.md` only for repeated, evidenced patterns.
- Update local `AGENTS.md` files when crate-specific invariants or commands change.

## Definition Of Done

- Relevant local `AGENTS.md` and `doc/ai` docs were read.
- Changes respect crate boundaries and safety rules.
- Focused tests or checks were run, or skipped with a concrete reason.
- Docs were updated when behavior, commands, or architecture changed.
- Open questions and hypotheses remain clearly labeled.
