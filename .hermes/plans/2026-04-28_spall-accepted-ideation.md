# Plan: Spall Accepted Ideation Batch (2026-04-28)

> **For Hermes:** Use `subagent-driven-development` skill to execute this plan task-by-task.

**Goal:** Implement the 5 accepted ideas from `ideate-2026-04-28-openapi-cli-spall.md` in dependency order: trivial fixes first, then foundational work, then the large Wave 3 feature.

**Architecture:** Each task stands alone. Phase A (trivial) and Phase B (tests) can be done in parallel. Phase C (refactoring) benefits from Phase B being in place. Phase D (chaining) is a new module that reuses existing `--spall-filter` and response-parsing patterns.

**Tech Stack:** Rust 2021, `clap` builder API, `serde`, `thiserror`, `miette`, `tokio` current_thread, SQLite (`rusqlite`), `jmespath`, `postcard`.

---

## Phase A — Trivial Fixes (XS effort)

### Task A1: Remove stale Wave 1 comment and fix `load_raw` error message

**Objective:** Fix `spall-core/src/loader.rs` so it no longer claims URLs are unimplemented in Wave 1.

**Files:**
- Modify: `spall-core/src/loader.rs:46-50`

**Step 1: Edit `load_raw`**

```rust
/// Load raw spec bytes from a source.
///
/// Wave 1: local file system only. Wave 1.5: URL fetching with caching.
pub fn load_raw(source: &str) -> Result<Vec<u8>, SpallCoreError> {
    if source.starts_with("http://") || source.starts_with("https://") {
        return Err(SpallCoreError::InvalidSource(
            format!("URL sources require the CLI fetch layer (spall_cli::fetch), not spall_core::loader: {}", source),
        ));
    }

    let path = std::path::PathBuf::from(source);
    std::fs::read(&path).map_err(|e| SpallCoreError::Io(e.to_string()))
}
```

**Step 2: Update doc comment on `load_spec`**

Change the doc comment at line 5-8 to:
```rust
/// Load and resolve an OpenAPI spec from a file path.
///
/// For URL sources, the CLI layer (`spall_cli::fetch`) handles HTTP fetching
/// and passes the resolved bytes to `load_spec_from_bytes` directly.
```

**Step 3: Verify**

Run: `cargo build --workspace`
Expected: Compiles without errors.

**Step 4: Commit**

```bash
git add spall-core/src/loader.rs
git commit -m "fix(loader): remove stale Wave 1 URL rejection message"
```

---

### Task A2: Add `#[serde(deny_unknown_fields)]` to config TOML structs

**Objective:** Prevent silent misconfiguration from typos in `~/.config/spall/apis/*.toml`.

**Files:**
- Modify: `spall-config/src/sources.rs` (find `SpallConfig`, `ApiToml`, `ProfileToml`, `AuthConfig`, `GlobalDefaults` structs)

**Step 1: Add attribute to structs**

Add `#[serde(deny_unknown_fields)]` to every struct in `sources.rs` that is deserialized from user config files:

```rust
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SpallConfig { ... }

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ApiToml { ... }

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ProfileToml { ... }

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig { ... }

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct GlobalDefaults { ... }
```

**Step 2: Verify**

Run: `cargo build --workspace`
Expected: Compiles. If clippy warns about `deny_unknown_fields` on structs with flatten, verify each struct doesn't use `#[serde(flatten)]` on fields; if it does, remove `deny_unknown_fields` from that specific struct and add it to the flattened sub-struct instead.

**Step 3: Test manually**

Create a temp TOML file with a typo:
```toml
name = "test"
spec = "/tmp/fake.json"
baseurl = "https://example.com"  # typo: should be base_url
```

Write a quick integration test in `spall-config/tests/config_test.rs` (create the file):
```rust
use spall_config::sources::{load_config, ConfigError};

#[test]
fn reject_unknown_fields() {
    let input = r#"
name = "test"
spec = "/tmp/fake.json"
baseurl = "https://example.com"
"#;
    let result: Result<_, _> = toml::from_str(input);
    assert!(result.is_err(), "expected error for unknown field 'baseurl'");
}
```

