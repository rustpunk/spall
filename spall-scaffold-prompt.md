# Claude Code Prompt: Scaffold `spall` Workspace

Paste everything below this line into Claude Code.

---

Scaffold a new Rust workspace called `spall` — a dynamic CLI tool that parses OpenAPI 3.x specs at runtime and generates CLI commands for making API requests. Think "Restish, but Rust."

## What to produce

A `cargo build`-able workspace with three crates, a root CLAUDE.md, and just enough real code to prove the architecture compiles. This is a foundation to iterate on — not a working MVP yet.

### Workspace structure

```
spall/
├── Cargo.toml              # Workspace manifest
├── CLAUDE.md               # Project context for future Claude Code sessions
├── README.md               # Project overview with name etymology
├── examples/
│   ├── config.toml         # Example global config showing all spec source methods
│   └── apis/
│       └── petstore.toml   # Example per-API config with auth, headers, base_url
├── spall-core/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── loader.rs       # Spec loading (file path, URL w/ caching)
│       ├── resolver.rs     # $ref resolution, cycle detection, parameter merging
│       ├── command.rs       # Resolved IR → clap Command tree builder
│       ├── cache.rs         # Postcard IR cache with hash-based invalidation
│       ├── ir.rs            # All resolved IR types (SpecIndex, ResolvedOperation, etc.)
│       └── error.rs         # SpallCoreError via thiserror
├── spall-config/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── registry.rs     # ApiRegistry — lightweight index of registered APIs
│       ├── sources.rs       # Config parsing: config.toml, apis/*.toml, spec_dirs scan
│       ├── credentials.rs   # Credential resolution stack (env > keyring > config)
│       └── error.rs         # SpallConfigError via thiserror
├── spall-cli/
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs          # Two-phase clap parse, dispatch
│       ├── http.rs           # Centralized ClientBuilder
│       ├── output.rs         # Response formatting, TTY detection, output modes
│       ├── execute.rs        # Request building, sending, response handling
│       ├── completions.rs    # Shell completion stubs (Wave 2)
│       └── commands/
│           ├── mod.rs
│           └── api.rs        # spall api add|list|remove|refresh
```

### Crate dependencies assignment

**spall-core** depends on: `openapiv3`, `clap` (builder API only), `serde`, `serde_json`, `serde_saphyr = "0.0.24"`, `postcard`, `sha2`, `indexmap`, `thiserror`, `url = "2"`.

**spall-config** depends on: `serde`, `toml`, `dirs`, `secrecy`, `thiserror`.

**spall-cli** depends on: `spall-core`, `spall-config`, `clap`, `tokio` (features: `rt`, `macros`, `net`, `time`, `io-util`), `reqwest` (default-features = false, features: `json`, `rustls-tls`, `rustls-native-certs`, `multipart`, `cookies`), `miette` (fancy feature), `is-terminal`, `secrecy`.

### Crate responsibilities

**spall-core**: OpenAPI spec loading (file/URL), `$ref` resolution into a compact IR, cycle detection with depth limits (8–10), path-level + operation-level parameter merging by `(name, in)` tuple, security scheme inheritance logic (operation replaces root, empty array = no auth), pre-compiled IR caching (postcard serialization with SHA-256 hash invalidation), and the dynamic clap `Command` builder that turns resolved operations into subcommands.

Key types in `ir.rs`:
- `SpecIndex` (lightweight, loaded at startup for routing)
- `ResolvedOperation` (full operation IR)
- `ResolvedParameter` (includes `style`, `explode`, `required`, `deprecated`, `schema`)
- `ResolvedRequestBody` (includes `content: IndexMap<String, ResolvedMediaType>`)
- `ResolvedResponse` (includes `content: IndexMap<String, ResolvedMediaType>`, `headers`)

**ALL IR types must derive `serde::Serialize` and `serde::Deserialize` for postcard caching. `secrecy::SecretString` must NEVER appear in any IR type.** Credential types live in `spall-config` only.

