# Integration Plan: `hasp` into `spall`

## 1. Cargo.toml Changes

Only **`spall-cli/Cargo.toml`** needs new dependencies. `spall-config` and `spall-core` do not touch `hasp` directly — config stays plain-string (`token_url: Option<String>`), and resolution is a runtime CLI concern.

**`spall-cli/Cargo.toml`**

```toml
[dependencies]
# ... existing deps unchanged ...
hasp = { path = "../../hasp/crates/hasp", default-features = false, features = ["env", "keyring", "file"], optional = true }

[features]
# hasp becomes default-on so normal users get secret resolution out of the box.
# Downstream packagers can build --no-default-features to strip it.
default = ["hasp"]
hasp = ["dep:hasp"]
hasp-full = [
    "hasp",
    "hasp/aws-sm",
    "hasp/aws-ssm",
    "hasp/vault",
    "hasp/op",
    "hasp/bw",
    "hasp/gcp-sm",
    "hasp/azure-kv",
]
```

- `default-features = false` on the `hasp` dep prevents pulling in the `env` default implicitly; we list `env` explicitly for clarity.
- `optional = true` keeps the compile-time door open for `--no-default-features`.
- `hasp-full` is a convenience feature for users who need cloud/enterprise backends. All heavy SDKs are gated behind it.

No changes are required in `spall-config/Cargo.toml` or `spall-core/Cargo.toml`.

---

## 2. Feature Flag Strategy

**Recommendation: Keep `hasp` gated behind a Cargo feature in `spall-cli`, but add it to the crate's default features.**

| Consideration | Analysis |
|---------------|----------|
| Existing pattern | `spall-cli/src/auth/mod.rs` already contains `#[cfg(feature = "hasp")]`. Removing the gate would require deleting those annotations and would make the code busier. |
| Compile time | With `env`+`keyring`+`file` only, the impact is modest. `env` is zero-cost, `file` is tiny, and `keyring` is the only platform-specific crate. |
| Binary size | Cloud SDK bloat (`aws-sdk-*`, `google-cloud-*`, `azure-security-*`) is completely excluded by default. |
| Downstream / distro builds | A feature flag lets packagers (Debian, Nix, Alpine) build `--no-default-features` to achieve a minimal closure. |
| User expectations | Since `hasp` is in `default`, `cargo build` and `cargo install` just work. No manual `--features hasp` is needed. |

**Verdict:** `default = ["hasp"]` is the sweet spot. It upgrades the existing empty `hasp = []` feature into a real, default-enabled dependency while preserving the conditional-compile annotations.

---

## 3. Auth Resolution Wiring

**Location:** `spall-cli/src/auth/mod.rs`

Replace the stub:

```rust
#[cfg(feature = "hasp")]
if let Some(url) = &cfg.token_url {
    let _ = url;
}
```

with a real call:

```rust
#[cfg(feature = "hasp")]
if let Some(url) = &cfg.token_url {
    let secret = hasp::get(url).map_err(|e| map_hasp_error(api_name, e))?;
    return resolve_from_config_and_token(cfg, kind, secret.expose_secret());
}
```

**`SecretString` type compatibility:**  
Both `hasp-core` and `spall-cli` depend on `secrecy = "0.10"`. Cargo's resolver unifies these to the exact same crate instance. `hasp` re-exports `secrecy::SecretString` as `hasp::SecretString`, which is literally the same type as the `secrecy::SecretString` already imported in `auth/mod.rs`. **No conversion wrapper is required.** The value returned by `hasp::get(url)` can be passed directly into `ResolvedAuth` variants or exposed with `.expose_secret()`.

**Additional wiring for new fields (see Section 6):**

Inside the Basic-auth branch (priority 4), add a `password_url` lookup:

```rust
#[cfg(feature = "hasp")]
if kind == AuthKind::Basic {
    if let Some(url) = &cfg.password_url {
        let password = hasp::get(url).map_err(|e| map_hasp_error(api_name, e))?;
        if let Some(username) = &cfg.username {
            return Some(ResolvedAuth::Basic {
                username: username.clone(),
                password,
            });
        }
    }
}
```

And inside OAuth2 handling (future-proofing, even if OAuth2 flow is still a stub):

