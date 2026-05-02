# Ideation: spall
_Dynamic OpenAPI 3.x CLI — break free, hit the endpoint._
_Generated: 2025-05-02 | Prior ideation: ideate-2026-04-28-openapi-cli-spall.md_

## Refactoring

### Replace `LAST_RESPONSE` static with an explicit `ResponseContext` type
- **Where:** `spall-cli/src/execute.rs` lines 14–26; `spall-cli/src/repl.rs` lines 155–175
- **What:** A `static Mutex<Option<serde_json::Value>>` is used to pass response JSON from `execute_operation` to `run_piped` across the async call boundary. This was pragmatic for shipping D3 but is a global mutable singleton that complicates testing and hides data flow.
- **Why it fits:** The current pattern makes unit-testing `run_piped` impossible without monkey-patching env or globals. An explicit `ResponseContext` passed as a parameter would make the dependency visible and the module testable.
- **Effort:** M

### Extract REPL command dispatch from string matching to an enum
- **Where:** `spall-cli/src/repl.rs` lines 38–81
- **What:** `match trimmed` against raw `&str` for `"quit"`, `"exit"`, `"help"`, `"history"`, and the catch-all dispatch. Adding a new command requires editing a long match arm and risks typo bugs.
- **Why it fits:** The current dispatch is fine for 4 commands but will not scale as the REPL grows. Converting to a `ReplCommand` enum with a parser function would make the REPL extensible for tab-completion and aliases.
- **Effort:** S

---

## Documentation

### Add module-level docs to `chain.rs`
- **Where:** `spall-cli/src/chain.rs` (file has no `//!` header)
- **What:** The chaining expression grammar (`opName --key $(jmespath)`), pipe semantics, and error variants are undocumented. A new contributor cannot discover how chaining works without reading `main.rs`.
- **Why it fits:** `chain.rs` is a new core module (post-D3) with public types. It is the primary interface for Wave 3's flagship feature; it deserves a synopsis.
- **Effort:** XS

### Document the pipe/response-capture semantics added in D3
- **Where:** `spall-cli/src/repl.rs` lines 30–35 (pipe detection); `spall-cli/src/execute.rs` lines 44–48 (`OperationResult`)
- **What:** The REPL's `|` syntax and `store_last_response`/`take_last_response` helpers have no doc comments or user-facing documentation. The `OperationResult` struct is `pub` but undocumented.
- **Why it fits:** These are new public APIs introduced during the current session's work. Without docs they will bit-rot as developers forget the JMESPath resolution order.
- **Effort:** XS

---

## Scope Gaps

### `spall-cli/src/chain.rs` has zero tests
- **Where:** `spall-cli/src/chain.rs` (entire file, 0 `#[cfg(test)]` blocks)
- **What:** The `ChainExpr` parser and `resolve` function are core to Wave 3 chaining but completely untested. There is no verification that `--id $(data.id)` parses correctly, that unknown JMESPath expressions fail gracefully, or that the recursive `handle_api_operation` call receives the right arguments.
- **Why it fits:** This module was created during the current batch of work (it did not exist in the 2026-04-28 ideation). It is in the critical path for every chained request.
- **Effort:** M

### REPL pipe error reporting is too coarse
- **Where:** `spall-cli/src/repl.rs` lines 32–34
- **What:** Any pipe stage failure prints `eprintln!("Pipe error: {:?}", e)`. This does not distinguish between (a) chain parse failure, (b) JMESPath evaluation failure, (c) HTTP error, or (d) missing previous response. Users cannot tell which stage failed or why.
- **Why it fits:** As the REPL pipe becomes the primary interactive chaining interface, actionable error messages are essential. The current `Debug` dump is a placeholder.
- **Effort:** S

---

## Feature Enrichment

### Add `--spall-dry-run` support for chained operations
- **Where:** `spall-cli/src/execute.rs` lines 100–120 (dry-run branch); `spall-cli/src/main.rs` (chain dispatch after `--spall-dry-run`)
- **What:** Dry-run mode prints the first request but does not show the derived arguments for subsequent chained stages. Users cannot verify what a chain would do without executing it.
- **Why it fits:** Dry-run is already implemented for single requests. Extending it to print each stage's resolved arguments (with interpolation values) is a natural UX improvement that reuses existing `preview.rs` patterns.
- **Effort:** S