Run: `cargo test -p spall-config --test config_test`
Expected: Test passes.

**Step 4: Commit**

```bash
git add spall-config/src/sources.rs spall-config/tests/config_test.rs
git commit -m "feat(config): deny unknown fields in TOML config"
```

---

## Phase B — `spall-config` Tests (M effort)

### Task B1: Add tests for registry parsing and resolution

**Objective:** Cover `registry.rs` with unit/integration tests for API registration, profile overlay, and source priority.

**Files:**
- Create: `spall-config/tests/registry_test.rs`

**Step 1: Write failing test for `ApiRegistry::load`**

```rust
use std::io::Write;
use tempfile::TempDir;

#[test]
fn registry_loads_single_api() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("config.toml");
    let mut file = std::fs::File::create(&config_path).unwrap();
    writeln!(file, r#"[[api]]
name = "petstore"
spec = "/tmp/petstore.json"
"#).unwrap();

    let registry = spall_config::registry::ApiRegistry::load_from_path(&config_path).unwrap();
    assert_eq!(registry.apis.len(), 1);
    assert_eq!(registry.apis[0].name, "petstore");
}
```

**Step 2: Run failing test**

Run: `cargo test -p spall-config --test registry_test`
Expected: FAIL — `load_from_path` may not exist yet (the current API is `ApiRegistry::load()`). If so, expose a `load_from_path` constructor in `registry.rs` or adjust the test to simulate the user's home directory.

**Step 3: Add testable constructor or mock path**

In `spall-config/src/registry.rs`, add:
```rust
impl ApiRegistry {
    pub fn load_from_path(path: &std::path::Path) -> Result<Self, SpallConfigError> {
        let config = crate::sources::load_config(path)?;
        Self::from_config(config)
    }
    // ... keep existing load()
}
```

**Step 4: Verify pass**

Run: `cargo test -p spall-config --test registry_test`
Expected: PASS.

**Step 5: Commit**

```bash
git add spall-config/src/registry.rs spall-config/tests/registry_test.rs
git commit -m "test(config): add registry loading tests"
```

---

### Task B2: Add tests for credential resolution

**Objective:** Cover `credentials.rs` and the auth resolution chain (env var, config, fallback).

**Files:**
- Create: `spall-config/tests/credentials_test.rs`

**Step 1: Write test for env var resolution**

```rust
use secrecy::ExposeSecret;

#[test]
fn resolve_from_env_var() {
    std::env::set_var("SPALL_TEST_API_TOKEN", "secret123");
    let result = spall_config::credentials::resolve_env("test-api");
    assert_eq!(result.unwrap().expose_secret(), "secret123");
    std::env::remove_var("SPALL_TEST_API_TOKEN");
}
```

**Step 2: Write test for missing fallback**

```rust
#[test]
fn resolve_missing_returns_none() {
    let result = spall_config::credentials::resolve_env("nonexistent-api");
    assert!(result.is_none());
}
```

**Step 3: Verify**

Run: `cargo test -p spall-config --test credentials_test`
Expected: PASS if the public API is `resolve_env`; otherwise adjust to match existing exported functions.

**Step 4: Commit**

```bash
git add spall-config/tests/credentials_test.rs
git commit -m "test(config): add credential resolution tests"
```

---

### Task B3: Add tests for `sources.rs` TOML parsing

**Objective:** Cover `sources.rs` — especially profile defaults, `spec_dirs` expansion, and edge cases.

**Files:**
- Create: `spall-config/tests/sources_test.rs`

**Step 1: Write test for default values**

