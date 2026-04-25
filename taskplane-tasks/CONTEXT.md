# Task Area Context: General

This task area covers all work on the `spall` CLI — a Rust-native OpenAPI 3.x CLI that parses specs at runtime and generates clap-based command trees for API operations.

## Workspace Layout

```
spall/
├── Cargo.toml              # Workspace manifest
├── spall-core/             # IR types, spec loading, $ref resolution, command builder
│   ├── src/cache.rs
│   ├── src/command.rs
│   ├── src/error.rs
│   ├── src/ir.rs
│   ├── src/lib.rs
│   ├── src/loader.rs
│   └── src/resolver.rs
├── spall-config/           # API registry, config scanning, credential resolver
│   ├── src/credentials.rs
│   ├── src/error.rs
│   ├── src/lib.rs
│   ├── src/registry.rs
│   └── src/sources.rs
├── spall-cli/              # Two-phase clap parse, HTTP execution, output formatting
│   ├── src/commands/api.rs
│   ├── src/commands/mod.rs
│   ├── src/completions.rs
│   ├── src/execute.rs
│   ├── src/http.rs
│   ├── src/main.rs
│   └── src/output.rs
└── CLAUDE.md               # Architecture design decisions
```

## Key Dependencies & Versions
- `clap 4` — Two-phase command tree construction
- `reqwest 0.13` — HTTP client; features: `json`, `rustls`, `multipart`, `cookies`, `socks`, `query`
- `openapiv3 2` — OpenAPI 3.0 parse (NOT `openapiv3-extended`)
- `serde-saphyr 0.0.24` — YAML deserialization
- `postcard`, `sha2` — IR cache serialization
- `miette 7`, `thiserror 2` — Error handling
- `secrecy 0.10` — Token redaction in debug/logging

## Conventions
- Rust 2021 edition, workspace
- All internal flags use `--spall-*` prefix
- `#[must_use]` on Result-returning functions (but NOT on functions whose return type already has `#[must_use]` — clippy `double_must_use`)
- Use `cargo clippy --workspace` before declaring done; fix or suppress warnings with intent
- Prefer `.strip_prefix()` over `starts_with` + manual slicing
- For clap arg access: check `contains_id` or `is_some` before accessing optional args on subcommands — clap panics on undefined IDs

## Architecture Reminders
- **Two-phase parse**: Phase 1 stubs (`.allow_external_subcommands(true)`, `.disable_help_flag(true)`). Phase 2 rebuilds full operation tree after spec load. API name must be prepended as `argv[0]` before Phase 2 `try_get_matches_from`.
- **Single-tag flattening**: `build_operations_cmd` registers ops directly under root when `groups.len() == 1`.
- **MergedMatches**: `get_flag` uses Phase 2 OR Phase 1; `get_one`/`get_many` prefer Phase 2 then fallback to Phase 1.
- **Error handling**: `SpallCliError` derives `miette::Diagnostic`; config/spec/network errors are wrapped via explicit `map_err` at `?` sites because `SpallConfigError` and `clap::Error` do not implement `miette::Diagnostic`.
