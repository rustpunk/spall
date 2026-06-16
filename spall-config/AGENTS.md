# AGENTS.md

## Purpose

`spall-config` owns configuration parsing, API registry construction, profile overlays, auth config types, and credential classification.

## Responsibilities

- Load global config and defaults from XDG config dir.
- Scan per-API TOML files and configured spec directories.
- Build `ApiRegistry` without parsing OpenAPI specs.
- Persist/remove per-API config files for API management.
- Represent auth config and resolved auth material using `SecretString`.

## Public APIs And Entry Points

- `registry::{ApiRegistry, ApiEntry, ProfileConfig}`
- `ApiRegistry::{load, from_entries, add_api, remove_api, find, resolve_profile}`
- `sources::{config_dir, expand_tilde, load_global_config, scan_api_configs, scan_spec_dirs}`
- `auth::{AuthConfig, AuthKind, ApiKeyLocation, ResolvedAuth, default_token_env}`
- `credentials::{CredentialKind, classify_bare_token}`

## Internal Module Map

- `auth.rs`: TOML-facing auth structs and secret serde adapters.
- `credentials.rs`: bare-token classification.
- `error.rs`: config errors.
- `registry.rs`: registry load/merge/add/remove/profile logic.
- `sources.rs`: XDG paths, TOML schemas, API/spec-dir scans.

## Dependency Rules

- Do not parse OpenAPI specs in this crate.
- Do not perform concrete secret backend resolution here unless architecture changes.
- Keep config parsing lightweight and deterministic where behavior depends on priority.

## Invariants

- Inline secret fields deserialize into `SecretString` but serialize as redacted/empty by design.
- TOML-facing config structs use `deny_unknown_fields`.
- Bare tokens become Basic only for unambiguous `user:pass`; ambiguous values remain Bearer.
- `default_token_env("foo-bar")` becomes `SPALL_FOO_BAR_TOKEN`.
- Spec-dir scan supports `.json`, `.yaml`, and `.yml`.
- Legacy keyring fields may map to `token_url` when no `token_url` is present.

## Common Mistakes

- Expecting inline secrets to round-trip through serialization.
- Inferring API-key auth from bare tokens.
- Adding spec parsing to registry load.
- Treating old design docs as describing current source priority without checking tests/source.

## Local Commands

- `cargo test -p spall-config`
- `cargo test -p spall-config --tests`

## Documentation Updates

Update `../doc/ai/80_OPEN_QUESTIONS.md` if config source priority is resolved. Update user docs under `../docs/src/config/` for config behavior changes.

## Approval Gates

- Current config source priority should be clarified and tested before changing.
- Whether profile header overlay order should be deterministic.

## Evidence

`src/auth.rs`, `src/credentials.rs`, `src/registry.rs`, `src/sources.rs`, `tests/auth_serde_test.rs`, `tests/credentials_test.rs`, `tests/registry_test.rs`, `tests/sources_test.rs`, `../examples/config.toml`.
