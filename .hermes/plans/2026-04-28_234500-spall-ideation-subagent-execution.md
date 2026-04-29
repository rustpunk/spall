# Execution Plan: Spall Accepted Ideation Batch
## Subagent-Driven Development

> Derived from: `.hermes/plans/2026-04-28_spall-accepted-ideation.md`
> Source verified: 2026-04-28 against actual files.

## Goal
Execute the 5 accepted ideas via the `subagent-driven-development` skill. Parallelize where safe (different crates / non-overlapping modules). Serialize where modules overlap.

## Source Truth (Current State)

| File | Lines | Notes |
|------|-------|-------|
| `spall-core/src/loader.rs` | 60 | `load_raw` still has "Wave 1: local file system only" comment. Error msg says "URL sources not yet supported in Wave 1". |
| `spall-config/src/sources.rs` | 261 | TOML structs (`SpallConfig`, `InlineApi`, `Defaults`, `ProxyDefaults`, `ApiToml`, `ProfileToml`) are **private**; no `deny_unknown_fields`. |
| `spall-config/src/auth.rs` | 126 | `AuthConfig` is `pub`; no `deny_unknown_fields`. |
| `spall-config/src/registry.rs` | 170 | `ApiRegistry::load()` hardcoded to `config_dir()`. No testable constructor. |
| `spall-config/src/credentials.rs` | 48 | `CredentialResolver::resolve()` is `todo!()`. Only `env_var_name()` is testable. |
| `spall-config/Cargo.toml` | 14 | No `[dev-dependencies]`. |
| `spall-cli/src/execute.rs` | 653 | `MergedMatches` defined inline at lines ~417-437. |
| `spall-cli/src/http.rs` | 234 | Manual Phase1/Phase2 closures (`get_timeout`, `get_retry`, etc.). `resolve_proxy` uses `get_flag_safe` / `get_one_safe`. |
| `spall-cli/src/main.rs` | 658 | No `chain` or `matches` module declarations. `--spall-chain` flag does not exist. |
| `spall-cli/src/repl.rs` | 119 | No pipe syntax. Dispatches via `run_with_args`. |
| `spall-cli/Cargo.toml` | 59 | `jmespath = "0.5"` present. `tempfile = "3"` present under `[dev-dependencies]`. |

## Critical Corrections vs Original Plan

1. **TOML structs are private.** Tests for `SpallConfig`/`ApiToml` must live inside `#[cfg(test)]` blocks in `sources.rs`, or structs must be exposed as `pub(crate)`. Implementer should prefer in-module unit tests to minimize API surface changes.
2. **No `resolve_env` in `credentials.rs`.** The original plan's B2 test references a non-existent function. Tests should target `CredentialResolver::env_var_name()` and `default_token_env()`.
3. **`spall-config` lacks `tempfile`.** If integration tests need temp dirs, the subagent must add `tempfile = "3"` to `[dev-dependencies]`.
4. **REPL chaining is architecturally blocked** because `run_with_args` returns `miette::Result<()>` and prints to stdout; there is no response body return path. Task 6 is restricted to pipe-detection scaffolding + `todo!()` with a documented gap.

## Dispatch Strategy

### Wave 1 — Parallel Leaf Tasks (independent crates)

These three tasks touch **three separate crates** and do not overlap on any module. Dispatch in parallel.

---

#### Task 1: Fix stale Wave 1 comment in `loader.rs`

**Crate:** `spall-core`
**Files:** `spall-core/src/loader.rs`

**Spec:**
1. In `load_raw`, replace the doc comment:
   - Remove: `Wave 1: local file system only. Wave 1.5: URL fetching with caching.`
   - Replace with a neutral description, e.g. `Load raw bytes from a local file path.`
2. In `load_raw`, replace the error message:
   - From: `"URL sources not yet supported in Wave 1: {source}"`
   - To: `"URL sources require the CLI fetch layer (spall_cli::fetch), not spall_core::loader: {source}"`