```rust
#[cfg(feature = "hasp")]
if kind == AuthKind::OAuth2 {
    if let Some(url) = &cfg.client_secret_url {
        let client_secret = hasp::get(url).map_err(|e| map_hasp_error(api_name, e))?;
        // Store in ResolvedAuth::OAuth2 or a future dedicated variant.
    }
}
```

---

## 4. Error Mapping

**Problem:** `hasp::get(url)` can fail with `hasp::Error` (NotFound, PermissionDenied, AuthenticationFailed, Backend { Transient, ... }, etc.). The current `auth::resolve` returns `Option<ResolvedAuth>`, which silently swallows failures. If a user explicitly configures `token_url`, a failure should be **surfaced**, not silently skipped.

**Plan:** Change `auth::resolve` to return `Result<Option<ResolvedAuth>, SpallCliError>`.

**Step A:** Add a new variant to `SpallCliError` in `spall-cli/src/main.rs`:

```rust
#[error("Auth resolution failed for '{api}': {message}")]
AuthResolution { api: String, message: String },
```

with exit code `EXIT_USAGE` (or a new dedicated code; `EXIT_USAGE` is acceptable for Wave 3).

**Step B:** In `spall-cli/src/auth/mod.rs`, add a small `#[cfg(feature = "hasp")]` helper:

```rust
#[cfg(feature = "hasp")]
fn map_hasp_error(api_name: &str, e: hasp::Error) -> crate::SpallCliError {
    crate::SpallCliError::AuthResolution {
        api: api_name.to_string(),
        message: e.to_string(),
    }
}
```

**Step C:** Update the signature and propagate:

```rust
pub fn resolve(
    api_name: &str,
    auth_config: Option<&AuthConfig>,
    cli_auth: Option<&str>,
) -> Result<Option<ResolvedAuth>, crate::SpallCliError> {
    // ...
}
```

**Step D:** Update call sites:

- `spall-cli/src/execute.rs`:
  ```rust
  let auth = crate::auth::resolve(&entry.name, entry.auth.as_ref(), cli_auth.as_deref())?;
  ```

- `spall-cli/src/commands/auth.rs`:
  ```rust
  let resolved = crate::auth::resolve(api_name, entry.auth.as_ref(), None)?;
  ```

Both callers already return `miette::Result` or `Result<_, SpallCliError>`, so the `?` operator composes cleanly.

---

## 5. Backend Availability in the Binary

| Backend | Default (`hasp` feature) | `hasp-full` | Rationale |
|---------|---------------------------|-------------|-----------|
| `env` | Yes | Yes | Zero-cost, universally useful for CI and local overrides. |
| `keyring` | Yes | Yes | Already referenced in spall legacy code; essential for developer workstations. |
| `file` | Yes | Yes | Needed for Docker/Kubernetes secret mounts (common in API CLI contexts). |
| `vault` | No | Yes | Enterprise use case; pulls in `reqwest`-based HTTP deps. Fine for `hasp-full`. |
| `op`, `bw` | No | Yes | Developer password managers; not CI-relevant. |
| `aws-sm`, `aws-ssm` | No | Yes | Heavy AWS SDK bloat; only enable when explicitly requested. |
| `gcp-sm`, `azure-kv` | No | Yes | Heavy cloud SDK bloat. |

**Rationale for `env`+`keyring`+`file` as default:**  
These three cover local development (keyring), CI/docker (env/file), and team secrets-sharing (file mount). They have negligible impact on compile time and binary size. Enterprise users who need HashiCorp Vault or cloud secret managers can build with `--features hasp-full`.

---

## 6. Config / TOML Changes

**`spall-config/src/auth.rs`** — extend `AuthConfig` with two minimal, backward-compatible optional fields:

```rust
/// URL-style secret reference for the password in Basic auth.
/// e.g. `keyring://spall/my-api-password`
pub password_url: Option<String>,