The command builder must:
- Generate unique internal Arg IDs to avoid collision when path/query/header params share a name. Use `format!("path-{}", name)`, `format!("query-{}", name)`, `format!("header-{}", name)`, `format!("cookie-{}", name)`.
- Preserve user-facing names via `.long(name)` for query/header/cookie params and `.value_name(name)` for positional path params.
- Map OpenAPI `schema.default` → `Arg::default_value()` and `schema.enum_values` → `Arg::value_parser([...])` when building query/header/cookie args.
- Respect request body `required` field: if required, mark `--data` as `.required(true)`. If optional, also register a `--no-data` flag so the user can explicitly omit the body.

Depends on: `openapiv3`, `clap`, `serde`, `serde_json`, `serde_saphyr`, `postcard`, `sha2`, `indexmap`, `thiserror`, `url`.

**spall-config**: Config file parsing (`config.toml`, per-API `.toml` files in `apis/`, `spec_dirs` auto-scan). Produces an `ApiRegistry` — a lightweight index of `ApiEntry { name, source, config_path }` that the CLI uses for Phase 1 routing without loading any specs. Also hosts the credential resolution stack types: env vars (`SPALL_<API>_TOKEN` where hyphens become underscores) > OS keyring > config file reference. Credential types use `secrecy::SecretString` — never raw strings for tokens.

Depends on: `serde`, `toml`, `dirs`, `secrecy`, `thiserror`.

**spall-cli**: The binary crate. Implements:
- **Two-phase parse**: Phase 1 loads `ApiRegistry` from spall-config, registers each API name as a clap subcommand with `allow_external_subcommands(true)` AND `disable_help_flag(true)` / `disable_version_flag(true)`. Phase 1 must NOT consume `--help` — it must fall through so Phase 2 can load the spec and show the real operation help. After Phase 1 matches an API name, extract remaining args, check for `--help` or `-h` manually, then load the matched spec, build the full operation command tree, and either print help or re-parse. If the spec fails to load and `--help` was requested, attempt to load a cached `SpecIndex` (lightweight routing metadata). If cached index exists, print a degraded operation list with a warning banner. If no cache exists, emit a structured `miette` diagnostic with `help:` text (e.g., "Try: spall api refresh <api>").
- **Centralized HTTP client** in `http.rs`: single `reqwest::Client::builder()` pipeline with stubs for timeout, proxy, TLS config, redirect policy, custom CA cert, client cert, insecure mode, default headers, user agent, base URL override, follow redirects, retry policy, and response time capture. Function signature MUST accept `--spall-timeout` (u64 seconds), `--spall-retry` (u8, max 3), and `--spall-follow` (bool) parameters. This is the highest-retrofit-cost component — get the interface right even if internals are `todo!()`.
- **Output formatting** in `output.rs`: TTY detection via `is-terminal`, output mode enum (`Pretty`, `Raw`, `Table`, `Yaml`), default to pretty when terminal / raw when piped. Support `--spall-output @file` (save response to file) and `--spall-download <path>`.
- **`--spall-*` prefixed flags**: ALL internal flags use `--spall-` prefix. Register these in the root clap Command as global args. Include: `--spall-output`, `--spall-verbose`, `--spall-debug`, `--spall-dry-run`, `--spall-header`, `--spall-auth`, `--spall-server`, `--spall-timeout`, `--spall-retry`, `--spall-follow`, `--spall-max-redirects`, `--spall-time`, `--spall-download`, `--spall-insecure`, `--spall-ca-cert`, `--spall-proxy`, `--spall-content-type`. `--spall-timeout` defaults to 30s. `--spall-retry` defaults to 1 (max 3). `--spall-follow` defaults to false (disabled). `--spall-max-redirects` defaults to 10.
- **Exit codes**: 0=success, 1=usage error, 2=network, 3=spec parse, 4=HTTP 4xx, 5=HTTP 5xx. Define as constants. (Exit code 10 for validation is Wave 2 — do not define yet.)
- **`spall api add|list|remove|refresh`** management subcommands. `refresh --all` batch operation is Wave 1.5.

Uses `#[tokio::main(flavor = "current_thread")]` — single-threaded runtime. Do NOT include `rt-multi-thread`.

Depends on: `spall-core`, `spall-config`, `clap`, `tokio`, `reqwest`, `miette`, `is-terminal`, `secrecy`.