3. In `load_spec` doc comment (lines 5-8), update text to clarify URL sources are handled by the CLI layer and resolved bytes are passed to `load_spec_from_bytes`.

**Verification:**
- `cargo build -p spall-core`
- `cargo clippy -p spall-core`

**Commit:**
```bash
git add spall-core/src/loader.rs
git commit -m "fix(loader): remove stale Wave 1 URL rejection message"
```

---

#### Task 2: Strict config deserialization + sources unit tests

**Crate:** `spall-config`
**Files:** `spall-config/src/sources.rs`, `spall-config/src/auth.rs`

**Spec:**
1. In `sources.rs`, add `#[serde(deny_unknown_fields)]` to the following **private** TOML structs:
   - `SpallConfig`
   - `InlineApi`
   - `Defaults`
   - `ProxyDefaults`
   - `ApiToml`
   - `ProfileToml`
2. In `auth.rs`, add `#[serde(deny_unknown_fields)]` to `AuthConfig`.
3. **Before adding the attribute**, verify none of these structs use `#[serde(flatten)]`. If one does, skip `deny_unknown_fields` for that struct and document why in a comment.
4. Append a `#[cfg(test)] mod tests { use super::*; ... }` block to the **bottom of `sources.rs`** with the following tests:
   - `test_reject_unknown_fields_api_toml`: parse a TOML string containing `baseurl = "..."` (typo for `base_url`) via `toml::from_str::<ApiToml>`. Assert the result is `Err`.
   - `test_reject_unknown_fields_spall_config`: parse a TOML string with an unknown top-level key into `SpallConfig`. Assert `Err`.
   - `test_empty_config`: parse empty string into `SpallConfig`. Assert `api.is_empty()`.
   - `test_defaults_populated`: parse a `[defaults]` section with `output` and `color`. Assert fields match.

**Verification:**
- `cargo test -p spall-config`
- `cargo clippy -p spall-config`

**Commit:**
```bash
git add spall-config/src/sources.rs spall-config/src/auth.rs
git commit -m "feat(config): deny unknown fields in TOML config"
```

---

#### Task 3: Registry testability + registry/credentials tests

**Crate:** `spall-config`
**Files:** `spall-config/src/registry.rs`, `spall-config/src/credentials.rs`, `spall-config/Cargo.toml`

**Spec:**
1. In `registry.rs`, expose a **test-friendly constructor** for `ApiRegistry`. The minimal approach is preferred:
   ```rust
   impl ApiRegistry {
       pub fn from_entries(entries: Vec<ApiEntry>, defaults: crate::sources::GlobalDefaults) -> Self {
           Self { apis: entries, defaults }
       }
   }
   ```
   (If an internal helper is needed to keep `load()` DRY, extract it, but do not change the public signature of `load()`.)
2. Create `spall-config/tests/registry_test.rs`. Use `ApiRegistry::from_entries` to construct a registry with 2-3 `ApiEntry` values. Test:
   - `find` returns the correct entry by name.
   - `find` returns `None` for a missing name.
   - `resolve_profile` correctly overlays `base_url`, `headers`, `proxy`, and `auth` when a matching profile exists.
   - `resolve_profile` returns the base entry unchanged when no profile is requested.
3. Create `spall-config/tests/credentials_test.rs`. Test:
   - `CredentialResolver::env_var_name()` for an API named `"my-api"` returns `"SPALL_MY_API_TOKEN"`.
   - `spall_config::auth::default_token_env("foo-bar")` returns `"SPALL_FOO_BAR_TOKEN"`.
   - **Do not** call `CredentialResolver::resolve()` — it is `todo!()` and will panic.
4. If `tempfile` is needed for any test fixture, add `tempfile = "3"` to `[dev-dependencies]` in `spall-config/Cargo.toml`.

**Verification:**
- `cargo test -p spall-config`
- `cargo clippy -p spall-config`

