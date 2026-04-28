# spall — Project Context

**spall** is a dynamic OpenAPI 3.x CLI. Name etymology: a fragment that breaks free from a corroding metal surface and flies. Tagline: "Break free. Hit the endpoint."

## Architecture

Three-crate workspace:
- `spall-core`: Spec loading, `$ref` resolution, IR, dynamic clap `Command` builder.
- `spall-config`: Config parsing, credential stack, `ApiRegistry`.
- `spall-cli`: Binary. Two-phase parse (Phase 1: registry scan; Phase 2: spec load + re-parse).

**Phase 1 API stubs must `.disable_help_flag(true)` so `--help` falls through to Phase 2.**

## Tech Stack

- `openapiv3` for spec deserialization (behavioral logic is ours).
- `clap` builder API (never derive).
- `serde_saphyr = "0.0.24"` (NOT `serde_yaml`). All YAML goes through `spall_core::yaml` chokepoint.
- `reqwest` with `default-features = false` + `rustls-tls` + `multipart`.
- `tokio` current_thread (`rt`, `macros`, `net`, `time`, `io-util`).
- `miette` + `thiserror`.
- `secrecy` for credentials.
- `postcard` + `sha2` for IR cache.

## Critical Conventions

- All internal CLI flags use `--spall-*` prefix.
- Exit codes: 0=ok, 1=usage, 2=network, 3=spec, 4=4xx, 5=5xx.
- No `.unwrap()` in library crates. `miette` Result in CLI crate only.
- All IR types derive `Serialize`/`Deserialize`; `SecretString` NEVER in IR.
- Credentials always wrapped in `SecretString` — never raw strings.
- `IndexMap` over `HashMap` where iteration order matters.
- Arg ID namespacing: `path-{n}`, `query-{n}`, `header-{n}`, `cookie-{n}`.
- Error boundary: `thiserror` in libraries, `miette` in CLI, `#[diagnostic(transparent)]` for passthrough.
- Never panic on user input.
- Rust 2021 edition. `#[must_use]` on Result-returning functions.

## Build/Test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace
cargo doc --workspace --no-deps
```

## Wave Structure

- Wave 1: Core request flow (MVP). ✅
- Wave 1.5: IR cache, performance, reliability. ✅
- Wave 2: QoL (validation, profiles, pagination, completions, history, output formats, filtering). ✅
- Wave 3 Independent: Auth providers, repeat, spec autodiscovery. ✅
- Wave 3: REPL, chaining. ⏳
- Wave 4: Daemon mode, plugins, mock server, OpenAPI 3.1. ⏳

**Current: Wave 3.** Don't implement beyond Wave 3 without explicit instruction.

## openapiv3 Crate Limitations

Only deserialization. `$ref` resolution, parameter merging, security inheritance, cycle detection, lenient parsing — all manual. We evaluated `openapiv3-extended` (v6) and chose `openapiv3` v2; re-evaluate before Wave 4 if resolver edge cases become painful.

## Key Patterns

- Parameter merge by `(name, in)` tuple.
- Security: operation replaces root, empty array = no auth.
- Cycle detection via visited HashSet + depth limit.
- Lenient parsing: missing types = any, force path params required, disambiguate duplicate operationIds.
- Cache writes: temp file + atomic `fs::rename`.
- JSON fallback if `serde_saphyr` fails on YAML.
- Graceful `--help` degradation: if Phase 2 spec load fails but `--help` was requested, attempt to load cached `SpecIndex`.
- YAML is parsed through `spall_core::yaml::from_str` — the single chokepoint with hard DoS budgets.