### Depth of implementation

For each crate, produce:
- All public types with doc comments and correct field types (use `todo!()` for method bodies that require real logic).
- Trait boundaries and error types (`thiserror` enums with meaningful variants).
- The full two-phase clap wiring in `main.rs` — this is the hardest part architecturally. Get it right even if the inner functions are stubs. **Phase 1 API stub commands MUST call `.disable_help_flag(true)` and `.disable_version_flag(true)` so that `--help` reaches Phase 2.**
- The centralized `ClientBuilder` function signature with all config parameters even if the body is mostly `todo!()`. Must include `timeout`, `proxy`, `tls`, `redirects` (follow + max), `retry_policy`, `default_headers`, `user_agent`, `base_url_override`, `ca_cert`, `client_cert`, `client_key`, `insecure`.
- All `--spall-*` global flags registered in the clap command tree.
- Exit code constants.
- Output mode enum and TTY detection logic (this is ~10 lines, implement it fully).
- The `SpecIndex` / `ResolvedOperation` IR split with correct field types.
- Parameter merge function signature: `fn merge_parameters(path_params: &[ReferenceOr<Parameter>], op_params: &[ReferenceOr<Parameter>]) -> Vec<ResolvedParameter>`
- Security inheritance function signature: `fn resolve_security(root: Option<&[SecurityRequirement]>, operation: Option<&[SecurityRequirement]>) -> Vec<SecurityRequirement>`
- Credential resolution types in spall-config (the stack, not the implementations).
- `cache.rs` with `load_or_resolve()` type signature (check cache hash, fall back to parse+resolve+serialize) — body is `todo!()`, this is Wave 1.5.
- `output.rs` stub with `OutputMode` enum and `save_response(path, body)` function signature (Wave 1).
- `http.rs` stub: `fn build_http_client(config: &HttpConfig) -> reqwest::ClientBuilder` with all fields. Include `--spall-follow` support by setting `redirect(Policy::limited(max))` when enabled.

Do NOT produce: actual HTTP execution logic (request sending + response handling can be stubs), OAuth2 flows, response filtering/JMESPath, shell completions, REPL mode, request/response history, `--spall-repeat`, spec autodiscovery, chaining, or any Wave 2+ feature implementations. Stub those boundaries with `todo!()` and a doc comment noting what goes there.

### Stubbing and lints

Because this scaffold is full of `todo!()` and unused parameters, **add `#![allow(dead_code, unused_variables, unused_imports)]`** to the top of every crate's `lib.rs` or `main.rs` root so `cargo clippy` doesn't fail on incomplete scaffolding.

### CLAUDE.md content

Write a `CLAUDE.md` at the workspace root that future Claude Code sessions will consume. Keep it under 120 lines. It should cover:

1. **Project identity**: spall is a dynamic OpenAPI 3.x CLI. Name etymology. Tagline: "Break free. Hit the endpoint."
2. **Architecture**: Three-crate workspace. Two-phase parse pattern (Phase 1: registry scan; Phase 2: spec load + re-parse). Phase 1 API stubs must `.disable_help_flag(true)` so `--help` falls through to Phase 2.
3. **Tech stack**: `openapiv3` for spec deserialization (behavioral logic is ours). `clap` builder API (never derive). `serde_saphyr = "0.0.24"` (NOT `serde_yaml`). `reqwest` with `default-features = false` + `rustls-tls` + `multipart`. `tokio` current_thread. `miette` + `thiserror`. `secrecy` for credentials. `postcard` + `sha2` for IR cache.
4. **Critical conventions**:
   - All internal CLI flags use `--spall-*` prefix
   - Exit codes: 0=ok, 1=usage, 2=network, 3=spec, 4=4xx, 5=5xx
   - No `.unwrap()` in library crates. `miette` Result in CLI crate only.
   - All IR types derive `Serialize`/`Deserialize`; `SecretString` NEVER in IR
   - Credentials always wrapped in `SecretString` — never raw strings
   - `IndexMap` over `HashMap` where iteration order matters
   - Arg ID namespacing: `path-{n}`, `query-{n}`, `header-{n}`, `cookie-{n}`
   - **Error boundary**: `thiserror` in libraries, `miette` in CLI, `#[diagnostic(transparent)]` for passthrough
   - Never panic on user input
   - Rust 2021 edition. `#[must_use]` on Result-returning functions.
