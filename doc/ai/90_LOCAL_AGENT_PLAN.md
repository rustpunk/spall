# Local Agent Plan

## Recommended Local `AGENTS.md` Locations

| Location | Priority | Why Local Guidance Is Needed | Confidence |
| --- | --- | --- | --- |
| `spall-core/AGENTS.md` | High | Core owns parser/resolver/cache/YAML IR contracts and has dependency restrictions | High |
| `spall-openapi/AGENTS.md` | High | CI-enforced transport-neutral boundary and bounded-memory streaming invariants | High |
| `spall-cli/AGENTS.md` | High | Runtime behavior is broad and easy to fork into parallel paths | High |
| `spall-config/AGENTS.md` | High | Secret handling, config source priority, and registry-lightweight invariants need local reminders | High |
| `doc/ai/AGENTS.md` | High | Durable onboarding docs need evidence labels and update rules | High |

## Rules To Include

- `spall-core`: no CLI/HTTP/runtime/credential deps; YAML chokepoint; cache/IR version discipline; preserve OpenAPI inheritance rules.
- `spall-openapi`: no concrete transport/runtime/CLI/config deps; preserve streaming caps; request specs must not perform I/O.
- `spall-cli`: dynamic Clap builder; MCP stdout discipline; use shared request path; preserve output-before-error mapping; handle multipart retry carefully.
- `spall-config`: no spec parsing; keep secrets redacted; preserve `deny_unknown_fields`; document config source priority uncertainty.
- `doc/ai`: use evidence labels, update open questions/changelog, do not invent intent.

## Local Commands

- `spall-core`: `cargo test -p spall-core`
- `spall-openapi`: `cargo test -p spall-openapi`, `cargo test -p spall-openapi --test stream_bound`
- `spall-cli`: `cargo test -p spall-cli`, focused e2e tests under `spall-cli/tests`
- `spall-config`: `cargo test -p spall-config`
- `doc/ai`: `git diff --check`, markdown/path searches

## Risks If No Local Guidance Exists

- `spall-core`: agents may reintroduce Clap/HTTP/secret coupling or break cache format.
- `spall-openapi`: agents may add `reqwest`/Tokio/Clap or buffer whole responses.
- `spall-cli`: agents may create duplicate request pipelines or violate MCP stdout.
- `spall-config`: agents may persist secrets or parse specs during registry load.
- `doc/ai`: docs may become overconfident or stale.

## Evidence

Sources: root `Cargo.toml`, `CLAUDE.md`, `.github/workflows/ci.yml`, crate `Cargo.toml` files, crate docs, tests, explorer reports.

## Suggested Creation Batches

1. Batch 1: root `AGENTS.md`, `doc/ai/*`, `doc/ai/AGENTS.md`.
2. Batch 2: crate root local `AGENTS.md` files for all four workspace crates.
3. Batch 3: only if future work proves needed, add local guidance under `spall-cli/src/mcp/` or `spall-openapi/src/stream.rs` ownership areas. Do not create those initially to avoid clutter.

## Not Recommended Now Local `AGENTS.md`

- `reserve/`: placeholder crate, not active implementation.
- `examples/`: simple config examples.
- `e2e/`: smoke fixtures are small and covered by root/CLI docs.
- `docs/book/`: generated output; avoid editing.
- `.hermes/`, `.pi/`, `notes/`: planning/history directories, not core implementation.
- Individual `spall-cli/src/*` modules: start with crate-level guidance; add narrower files only after repeated mistakes.
