## Goal
Implement the three most impactful missing features from the spall Wave 3/4 roadmap using sub-agents for better context management:
1. **URL Spec Loading** in `spall-core/src/loader.rs` (stub currently rejects URLs)
2. **Syntax Highlighting** in `spall-cli/src/output.rs` (syntect dependency already present)
3. **REPL / Shell Mode** interaction loop in `spall-cli`

## Current Context / Assumptions
- **loader.rs** line 46-48: URL sources return an error: "URL sources not yet supported in Wave 1"
- **output.rs**: Table/CSV modes exist but Pretty JSON output has no syntax highlighting (already depends on `syntect` in spall-cli/Cargo.toml)
- **main.rs**: No REPL mode exists — only single-shot command execution
- **Architecture**: Two-phase clap parse. Phase 1 builds stub commands from registry. Phase 2 loads spec and rebuilds command tree.
- **Auth**: OAuth2 stub exists (`auth/oauth2.rs` only applies tokens, no PKCE flow)
- **Dependencies**: `syntect = "0.20"` in spall-cli/Cargo.toml but unused for output formatting

## Proposed Approach
Use three parallel sub-agents, one per feature, then integrate into a single PR:

### Sub-Agent 1: URL Spec Loading
Implement HTTP fetching for spec sources in `spall-core/src/loader.rs`, with caching through existing `spall_core::cache::load_or_resolve`.

### Sub-Agent 2: Syntax Highlighting  
Wire `syntect` into `spall-cli/src/output.rs` for Pretty JSON mode. Emit highlighted code blocks to stdout when TTY detected. Use `base16-ocean.dark` theme or similar from syntect defaults.

### Sub-Agent 3: REPL Mode  
Add a `repl` module to `spall-cli/src/` and integrate into main.rs `match` on Phase 1 commands. Loop reading lines, dispatching to existing `handle_api_operation` logic.

## Step-by-Step Plan

### Phase A — Parallel implementation (sub-agents)

| # | Task | File(s) | Deliverable |
|---|------|---------|-------------|
| A1 | **URL fetch module** | `spall-core/src/fetch.rs` (new) | HTTP fetch with `reqwest`, cache integration, retry, proxy support |
| A2 | **Loader.rs integration** | `spall-core/src/loader.rs` | Replace `load_raw` reject with `fetch::load_raw` call. Keep same error interface. |
| A3 | **Output highlighting** | `spall-cli/src/output.rs` | Add `highlight_json()` using `syntect`. Wire into `emit_json_value` for Pretty mode. |
| A4 | **REPL module** | `spall-cli/src/repl.rs` (new) | History-backed loop using `rustyline` or `dialoguer`. Dispatch per-API commands. |
| A5 | **main.rs integration** | `spall-cli/src/main.rs` | Add `repl` subcommand and dispatch loop. |

### Phase B — Integration & tests

| # | Task | File(s) | Validation |
|---|------|---------|------------|
| B1 | Cargo deps | `spall-cli/Cargo.toml` | Add `rustyline` dependency for REPL |
| B2 | E2E URL test | `spall-cli/tests/url_e2e.rs` | Start mock server, `spall api add <url>`, hit endpoint via URL-loaded spec |
| B3 | Highlighting test | `spall-cli/tests/highlight_e2e.rs` | Verify TTY output contains ANSI escape codes |
| B4 | REPL test | `spall-cli/tests/repl_e2e.rs` | Spawn REPL, send commands, assert output |

### Phase C — Polish
- Ensure `fetch.rs` handles `insecure`/`proxy`/`timeout` via HttpConfig builders.
- Ensure REPL supports `history` and `repeat` natively.
- Add `--spall-repl` flag to main.rs Phase 1 globals or make `repl` a first-class subcommand.

## Risks, Tradeoffs, and Open Questions

1. **Sub-agent context limits**: Each sub-agent gets a fresh terminal session and limited toolset (no delegate_task inside them). I will pack full context (file paths, current code, constraints) into each `context` field.
2. **Error propagation from fetch**: `SpallCoreError::InvalidSource` currently wraps a string. Fetch should produce a new `SpallCoreError::Network(String)` variant or reuse existing.
3. **REPL tool choice**: `rustyline` is best but adds a dependency. Alternative: thin stdin loop with `dialoguer`. We'll use `rustyline` for history/editing.
4. **syntect asset bundling**: Syntect requires a theme set. We'll embed `base16-ocean.dark` via `syntect::dumps::from_uncompressed_data` from a bundled binary, or lazy-download.
5. **No open questions.** Approach is clear.

## How to execute
Run the three sub-agents in parallel via `delegate_task(tasks=...)` with role='orchestrator'. After they return, review diffs, run tests, and commit.

## Verification Steps
- `cargo test --workspace`
- `cargo build --workspace`
- `cargo clippy --workspace`
- Manual: `echo '{"test": true}' | spall testapi get-data --spall-output pretty` → highlighted JSON output
- Manual: `spall repl` → interactive loop against default or registered API

## Files Likely to Change
- `spall-core/src/loader.rs` (fetch integration)
- `spall-core/src/fetch.rs` (new)
- `spall-cli/src/output.rs` (highlighting)
- `spall-cli/src/repl.rs` (new)
- `spall-cli/src/main.rs` (repl subcommand)
- `spall-cli/Cargo.toml` (rustyline)
- `spall-cli/tests/url_e2e.rs` (new)
- `spall-cli/tests/highlight_e2e.rs` (new)
- `spall-cli/tests/repl_e2e.rs` (new)