5. **Build/test**: `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace`, `cargo doc --workspace --no-deps`
6. **Wave structure**: Waves 1, 1.5, 2, 3, 4. Current: Wave 1. Wave 1.5 is IR cache. Don't implement beyond Wave 1 without explicit instruction.
7. **openapiv3 crate limitations**: Only deserialization. $ref resolution, parameter merging, security inheritance, cycle detection, lenient parsing — all manual. We evaluated `openapiv3-extended` (v6) and chose `openapiv3` v2; re-evaluate before Wave 2 if resolver edge cases become painful.
8. **Key patterns**: Parameter merge by `(name, in)` tuple. Security: operation replaces root, empty array = no auth. Cycle detection via visited HashSet + depth limit. Lenient parsing: missing types = any, force path params required, disambiguate duplicate operationIds.
9. **Resilience patterns**: JSON fallback if `serde_saphyr` fails on YAML (many URLs serve JSON). Cache-first `--help` degradation on spec load failure. Atomic `fs::rename` for cache writes.

### Example config files

Include in `examples/`:
- `config.toml` showing all three spec source methods (`[[api]]` inline, `spec_dirs`, reference to `apis/*.toml`)
- `apis/petstore.toml` showing per-API config with source, base_url override, default headers, and auth section (using `token_env` not raw token)

### README.md

Brief project README with:
- Name + tagline
- Etymology paragraph
- Feature summary (dynamic CLI from OpenAPI specs, no codegen)
- Status badge placeholder (alpha/WIP)
- Quick usage example (add API, list operations, make request)

## Verification

After scaffolding, run these checks and fix any issues:

```bash
cargo build --workspace 2>&1
cargo test --workspace 2>&1
cargo clippy --workspace 2>&1
cargo doc --workspace --no-deps 2>&1
```

All four must pass with zero errors. Warnings from incomplete stub code are acceptable because of the `#![allow(...)]` pragmas. If clippy emits errors (not warnings), fix them. If doc comments have broken links, fix them. If a dependency version doesn't exist on crates.io, find the correct current version and use that.

## Guidelines

- Use the clap builder API exclusively — no derive macros for clap. The entire point is runtime-dynamic command construction.
- For openapiv3 types, work with `ReferenceOr<T>` — this is how $refs appear in the parsed spec. Your resolver's job is to flatten these.
- `IndexMap` over `HashMap` where order matters (operation iteration, parameter order).
- The `ApiRegistry` in spall-config must be constructible in <5ms for 50 registered APIs — it should never open or parse spec files, only scan config and directory listings.
- Tokio: use `#[tokio::main(flavor = "current_thread")]`. Do NOT use `features = ["full"]` — specify only `rt`, `macros`, `net`, `time`, `io-util`.
- Wrap all credential/token types in `secrecy::SecretString`. The `SecretString` type prints `[REDACTED]` in Debug and zeroizes memory on drop.
- The centralized HTTP client builder in `http.rs` is the most important module in the CLI crate. Every HTTP configuration concern must flow through this single function. Get the function signature right.
- Avoid over-engineering. No trait abstractions or generics unless they serve a concrete purpose in the current wave. Concrete types first, extract traits when a second implementation appears.
- When naming synthesized operation IDs from `{method}-{path}`, algorithm: lowercase method, split path on `/`, remove `{...}` brackets, join with `-`, append `_2`, `_3` for duplicates.
- Cache writes must use temp file + atomic `fs::rename` to prevent corruption on concurrent invocations.
- **JSON fallback**: If `serde_saphyr` fails to parse YAML but the trimmed content starts with `{` or `[`, attempt `serde_json` directly. Many "YAML" URLs actually serve JSON.
- **Graceful `--help` degradation**: If Phase 2 spec load fails but the user requested `--help`, attempt to load a cached `SpecIndex`. If present, print a degraded operation list with a warning banner. Only emit a hard error if no cache exists.