```rust
#[test]
fn defaults_are_populated() {
    let input = r#"
[defaults]
output = "json"
color = "auto"
"#;
    let cfg: spall_config::sources::SpallConfig = toml::from_str(input).unwrap();
    assert_eq!(cfg.defaults.output, Some("json".to_string()));
    assert_eq!(cfg.defaults.color, Some("auto".to_string()));
}
```

**Step 2: Write test for empty config**

```rust
#[test]
fn empty_config_is_valid() {
    let input = "";
    let cfg: spall_config::sources::SpallConfig = toml::from_str(input).unwrap();
    assert!(cfg.api.is_empty());
}
```

**Step 3: Verify**

Run: `cargo test -p spall-config --test sources_test`
Expected: PASS.

**Step 4: Commit**

```bash
git add spall-config/tests/sources_test.rs
git commit -m "test(config): add sources TOML parsing tests"
```

---

## Phase C — MergedMatches Refactoring (S effort)

### Task C1: Extract `MergedMatches` to a shared module

**Objective:** Centralize Phase1/Phase2 flag lookup so `http.rs` and `execute.rs` both consume it.

**Files:**
- Create: `spall-cli/src/matches.rs`
- Modify: `spall-cli/src/execute.rs` (remove inline `MergedMatches`)
- Modify: `spall-cli/src/http.rs` (replace manual fallback closures)
- Modify: `spall-cli/src/main.rs` (add `mod matches;`)

**Step 1: Create `spall-cli/src/matches.rs`**

```rust
use clap::ArgMatches;

/// Unified view over Phase 1 and Phase 2 clap matches.
/// Prefers Phase 2 values, falling back to Phase 1.
#[derive(Debug, Clone, Copy)]
pub struct MergedMatches<'a> {
    pub phase1: &'a ArgMatches,
    pub phase2: &'a ArgMatches,
}

impl MergedMatches<'_> {
    pub fn get_flag(&self, id: &str) -> bool {
        self.phase2.get_flag(id) || self.phase1.get_flag(id)
    }

    pub fn get_one<T: Clone + Send + Sync + 'static>(&self, id: &str,
    ) -> Option<T> {
        self.phase2
            .get_one::<T>(id)
            .cloned()
            .or_else(|| self.phase1.get_one::<T>(id).cloned())
    }

    pub fn get_many<T: Clone + Send + Sync + 'static>(
        &self,
        id: &str,
    ) -> Option<clap::parser::ValuesRef<'_<, T>> {
        self.phase2
            .get_many::<T>(id)
            .or_else(|| self.phase1.get_many::<T>(id))
    }
}
```

**Step 2: Replace inline definition in `execute.rs`**

Delete the `struct MergedMatches` and its `impl` block at lines ~413–437 in `execute.rs`. Replace with:
```rust
use crate::matches::MergedMatches;
```

Keep the `merge_matches` helper function since it constructs the struct.

**Step 3: Rewrite `config_from_matches` in `http.rs`**

Replace the manual `let get_timeout = || ...` pattern with:
```rust
use crate::matches::MergedMatches;

pub fn config_from_matches(p1: &ArgMatches, p2: &ArgMatches) -> HttpConfig {
    let m = MergedMatches { phase1: p1, phase2: p2 };
    let mut cfg = HttpConfig::default();

    if let Some(timeout) = m.get_one::<u64>("spall-timeout") {
        cfg.timeout = std::time::Duration::from_secs(timeout);
    }

    if let Some(retry) = m.get_one::<u8>("spall-retry") {
        cfg.retry = retry;
    }

    cfg.follow_redirects = m.get_flag("spall-follow");

    if let Some(max) = m.get_one::<usize>("spall-max-redirects") {
        cfg.max_redirects = max;
    }

    cfg.insecure = m.get_flag("spall-insecure");

    if let Some(cert) = m.get_one::<String>("spall-ca-cert") {
        cfg.ca_cert = Some(cert);
    }

    cfg.no_proxy = m.get_flag("spall-no-proxy");

    if let Some(proxy) = m.get_one::<String>("spall-proxy") {
        cfg.proxy = Some(proxy);
    }

    if let Some(server) = m.get_one::<String>("spall-server") {
        cfg.base_url_override = Some(server);
    }

    if let Some(auth) = m.get_one::<String>("spall-auth") {
        cfg.auth_header = Some(auth);
    }

    if let Some(headers) = m.get_many::<String>("spall-header") {
        for h in headers {
            if let Some((k, v)) = h.split_once(':') {
                cfg.custom_headers
                    .push((k.trim().to_string(), v.trim().to_string()));
            }
        }
    }

    cfg
}
```