/// URL-style secret reference for the OAuth2 client secret.
/// e.g. `env://SPALL_CLIENT_SECRET`
pub client_secret_url: Option<String>,
```

Both are `Option<String>` with `serde(default)`, so existing `config.toml` files without these fields continue to deserialize correctly.

**`spall-config/src/sources.rs`** — no changes required. The legacy `keyring_service`+`keyring_user` mapping already produces `token_url`. That logic remains valid. There are no legacy fields for `password_url` or `client_secret_url`, so nothing to migrate.

**Semantic priority inside `auth::resolve` (already established in Section 3):**

For **Basic** auth:
1. `token_url` (generic, already implemented)
2. `password_url` (new, Basic-specific)
3. `password_env` (existing)

For **OAuth2**:
1. `client_secret_url` (new)
2. `client_secret` (existing inline, insecure)

This is additive only and preserves all existing behavior.

---

## 7. Testing Strategy

| Test | Type | Location | Notes |
|------|------|----------|-------|
| `env://` resolution | Integration | `spall-cli/tests/auth_hasp.rs` | Set `SPALL_TEST_TOKEN=abc`, configure `token_url = "env://SPALL_TEST_TOKEN"`, assert `ResolvedAuth::Bearer` with correct value. |
| `file://` resolution | Integration | `spall-cli/tests/auth_hasp.rs` | Create a `tempfile`, write a secret, resolve via `file://` path. |
| Error mapping | Unit | `spall-cli/src/auth/mod.rs` (`#[cfg(test)]`) | Mock `hasp::Error::NotFound` and `hasp::Error::Backend { Transient, .. }`, assert they map to `SpallCliError::AuthResolution` with sensible messages. |
| Keyring | Integration / Ignored | `spall-cli/tests/auth_hasp.rs` | `#[ignore = "requires OS keyring"]` or gate with `#[cfg(target_os = "macos")]`. Do not run in CI. |
| `password_url` for Basic | Integration | `spall-cli/tests/auth_hasp.rs` | Verify `env://` password resolves into `ResolvedAuth::Basic`. |

**CI strategy:**  
Only `env://` and `file://` tests run in CI. Keyring tests are skipped. No cloud SDK credentials are required in the test suite.

---

## 8. Implementation Order

1. **Update `spall-cli/Cargo.toml`**  
   - Add `hasp` path dependency (`default-features = false`, `features = ["env", "keyring", "file"]`, `optional = true`).  
   - Redefine features: `default = ["hasp"]`, `hasp = ["dep:hasp"]`, `hasp-full = [ ... ]`.

2. **Extend `AuthConfig` in `spall-config/src/auth.rs`**  
   - Add `password_url: Option<String>` and `client_secret_url: Option<String>`.

3. **Add `AuthResolution` error variant to `SpallCliError`** in `spall-cli/src/main.rs`.

4. **Change `auth::resolve` signature** in `spall-cli/src/auth/mod.rs` from `Option<ResolvedAuth>` to `Result<Option<ResolvedAuth>, SpallCliError>`.

5. **Implement real hasp resolution** in `spall-cli/src/auth/mod.rs`:  
   - Replace the `token_url` stub with `hasp::get(url)`.  
   - Add `password_url` resolution inside the Basic branch.  
   - Add `client_secret_url` resolution inside the OAuth2 branch.  
   - Add `map_hasp_error` helper.

6. **Update callers** to handle the new `Result` return type:  
   - `spall-cli/src/execute.rs`  
   - `spall-cli/src/commands/auth.rs`

7. **Add unit tests** for `hasp::Error` -> `SpallCliError` mapping in `auth/mod.rs`.

8. **Add integration tests** in `spall-cli/tests/auth_hasp.rs` for `env://` and `file://` backends.

9. **Verify backward compatibility** by deserializing an old `config.toml` that lacks `password_url` / `client_secret_url`.

10. **Update inline comments / doc strings** in `auth/mod.rs` to remove `TODO(hasp)` stubs.

---

## Critical Files for Implementation

- `/home/glitch/code/rustpunk/spall/spall-cli/Cargo.toml` — Dependency declaration, feature gates (`hasp`, `hasp-full`), and backend selection.
- `/home/glitch/code/rustpunk/spall/spall-config/src/auth.rs` — `AuthConfig` struct extensions (`password_url`, `client_secret_url`).
- `/home/glitch/code/rustpunk/spall/spall-cli/src/auth/mod.rs` — Core hasp integration: `hasp::get(url)` calls, `SecretString` consumption, error mapping helper, and `resolve` signature change.
- `/home/glitch/code/rustpunk/spall/spall-cli/src/main.rs` — New `SpallCliError::AuthResolution` variant needed by the auth resolver.
- `/home/glitch/code/rustpunk/spall/spall-cli/src/execute.rs` — Caller site that must propagate the new `Result` from `auth::resolve`.