**Commit:**
```bash
git add spall-config/src/registry.rs spall-config/tests spall-config/Cargo.toml
git commit -m "test(config): add registry and credential resolution tests"
```

---

### Wave 2 — Refactoring (touches `spall-cli/src/main.rs`)

This task modifies `spall-cli/src/main.rs`, so it must wait until Wave 1 is fully merged.

---

#### Task 4: Extract `MergedMatches` to shared module

**Crate:** `spall-cli`
**Files:**
- Create: `spall-cli/src/matches.rs`
- Modify: `spall-cli/src/execute.rs`
- Modify: `spall-cli/src/http.rs`
- Modify: `spall-cli/src/main.rs`

**Spec:**
1. Create `spall-cli/src/matches.rs`:
   ```rust
   use clap::ArgMatches;

   /// Unified view over Phase 1 and Phase 2 clap matches.
   /// Prefers Phase 2 values, falling back to Phase 1.
   #[derive(Debug, Clone, Copy)]
   pub struct MergedMatches<'a> {
       pub phase1: &'a ArgMatches,
       pub phase2: &'a ArgMatches,
   }

   impl<'a> MergedMatches<'a> {
       pub fn get_flag(&self, id: &str) -> bool {
           self.phase2.get_flag(id) || self.phase1.get_flag(id)
       }

       pub fn get_one<T: Clone + Send + Sync + 'static>(&self, id: &str) -> Option<T> {
           self.phase2
               .get_one::<T>(id)
               .cloned()
               .or_else(|| self.phase1.get_one::<T>(id).cloned())
       }

       pub fn get_many<T: Clone + Send + Sync + 'static>(
           &self,
           id: &str,
       ) -> Option<clap::parser::ValuesRef<'a, T>> {
           self.phase2.get_many::<T>(id).or_else(|| self.phase1.get_many::<T>(id))
       }
   }
   ```
   **Critical:** the lifetime on `ValuesRef` must be `'a` so it borrows from the underlying `ArgMatches` refs.
2. In `execute.rs`:
   - Delete the `struct MergedMatches` and its `impl` block (lines ~417-437).
   - Add `use crate::matches::MergedMatches;`.
   - Keep the `merge_matches()` helper function unchanged (it constructs the struct).
3. In `http.rs`:
   - Rewrite `config_from_matches` to construct `let m = MergedMatches { phase1: p1, phase2: p2 };` and use `m.get_one::<u64>("spall-timeout")`, `m.get_flag("spall-follow")`, etc. Delete all manual closure fallback helpers (`get_timeout`, `get_retry`, `get_max`, `get_cert`, `get_proxy`, `get_server`, `get_auth`, `get_headers`).
   - Rewrite `resolve_proxy` to construct the same `MergedMatches` and use `m.get_flag("spall-no-proxy")` / `m.get_one::<String>("spall-proxy")`. Delete `get_flag_safe` and `get_one_safe` **if** they are no longer used anywhere in `http.rs`.
   - Preserve the **exact** proxy resolution priority:
     1. `--spall-no-proxy`
     2. `--spall-proxy`
     3. `entry.proxy`
     4. `env_proxy()`
     5. `global_defaults.proxy`
     6. `None`
4. In `main.rs`: add `mod matches;` alongside the other module declarations.

**Verification:**
- `cargo build --workspace`
- `cargo test --workspace`
- `cargo clippy --workspace`

**Commit:**
```bash
git add spall-cli/src/matches.rs spall-cli/src/execute.rs spall-cli/src/http.rs spall-cli/src/main.rs
git commit -m "refactor(cli): extract MergedMatches for Phase1/Phase2 flag lookup"
```

---

### Wave 3 — Chaining (depends on Wave 2 because `main.rs` is modified)

Task 5 and Task 6 both touch `spall-cli`. Task 5 modifies `main.rs`; Task 6 modifies `repl.rs`. Task 6 can run after Task 5 because it doesn't touch `main.rs`, but for simplicity and review sanity, run them sequentially.