### Pre-compile JMESPath expressions in `ChainExpr` at parse time
- **Where:** `spall-cli/src/chain.rs` (current `resolve` implementation)
- **What:** `ChainExpr::resolve` calls `jmespath::compile` for every binding, every stage, every invocation. JMESPath compilation is not free; repeated chains pay this cost repeatedly.
- **Why it fits:** The `ChainExpr` struct can store pre-compiled `jmespath::Expression` values alongside the raw strings. This is a single-field addition that improves pipe latency for multi-stage REPL sessions.
- **Effort:** S

---

## New Features (scope-aligned)

### `spall chain validate <expr> --against <file.json>`
- **Where:** New subcommand or flag; leverages `spall-cli/src/chain.rs` and `spall-cli/src/filter.rs`
- **What:** A CLI helper that takes a chain expression and a JSON payload, prints the resolved arguments for each stage without making HTTP requests. Useful for developing and debugging chain expressions.
- **Why it fits:** The chaining feature is new and users will need to iterate on JMESPath expressions. A validation helper is a natural twin to the `--spall-filter` testing workflow already present.
- **Effort:** S

---

## Quality

### Add unit tests for `ChainExpr` parser edge cases
- **Where:** `spall-cli/src/chain.rs`; create `#[cfg(test)]` block
- **What:** No tests exist for: empty chain expression, malformed JMESPath, missing `--` prefix on interpolated args, chaining with zero bindings, or chaining across different APIs.
- **Why it fits:** This is a direct continuation of the B2/B3 test-writing pattern just completed in `spall-config`. The parser is small enough to cover exhaustively in under 100 lines.
- **Effort:** S

### Add test for `store_last_response` / `take_last_response` edge cases
- **Where:** `spall-cli/src/execute.rs` lines 14–26
- **What:** No test verifies behavior when `take_last_response` is called before `store_last_response`, when the mutex is poisoned, or when a pipe stage fails midway and `take_last_response` returns stale data from a previous successful operation.
- **Why it fits:** The `LAST_RESPONSE` static was the pragmatic choice for D3 but is a subtle correctness risk. Even a few tests would document the expected semantics.
- **Effort:** XS

---

## DX / UX

### Replace `eprintln!("Pipe error: {:?}")` with structured REPL error output
- **Where:** `spall-cli/src/repl.rs` lines 32–34; `spall-cli/src/repl.rs` lines 74–80 (general dispatch error)
- **What:** Both pipe failures and general dispatch failures use `{:?}` debug formatting. The REPL should print human-readable, color-coded error summaries (stage number, error kind, suggestion).
- **Why it fits:** The REPL is an interactive shell; raw debug output breaks immersion and makes troubleshooting chains frustrating. `miette` is already a dependency.
- **Effort:** S

### Add `history search` subcommand
- **Where:** `spall-cli/src/commands/history.rs` (only `list`, `show`, `clear` exist); `spall-cli/src/history.rs` schema already supports filtering
- **What:** The SQLite schema stores `api`, `operation`, `method`, `url`, and `status_code`, but the CLI only allows listing by limit or fetching by ID. There is no way to search by API name, status code, or URL substring.
- **Why it fits:** This was already identified in the 2026-04-28 ideation but remains unimplemented. The schema is ready; the CLI surface is not.
- **Effort:** S

---

## Hygiene

### Remove `[lints.rust] unused = "allow"` and fix resulting warnings
- **Where:** `spall-cli/Cargo.toml` line 59; `spall-cli/src/commands/mod.rs`; `spall-cli/src/commands/api.rs`; `spall-cli/src/auth/oauth2.rs`
- **What:** The global override suppresses all unused warnings. Per-file `#![allow(unused_imports)]` in `commands/mod.rs` and `commands/api.rs` compound the issue. After D1–D3, some newly public functions may be unused in certain configurations.
- **Why it fits:** This was identified in the 2026-04-28 ideation as an XS-effort task but remains open. Cleaning it up now, after the D-series changes, would prevent dead code from accumulating.
- **Effort:** XS

---

## Open questions
- Is the `ChainExpr` grammar stable, or will it evolve to support arithmetic/transforms on captured values?
- Should pipe stages support conditional execution (e.g., `api op | if .ok then api op2`), or is that out of scope?
- Does the history schema need to store chain stage metadata, or is a single top-level record per chain sufficient?

## Out of scope
- OpenAPI 3.1 support (Wave 4, explicitly deferred in project docs)
- Daemon mode / plugin system (Wave 4, deferred)
- Full OAuth2 PKCE browser flow (stubbed in Wave 3 but beyond current bandwidth; already tracked in prior ideation)
- Replacing `openapiv3` with `openapiv3-extended` (Wave 4 re-evaluation, deferred)