**Step 4: Rewrite `resolve_proxy` in `http.rs`**

Replace the manual Phase 1 / Phase 2 lookups in `resolve_proxy` with `MergedMatches`:
```rust
pub fn resolve_proxy(
    entry: &spall_config::registry::ApiEntry,
    global_defaults: &spall_config::sources::GlobalDefaults,
    p1: &clap::ArgMatches,
    p2: &clap::ArgMatches,
) -> Option<String> {
    let m = MergedMatches { phase1: p1, phase2: p2 };

    if m.get_flag("spall-no-proxy") {
        return None;
    }

    if let Some(proxy) = m.get_one::<String>("spall-proxy") {
        return Some(proxy);
    }

    if entry.proxy.is_some() {
        return entry.proxy.clone();
    }

    if let Some(proxy) = env_proxy() {
        return Some(proxy);
    }

    if global_defaults.proxy.is_some() {
        return global_defaults.proxy.clone();
    }

    None
}
```

**Step 5: Verify**

Run: `cargo build --workspace`
Expected: Compiles without errors.

Run: `cargo test --workspace`
Expected: All existing tests pass.

**Step 6: Commit**

```bash
git add spall-cli/src/matches.rs spall-cli/src/execute.rs spall-cli/src/http.rs spall-cli/src/main.rs
git commit -m "refactor(cli): extract MergedMatches for Phase1/Phase2 flag lookup"
```

---

## Phase D — Request Chaining (L effort)

### Task D1: Design the chaining expression grammar and CLI surface

**Objective:** Define how users express capture-and-feed semantics.

**Files:**
- Modify: `spall-cli/src/main.rs` (add `--spall-chain` global arg)
- Modify: `spall-design.md` (add a new subsection under Wave 3 if desired)

**Decision (in-plan, override if you disagree):**

Use `--spall-chain "operationId --param $(chain.jmespath)"` as the first-pass syntax. Keep it simple:
- `--spall-chain <expression>` is a global flag that appears on `main.rs`.
- The expression is evaluated after the response body is received.
- If the expression starts with a bare operation ID, treat it as "after this request, run that operation with the captured value".

Alternatively, for REPL-only chaining, implement a pipe operator `|` inside the REPL.

**Step 1: Add `--spall-chain` global arg**

In `spall-cli/src/main.rs`, inside `spall_global_args()` (or wherever global args are defined; find the function by searching for it), add:
```rust
Arg::new("spall-chain")
    .long("spall-chain")
    .value_name("EXPR")
    .help("After this request, run another operation with captured values (e.g., 'op2 --id $.data.id')"),
```

**Step 2: Commit**

```bash
git add spall-cli/src/main.rs
git commit -m "feat(cli): add --spall-chain flag surface"
```

---

### Task D2: Create `spall-cli/src/chain.rs` module

**Objective:** Implement the capture-evaluate-and-dispatch logic.

**Files:**
- Create: `spall-cli/src/chain.rs`
- Modify: `spall-cli/src/main.rs` (add `mod chain;`)

**Step 1: Create the module**

