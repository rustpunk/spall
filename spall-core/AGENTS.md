# AGENTS.md

## Purpose

`spall-core` is the library crate for OpenAPI/Arazzo parsing, OpenAPI-to-IR resolution, validation helpers, YAML parsing, and cache serialization.

## Responsibilities

- Parse OpenAPI JSON/YAML bytes into `openapiv3::OpenAPI`.
- Resolve specs into `ResolvedSpec` and `SpecIndex`.
- Merge parameters, security, and server inheritance.
- Maintain postcard cache behavior and `IR_VERSION`.
- Provide the `spall_core::yaml` chokepoint and `SpallValue`.
- Model/evaluate supported Arazzo structures and expressions.

## Public APIs And Entry Points

- `loader::{load_spec, load_spec_from_bytes, load_raw}`
- `resolver::{resolve_spec, merge_parameters, resolve_security, resolve_schema}`
- `cache::{load_or_resolve, write_cache, load_cached_index, invalidate, IR_VERSION}`
- `validator::{validate_param, validate_body}`
- `yaml::{from_str, to_string}`
- `value::SpallValue`
- `arazzo::*`

## Internal Module Map

- `ir.rs`: cache-visible resolved IR.
- `loader.rs`: file/raw-byte loading; URL fetching is CLI-owned.
- `resolver.rs`: OpenAPI to IR.
- `cache.rs`: postcard cache and hashes.
- `yaml.rs`: serde-saphyr chokepoint and budgets.
- `validator.rs`: lightweight schema validation.
- `value.rs`: closed JSON-shaped values for IR.
- `extensions.rs`: `x-cli-*` extensions.
- `arazzo/`: model, expressions, source helpers.

## Dependency Rules

- Do not add `clap`, HTTP clients, async runtimes, `spall-cli`, or credential resolution.
- Do not call YAML parsers directly outside `yaml.rs`.
- Do not put `SecretString` or credentials into IR/cache types.

## Invariants

- IR cache uses postcard and SHA-256.
- Review/bump `IR_VERSION` when cache-visible IR layout changes.
- Use `SpallValue`, not `serde_json::Value`, in cache-visible IR fields.
- Parameter merge key is `(name, in)`; operation parameters override path parameters.
- Operation security replaces root security; explicit empty operation security means no auth.
- Server precedence is operation > path > spec > `/`.
- Preserve absent vs explicit empty Arazzo action chains.

## Common Mistakes

- Moving dynamic Clap command building back into core.
- Treating URL fetch as `loader.rs` responsibility.
- Bypassing `spall_core::yaml`.
- Simplifying recursive schema guards without resolver tests.

## Local Commands

- `cargo test -p spall-core`
- `cargo test -p spall-core --no-run`
- `cargo tree -p spall-core -e normal`

## Documentation Updates

Update `../doc/ai/10_ARCHITECTURE.md`, `../doc/ai/30_DESIGN_RULES.md`, and `../doc/ai/AI_CHANGELOG.md` for boundary, IR, cache, or resolver rule changes.

## Unclear / Ask Human

- Whether the "no unwrap in library crates" rule allows internal invariant unwraps.
- Whether `url = "2"` is still needed by this crate.

## Evidence

`src/lib.rs`, `src/ir.rs`, `src/resolver.rs`, `src/cache.rs`, `src/yaml.rs`, `src/value.rs`, `tests/cache_roundtrip_test.rs`, `tests/resolver_test.rs`, `tests/arazzo_parse_test.rs`.