---

#### Task 5: Add `--spall-chain` flag and `chain.rs` parser

**Crate:** `spall-cli`
**Files:**
- Modify: `spall-cli/src/main.rs`
- Create: `spall-cli/src/chain.rs`

**Spec:**
1. In `main.rs`:
   - Add `mod chain;` to the module list.
   - In `spall_global_args()`, append:
     ```rust
     Arg::new("spall-chain")
         .long("spall-chain")
         .value_name("EXPR")
         .global(true)
         .help("Chain request: capture values from response and feed into another operation (e.g. 'op2 --id $.data.id')"),
     ```
2. Create `spall-cli/src/chain.rs`:
   ```rust
   use serde_json::Value;

   #[derive(Debug, Clone)]
   pub struct ChainExpr {
       pub target_op_id: String,
       pub bindings: Vec<(String, String)>,
   }

   impl ChainExpr {
       pub fn parse(expr: &str) -> Result<Self, crate::SpallCliError> {
           let mut parts = expr.split_whitespace();
           let target_op_id = parts
               .next()
               .ok_or_else(|| crate::SpallCliError::Usage(
                   "chain expression requires target operation".to_string()
               ))?
               .to_string();

           let mut bindings = Vec::new();
           while let Some(token) = parts.next() {
               if token.starts_with("--") {
                   let param_id = token.trim_start_matches("--").to_string();
                   let jmespath = parts
                       .next()
                       .ok_or_else(|| crate::SpallCliError::Usage(
                           format!("chain param '{}' needs a jmespath expression", param_id)
                       ))?;
                   bindings.push((param_id, jmespath.to_string()));
               }
           }

           Ok(ChainExpr { target_op_id, bindings })
       }

       pub fn resolve(&self, response_json: &Value) -> Result<Vec<String>, crate::SpallCliError> {
           let mut args = vec![self.target_op_id.clone()];
           for (param, expr) in &self.bindings {
               match jmespath::search(expr, response_json) {
                   Ok(val) => {
                       let s = match val {
                           jmespath::Variable::String(s) => s,
                           other => other.to_string(),
                       };
                       args.push(format!("--{}", param));
                       args.push(s);
                   }
                   Err(e) => {
                       return Err(crate::SpallCliError::Usage(format!(
                           "chain jmespath error for '{}': {}", expr, e
                       )));
                   }
               }
           }
           Ok(args)
       }
   }
   ```
   **Note:** the `jmespath` crate API must be verified. If `jmespath::Variable` does not exist in version 0.5, adapt to the actual crate types (e.g. `jmespath::JmespathResult` or `serde_json::Value`).

**Verification:**
- `cargo build --workspace`
- `cargo test --workspace`
- `cargo clippy --workspace`

**Commit:**
```bash
git add spall-cli/src/chain.rs spall-cli/src/main.rs
git commit -m "feat(cli): add --spall-chain flag and chain expression parser"
```

---

#### Task 6: REPL pipe syntax scaffolding

**Crate:** `spall-cli`
**Files:** `spall-cli/src/repl.rs`

**Spec:**
1. In the REPL loop (`run`), before the `match trimmed` block, detect pipe characters:
   ```rust
   if trimmed.contains('|') {
       let stages: Vec<&str> = trimmed.split('|').map(|s| s.trim()).collect();
       if let Err(e) = run_piped(&stages, registry, cache_dir).await {
           eprintln!("Pipe error: {:?}", e);
       }
       continue;
   }
   ```