```rust
use serde_json::Value;
use spall_core::ir::ResolvedOperation;

/// Parsed chain expression.
#[derive(Debug, Clone)]
pub struct ChainExpr {
    pub target_op_id: String,
    /// (param_id, jmespath_expression) pairs
    pub bindings: Vec<(String, String)>,
}

impl ChainExpr {
    /// Parse a simple expression like `op2 --id $.id --name $.name`
    pub fn parse(expr: &str) -> Result<Self, crate::SpallCliError> {
        let mut parts = expr.split_whitespace();
        let target_op_id = parts
            .next()
            .ok_or_else(|| crate::SpallCliError::Usage("chain expression requires target operation".to_string()))?
            .to_string();

        let mut bindings = Vec::new();
        while let Some(token) = parts.next() {
            if token.starts_with("--") {
                let param_id = token.trim_start_matches("--").to_string();
                let jmespath = parts
                    .next()
                    .ok_or_else(|| crate::SpallCliError::Usage(format!("chain param '{}' needs a jmespath expression", param_id)))?;
                bindings.push((param_id, jmespath.to_string()));
            }
        }

        Ok(ChainExpr { target_op_id, bindings })
    }

    /// Evaluate JMESPath expressions against the response JSON and return resolved CLI args.
    pub fn resolve(
        &self,
        response_json: &Value,
    ) -> Result<Vec<String>, crate::SpallCliError> {
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
                        "chain jmespath error for '{}': {}",
                        expr, e
                    )));
                }
            }
        }
        Ok(args)
    }
}
```

**Step 2: Add module declaration**

Add `mod chain;` to `spall-cli/src/main.rs`.

**Step 3: Verify**

Run: `cargo build --workspace`
Expected: Compiles.

**Step 4: Commit**

```bash
git add spall-cli/src/chain.rs spall-cli/src/main.rs
git commit -m "feat(cli): add chain expression parser and resolver"
```

---

### Task D3: Wire chaining into the REPL

**Objective:** Enable `api get-user | api get-repo --owner $id` or similar syntax inside the REPL.

**Files:**
- Modify: `spall-cli/src/repl.rs`

**Step 1: Add pipe parsing to REPL input**

Inside the REPL loop (after trimming), detect `|`:
```rust
if trimmed.contains("|") {
    let stages: Vec<&str> = trimmed.split("|").map(|s| s.trim()).collect();
    // Execute first stage normally, capture JSON response, feed into subsequent stages.
    // ... implementation omitted for brevity, but use the chain module.
} else {
    // existing dispatch logic
}
```

**Step 2: Add `run_piped` helper**

```rust
async fn run_piped(
    stages: &[Vec<String>],
    registry: &spall_config::registry::ApiRegistry,
    cache_dir: &std::path::Path,
) -> Result<(), crate::SpallCliError> {
    // Execute first stage, collect JSON response
    // For each subsequent stage, parse chain expression and substitute
    // Dispatch via crate::run_with_args
    todo!("implement piped execution")
}
```

**Step 3: Commit**

```bash
git add spall-cli/src/repl.rs
git commit -m "feat(repl): add pipe syntax scaffolding for request chaining"
```

---

## Acceptance Criteria

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace` is clean (or only has pre-existing warnings)
- [ ] `cargo build --workspace` succeeds
- [ ] `spall-core/src/loader.rs` no longer mentions "Wave 1" for URL handling
- [ ] Typo in `~/.config/spall/apis/*.toml` produces a clear error message
- [ ] `spall-config` has unit tests for registry, credentials, and sources
- [ ] `MergedMatches` is used in both `execute.rs` and `http.rs`
- [ ] `--spall-chain` flag exists and `chain.rs` module compiles

## Risks & Rollback

- `deny_unknown_fields` may break configs that currently work due to extra keys (e.g., comments or copy-pasted fields). This is acceptable — strictness is the goal.
- `MergedMatches` refactor touches `http.rs` proxy/config resolution which is security-adjacent. Review diffs carefully.
- Chaining is large and may be split into a separate plan if scope grows.

## Handoff

Phase A, B, and C are independent and can be parallelized across subagents. Phase D depends on Phase C (understanding of `run_with_args` dispatch) but not on A or B.

Plan saved to `.hermes/plans/`.