2. Add the `run_piped` async helper at the bottom of `repl.rs`:
   ```rust
   async fn run_piped(
       stages: &[&str],
       registry: &spall_config::registry::ApiRegistry,
       cache_dir: &std::path::Path,
   ) -> Result<(), crate::SpallCliError> {
       // TODO: implement piped execution.
       // This requires `run_with_args` (or a new internal dispatch function)
       // to return the response body so that JMESPath expressions can be
       // evaluated against it before dispatching the next stage.
       eprintln!("Pipe syntax detected with {} stages: {:?}", stages.len(), stages);
       eprintln!("Piped execution is not yet implemented.");
       Ok(())
   }
   ```
   The helper must compile and print a clear message. It must **not** panic.

**Verification:**
- `cargo build --workspace`
- `cargo test --workspace`
- `cargo clippy --workspace`

**Commit:**
```bash
git add spall-cli/src/repl.rs
git commit -m "feat(repl): add pipe syntax scaffolding for request chaining"
```

---

## Execution Order

1. **Wave 1** — Fire Task 1, Task 2, Task 3 in parallel via `delegate_task`.
2. **Controller Integration** — After Wave 1 returns:
   - Audit every deliverable (read changed files).
   - Run `cargo test --workspace` and `cargo clippy --workspace`.
   - Fix any merge conflicts or compilation errors (Wave 1 tasks are in separate crates, so conflicts are unlikely).
3. **Wave 2** — Dispatch Task 4.
4. **Controller Integration** — Review `matches.rs`, verify `http.rs` proxy resolution still works correctly (security-adjacent), run full workspace build/test.
5. **Wave 3** — Dispatch Task 5, then Task 6 sequentially.
6. **Controller Integration** — Run final `cargo test --workspace`, `cargo clippy --workspace`.
7. **Final Integration Review** — Dispatch a quality-reviewer subagent over the entire diff to check for:
   - Missing `pub` visibility on new items.
   - Leftover `todo!()` outside of intentionally scoped stubs.
   - Integration between `matches.rs` and `http.rs` / `execute.rs`.
   - Whether new tests actually run and pass in CI.

## Acceptance Criteria

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace` is clean
- [ ] `spall-core/src/loader.rs` no longer mentions "Wave 1" for URL sources
- [ ] Typo in TOML config produces a clear deserialization error
- [ ] `spall-config` has unit tests for `sources.rs` and integration tests for `registry.rs` + `credentials.rs`
- [ ] `MergedMatches` lives in `spall-cli/src/matches.rs` and is consumed by both `execute.rs` and `http.rs`
- [ ] `--spall-chain` flag exists and `chain.rs` compiles
- [ ] REPL recognizes `|` but prints a graceful "not yet implemented" message

## Risks & Rollback

- `deny_unknown_fields` may break existing user configs with extra keys. This is intentional strictness. If a user reports breakage, the rollback is a single commit revert on `spall-config/src/sources.rs`.
- `MergedMatches` refactor touches proxy/config resolution. The `resolve_proxy` logic must preserve the exact priority chain. The spec reviewer for Task 4 must verify priority order line-by-line.
- `jmespath` crate version 0.5 API may differ slightly from the assumed API in Task 5. The subagent must compile-check the exact types (`jmespath::search`, `jmespath::Variable`) and adapt the code.

## Subagent Toolsets

| Task | Toolsets |
|------|----------|
| Task 1 (loader) | `['terminal', 'file']` |
| Task 2 (config strict) | `['terminal', 'file']` |
| Task 3 (config tests) | `['terminal', 'file']` |
| Task 4 (MergedMatches) | `['terminal', 'file']` |
| Task 5 (chain flag) | `['terminal', 'file']` |
| Task 6 (REPL pipe) | `['terminal', 'file']` |

## Review Instructions per Task

For each task in Waves 1-3, the controller will run:

1. **Spec Compliance Review**: dispatch a subagent with the original spec text and ask: "Did the implementer satisfy every bullet? List gaps."
2. **Code Quality Review**: dispatch a subagent with the changed files and ask: "Review for Rust idioms, error handling, test quality, and security. Verdict: APPROVED or REQUEST_CHANGES with specific fixes."

No task proceeds past a wave until both reviews are APPROVED.
