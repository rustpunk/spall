# `spall` — A Rust-native OpenAPI CLI

> Break free. Hit the endpoint.

`spall` is a dynamic CLI tool that parses OpenAPI 3.x specifications at runtime and generates fully-featured command-line interfaces for making API requests — with validation, auth, colored output, and schema-aware help.

*A spall is the fragment that breaks free from a corroding metal surface and flies. Your request — shaped by the spec, launched from the terminal, sent across the gap.*

Think **Restish, but Rust**.

---

## Why This Exists

The current landscape has a gap:

| Tool | Language | Approach | Limitation |
|------|----------|----------|------------|
| **Restish** | Go | Dynamic runtime CLI from OpenAPI | Go-only, somewhat stale |
| **Climate** | Go | Library for bootstrapping cobra from spec | Library, not standalone tool |
| **Hurl** | Rust | Plain-text HTTP runner | Not schema-driven |
| **openapi-generator** | Java | Codegen Rust client libraries | Static, heavy, requires JVM |
| **HTTPie / curl** | Various | Generic HTTP clients | No API awareness |
| **spall** | **Rust** | **Dynamic runtime CLI from OpenAPI** | **This project** |

No Rust tool dynamically loads an OpenAPI spec and turns it into a validated, documented CLI.

---

## Core Architecture

```
┌─────────────────────────────────────────────────────┐
│                    spall CLI                        │
│                                                     │
│  ┌─────────┐   ┌──────────┐   ┌──────────────────┐ │
│  │ Config   │   │ Registry │   │ Shell/Completion │ │
│  │ Manager  │   │ (APIs)   │   │ Engine           │ │
│  └────┬─────┘   └────┬─────┘   └──────────────────┘ │
│       │              │                               │
│  ┌────▼──────────────▼──────────────────────────┐   │
│  │           Spec Engine                        │   │
│  │  ┌────────────┐  ┌───────────┐  ┌─────────┐ │   │
│  │  │ Loader     │  │ Resolver  │  │ Command │ │   │
│  │  │ (file/url) │→ │ ($ref)    │→ │ Builder │ │   │
│  │  └────────────┘  └───────────┘  └────┬────┘ │   │
│  └──────────────────────────────────────┼──────┘   │
│                                         │           │
│  ┌──────────────────────────────────────▼──────┐   │
│  │           Execution Engine                  │   │
│  │  ┌───────────┐ ┌──────────┐ ┌────────────┐ │   │
│  │  │ Validator │ │ Request  │ │ Response   │ │   │
│  │  │ (input)   │ │ Builder  │ │ Formatter  │ │   │
│  │  └───────────┘ └──────────┘ └────────────┘ │   │
│  └─────────────────────────────────────────────┘   │
│                                                     │
│  ┌─────────────────────────────────────────────┐   │
│  │           Auth Layer                        │   │
│  │  API Key │ Bearer │ Basic │ OAuth2 (PKCE)   │   │
│  └─────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────┘
```

---

## Crate Dependencies

```toml
[dependencies]
# OpenAPI parsing
openapiv3 = "2"                    # OpenAPI 3.0.x structs (serde-native)

# CLI
clap = { version = "4", features = ["string"] }  # Builder API, no derive needed

# HTTP
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "rustls-native-certs", "multipart", "cookies"] }
tokio = { version = "1", features = ["rt", "macros", "net", "time", "io-util"] }  # current_thread, NOT rt-multi-thread

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_saphyr = "0.0.24"             # NOT serde_yaml (archived) or serde_yaml_ng
postcard = { version = "1", features = ["alloc"] }  # Stable wire format, no OOM hazard, Mozilla-sponsored
sha2 = "0.10"                     # Spec hash for cache invalidation

# Output
syntect = "5"                      # Syntax highlighting for response bodies
tabled = "0.20"                    # Table formatting
crossterm = "0.29"                 # Terminal colors/styling
is-terminal = "0.4"               # TTY detection for output format defaults

# Validation
jsonschema = "0.46"                # JSON Schema validation for request bodies

# Config/Auth
dirs = "5"                         # XDG config dirs
keyring = "3"                      # OS credential store. v4 is RC — evaluate before Wave 3.
secrecy = "0.10"                   # SecretString — zeroizes on drop, redacts in Debug
toml = "0.8"                       # Config file format
url = "2"                          # Server URL parsing / template expansion

# Collections
indexmap = { version = "2", features = ["serde"] }  # Ordered maps for parameter/operation iteration

# Utilities
thiserror = "2"
miette = { version = "7", features = ["fancy"] }  # Diagnostic errors (binary crate only)
```

### Why these choices

- **`openapiv3`**: Pure serde deserialization for OpenAPI 3.0.x. Stable, 6M+ downloads. **Critical limitation**: it handles *only* deserialization — no `$ref` resolution, no parameter merging, no security inheritance, no validation. All behavioral logic is our responsibility. We evaluated `openapiv3-extended` (v6) and chose `openapiv3` v2 because `extended` introduces additional structural complexity without clear value for our CLI use case. If we encounter significant resolver edge cases, re-evaluate before Wave 2.
- **`clap` builder API**: The derive API requires compile-time knowledge of subcommands. The builder API lets you construct `Command` and `Arg` objects dynamically from the parsed spec at runtime. This is the critical architectural decision.
- **`serde_saphyr` over `serde_yaml`**: dtolnay's original `serde_yaml` is archived/unmaintained. `serde_saphyr` (built on `saphyr`) is the maintained replacement. Avoid `serde_yaml_ng` and especially `serde_yml` (known unsoundness). Version `0.0.24` is the current latest. **Resilience**: if YAML parsing fails but the trimmed content starts with `{` or `[`, attempt `serde_json` directly — many "YAML" URLs actually serve JSON.
- **`tokio` current_thread**: CLI tools do sequential work. `current_thread` runtime has ~0.5ms startup vs ~2-3ms for multi-thread, lower memory, no `Send` bounds required. Use `#[tokio::main(flavor = "current_thread")]`.
- **`secrecy`**: Wraps credentials in `SecretString` — auto-zeroizes memory on drop, prints `[REDACTED]` in Debug. **Must never be cached via postcard** — credential types live in `spall-config`, not IR.
- **`miette` + `thiserror`**: `thiserror` for typed error enums in library crates, `miette` for rich diagnostic display in the binary crate only.
- **`is-terminal`**: TTY detection for output format defaults (pretty JSON when terminal, raw JSON when piped).
- **`indexmap`**: Ordered maps for parameter and operation iteration. OpenAPI spec order matters for CLI UX.
- **`postcard`** over `bincode`: `bincode` archived. `postcard` (v1.x) has a stable wire spec, produces the smallest output, and avoids bincode 1's unbounded length-prefix OOM hazard.
- **`reqwest` with `default-features = false`**: Prevents pulling in the default `native-tls` backend alongside `rustls-tls`, reducing binary bloat and cross-compilation issues.

### What openapiv3 does NOT handle

| Concern | Status | Spall must implement |
|---------|--------|---------------------|
| JSON/YAML deserialization | ✅ Handled | — |
| `$ref` as `ReferenceOr<T>` enum | ✅ Parsed | Resolution + cycle detection + depth limits |
| External file `$ref` | ❌ | File loading (Wave 4; require pre-bundled specs initially) |
| `allOf`/`anyOf`/`oneOf` | ✅ Parsed as enum variants | Flattening, property merging, discriminator logic |
| Path-level + operation-level param merge | ❌ | Merge by `(name, in)` tuple, operation overrides path |
| Security scheme inheritance | ❌ | Operation `security` replaces (not extends) root-level |
| Server variable substitution | ❌ | URL template expansion |
| Spec validation | ❌ | Lenient parsing strategy (see below) |
| Content type negotiation | ✅ Parsed as `IndexMap<String, MediaType>` | Default selection + `--spall-content-type` override |
| `x-*` extensions | ✅ Parsed as `IndexMap<String, Value>` | Interpretation of `x-cli-*` vocabulary |

---

## Feature Map

### Wave 1 — Core Request Flow (shippable MVP)

The goal: parse → build → send → format → save. Every feature here is required for a genuine `curl` replacement for OpenAPI APIs.

| Feature | Description |
|---------|-------------|
| `spall api add <n> <spec-path-or-url>` | Register an API from a local file or URL |
| `spall api list` | List registered APIs |
| `spall api remove <n>` | Unregister an API |
| `spall <api> --help` | Auto-generated help from spec info/description |
| `spall <api> <operation-id> [args]` | Execute an operation |
| Path params as positional args | `spall github get-user octocat` |
| Query params as `--flags` | `spall github list-repos --per-page 50` |
| Header params as `--header-<name>` | `--header-x-request-id uuid-123` |
| Cookie params as `--cookie-<name>` | `--cookie-session abc123` |
| Request body `--data` (JSON + raw) | `--data '{"n":"x"}'`, `--data @file.json`, `--data -` for stdin, `--data 'raw text' --spall-content-type text/plain` |
| `--form` multipart upload | `--form file=@image.png --form description="avatar"` for `multipart/form-data` |
| `--field` form-urlencoded | `--field grant_type=client_credentials` for `application/x-www-form-urlencoded` |
| Response output w/ TTY detection | Pretty JSON when terminal, raw JSON when piped, `--spall-output` override |
| Save response to file | `--spall-output @response.json` or `--spall-download ./invoice.pdf` |
| `--spall-follow` / `--spall-max-redirects` | Follow HTTP 3xx redirects (default: off, max 10); curl parity |
| `--spall-time` | Include request/response timing in `--spall-verbose` output |
| Tag grouping | Operations grouped under tag subcommands (`spall github repos list-repos`) |
| `--spall-verbose` / `--spall-debug` | Request/response headers to stderr; wire-level debug logging |
| `--spall-dry-run` | Print curl equivalent without executing |
| `--spall-header` injection | Non-sensitive headers only. For auth, use `--spall-auth` or env vars |
| `--spall-server` | Override base URL for a single request |
| `--spall-timeout` | Request / spec fetch timeout (default: 30s) |
| `--spall-retry` | Retry failed spec fetch / HTTP request (default: 1, max: 3) |
| `--spall-content-type` | Override request content type |
| `--spall-ca-cert` | Custom CA certificate path |
| `--spall-proxy` | HTTP/SOCKS proxy URL |
| `--spall-insecure` | Skip TLS verification |
| Centralized HTTP client | Single `ClientBuilder` pipeline for all requests |
| `--spall-*` flag namespace | All internal flags prefixed to avoid API parameter collision |
| `--spall-version` | `-V` — print version and exit |
| Exit code convention | 0=success, 1=usage, 2=network, 3=spec, 4=4xx, 5=5xx |
| Parameter merging | Path-level + operation-level merged by `(name, in)` tuple |
| Security scheme inheritance | Operation `security` replaces root-level; empty array = no auth |
| Lenient spec parsing | Graceful handling of missing types, duplicate IDs, broken `$ref` siblings |
| Credential env var support | `SPALL_<API>_TOKEN` environment variable resolution (hyphens → underscores) |
| `--spall-auth` basic token pass-through | `--spall-auth "Bearer $TOKEN"` for Wave 1; structured auth in Wave 3 |

### Wave 1.5 — Performance

| Feature | Description |
|---------|-------------|
| Pre-compiled IR cache | Serialize resolved spec to postcard, skip re-parsing on subsequent runs |
| Hash-based invalidation | SHA-256 of raw spec bytes; re-parse only when spec changes |
| IR version field | Automatic cache invalidation when spall upgrades change the IR struct layout |
| Remote spec caching | Cache URL-fetched specs locally with TTL and ETag support |
| Cache atomic writes | Temp file + atomic rename to prevent corruption on concurrent invocations |
| `spall api refresh --all` | Batch refresh all cached remote specs |

### Wave 2 — Quality of Life

| Feature | Description |
|---------|-------------|
| Input/response validation | Validate params/body against schema before request; warn on mismatch |
| Exit code 10 | Request body / parameter validation failed |
| `--profile` environment profiles | `--profile staging` / `--profile production`; `[profile.staging]` in per-API config |
| `--spall-paginate` | Auto-detect `Link: rel=next` (RFC 5988) or cursor params; follow until exhausted; output single concatenated JSON array |
| `--spall-preview` | Show resolved URL, headers, and body *without* sending |
| `x-cli-*` extensions | `x-cli-name`, `x-cli-hidden`, `x-cli-group` (Restish compat) |
| Config profiles | Per-API config (base URL override, default headers) |
| Shell completions | Generate bash/zsh/fish completions dynamically |
| Request/response history | SQLite log of recent calls |
| Output formats | JSON, YAML, table, CSV, raw |
| Response filtering (JMESPath) | `--filter '.items[].name'` — extract values without external `jq` |

### Wave 3 — Power Features

| Feature | Description |
|---------|-------------|
| Auth providers | API key, Bearer, Basic, OAuth2 Authorization Code w/ PKCE |
| Credential storage | OS keyring integration |
| Spec autodiscovery | RFC 8631 `service-desc` link relation |
| REPL / shell mode | `spall shell <api>` — interactive session, spec loaded once |
| Chaining | Capture response values, feed into next request |
| `--spall-repeat` | Replay last request (or replay from history) |

### Wave 4 — Ecosystem

| Feature | Description |
|---------|-------------|
| Daemon mode | Background process holds specs in memory; CLI becomes thin Unix socket client |
| Plugin system | WASM or Lua plugins for custom auth/transforms |
| Mock server | Serve mock responses from the spec |
| Diff | Compare two spec versions |
| `spall init` | Scaffold a spec from an API via probing |
| OpenAPI 3.1 support | Evaluate `oas3` crate vs extending current resolver |
| Spec import from Postman/Bruno | Migration path from existing tools |

---

## Key Design Decisions

### 1. Dynamic Command Building (the core trick)

The entire CLI tree is constructed at runtime from the parsed spec.

**Critical: Arg ID uniqueness.** OpenAPI allows a path param `id` and a query param `id` on the same operation. clap `Arg` IDs must be unique within a `Command`. Namespace them internally while preserving user-facing names.

```rust
// Pseudocode: spec → clap Commands
fn build_commands(spec: &ResolvedSpec) -> clap::Command {
    let mut root = Command::new("spall");
    let groups = group_by_tag(&spec.operations);

    for (tag, operations) in groups {
        let mut group_cmd = Command::new(&tag)
            .about(tag_description(spec, &tag));

        for op in operations {
            let op_id = op.operation_id.to_kebab_case();
            let mut op_cmd = Command::new(&op_id)
                .about(op.summary.as_deref().unwrap_or(""));

            // Path params → positional args (internal ID: path-{name})
            for param in op.path_params() {
                let id = format!("path-{}", param.name);
                op_cmd = op_cmd.arg(
                    Arg::new(&id)
                        .value_name(&param.name)
                        .required(true)  // always required per lenient parsing
                        .help(schema_aware_help(&param))
                );
            }

            // Query params → --flags (internal ID: query-{name})
            for param in op.query_params() {
                let id = format!("query-{}", param.name);
                op_cmd = op_cmd.arg(
                    Arg::new(&id)
                        .long(&param.name)
                        .required(param.required)
                        .help(schema_aware_help(&param))
                        .default_value_if(param.schema.default.as_ref().map(|v| v.to_string()))
                        .value_parser(clap_value_parser(&param.schema))  // enums → possible_values, types → value parsers
                );
            }

            // Header params → --header-{name}
            for param in op.header_params() {
                let id = format!("header-{}", param.name);
                op_cmd = op_cmd.arg(
                    Arg::new(&id)
                        .long(format!("header-{}", param.name.to_kebab_case()))
                        .required(param.required)
                        .help(schema_aware_help(&param))
                );
            }

            // Cookie params → --cookie-{name}
            for param in op.cookie_params() {
                let id = format!("cookie-{}", param.name);
                op_cmd = op_cmd.arg(
                    Arg::new(&id)
                        .long(format!("cookie-{}", param.name.to_kebab_case()))
                        .required(param.required)
                        .help(schema_aware_help(&param))
                );
            }

            if let Some(body) = &op.request_body {
                let mut data_arg = Arg::new("data")
                    .long("data")
                    .short('d')
                    .action(clap::ArgAction::Append)  // repeatable
                    .help("Request body (JSON). Use @file.json or - for stdin. Prefix with -- --no-data to skip.");
                if body.required {
                    data_arg = data_arg.required(true);
                } else {
                    op_cmd = op_cmd.arg(Arg::new("no-data").long("no-data").help("Send request with no body"));
                }
                op_cmd = op_cmd.arg(data_arg);
            }

            if op.deprecated {
                op_cmd = op_cmd.before_help("[DEPRECATED] This operation is deprecated.");
            }

            group_cmd = group_cmd.subcommand(op_cmd);
        }

        root = root.subcommand(group_cmd);
    }

    root
}

The `clap_value_parser` helper inspects the resolved schema and, when available, sets up `.value_parser()` for enums (`possible_values`), numeric/boolean type validators, and `.default_value()` from OpenAPI `schema.default`.
```

### 2. Operation ID as Command Name

`operationId` becomes the CLI command name, kebab-cased. If no `operationId`, synthesize from method and path:

```
GET /users/{id}      → "get-users-by-id"
POST /repos           → "post-repos"
GET /items/{id}/tags  → "get-items-by-id-tags"
```

**Algorithm:**
1. Lowercase HTTP method.
2. Split path on `/`.
3. Remove leading/trailing empty segments.
4. Remove `{` and `}` brackets from path-param placeholders.
5. Join remaining segments with `-`.
6. If duplicate, append `_2`, `_3`, etc.

### 3. Parameter Serialization (`style` and `explode`)

OpenAPI 3.0 defines how array/object parameters serialize into the URL. Default rules:

| Location | Default `style` | Default `explode` | Array example (input `[a,b]`) |
|----------|----------------|-------------------|------------------------------|
| path | `simple` | `false` | `/a,b` |
| query | `form` | `true` | `?tags=a&tags=b` |
| header | `simple` | `false` | `X-Tags: a,b` |
| cookie | `form` | `false` | `tags=a%2Cb` |

`ResolvedParameter` must carry `style` and `explode` fields. The request builder serializes accordingly.

### 4. $ref Resolution Strategy

Resolution happens once at load time into a compact IR.

- **Cycle detection**: `HashSet<String>` visited set + depth limit (8–10). Emit `RecursiveSchema` marker beyond limit.
- **Dangling refs**: If `$ref` points to a non-existent component, emit a clear error:
  > `SpecError::UnresolvedRef { path: "#/components/schemas/Foo", context: "operation create-repo" }`
- **Parameter merging**: Merge by `(name, in)` tuple, operation overrides path.
- **Security inheritance**: Operation `security` **completely replaces** root-level. Empty array `security: []` = no auth.

**IR split:**

```rust
#[derive(Serialize, Deserialize)]
pub struct SpecIndex {
    pub title: String,
    pub base_url: String,
    pub version: u32,           // IR format version
    pub operations: Vec<OperationMeta>,
}

#[derive(Serialize, Deserialize)]
pub struct ResolvedOperation {
    pub operation_id: String,
    pub method: HttpMethod,
    pub path_template: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub deprecated: bool,
    pub parameters: Vec<ResolvedParameter>,
    pub request_body: Option<ResolvedRequestBody>,
    pub responses: IndexMap<String, ResolvedResponse>,
    pub security: Vec<SecurityRequirement>,
    pub tags: Vec<String>,
    pub extensions: IndexMap<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
pub struct ResolvedResponse {
    pub description: Option<String>,
    pub content: IndexMap<String, ResolvedMediaType>,  // status code → content type → schema
    pub headers: IndexMap<String, ResolvedHeader>,
}
```

**No openapiv3 types leak into IR.** All types derive `Serialize`/`Deserialize`. Credential-bearing types (`SecretString`) are **never** included in IR — they live in `spall-config` only.

### 5. Two-Phase Parse (Lazy Spec Loading)

```
Phase 1: Index scan (~1ms)          Phase 2: Spec load (~50-200ms)
┌──────────────────────┐            ┌──────────────────────────┐
│ Read config.toml     │            │ Load matched spec file   │
│ Scan apis/*.toml     │            │ Deserialize OpenAPI      │
│ Scan spec_dirs       │     ┌─────→│ Resolve $refs            │
│                      │     │      │ Build operation Commands │
│ Build thin Command:  │     │      │ Re-parse remaining args  │
│   spall              │     │      │ Execute request          │
│   ├── github    ──── │ ────┘      └──────────────────────────┘
│   ├── petstore       │
│   ├── internal       │
│   └── api (manage)   │
│                      │
│ Match: "github"      │
└──────────────────────┘
```

**Phase 1:** Scan config and register API names as clap subcommands. **Disable clap's built-in help flag** on API stubs so `--help` falls through to Phase 2.

```rust
fn build_phase1(registry: &[ApiEntry]) -> clap::Command {
    let mut root = Command::new("spall")
        .subcommand(api_management_cmd());

    for entry in registry {
        root = root.subcommand(
            Command::new(&entry.name)
                .about(format!("{}", entry.source))
                .allow_external_subcommands(true)
                .disable_help_flag(true)    // CRITICAL: let --help fall through to Phase 2
                .disable_version_flag(true)
        );
    }

    root
}
```

**Phase 2:** Detect `--help`/`-h` manually in remaining args after Phase 1 match. Load spec, build full command tree, then route to help or execution.

```rust
fn execute(registry: &[ApiEntry], args: Vec<String>) -> Result<()> {
    let phase1 = build_phase1(registry);
    let matches = phase1.try_get_matches_from(&args)?;

    match matches.subcommand() {
        Some(("api", sub)) => handle_api_management(sub),
        Some((api_name, phase1_matches)) => {
            let remaining: Vec<String> = collect_remaining_args(phase1_matches);
            let wants_help = remaining.iter().any(|a| a == "--help" || a == "-h");

            let entry = registry.iter().find(|e| e.name == api_name).unwrap();

            let spec_result = load_and_resolve_spec(&entry.source);
            let api_config = load_api_config(&entry.config);

            match (spec_result, wants_help) {
                (Ok(spec), true) => {
                    let phase2_cmd = build_operations_cmd(api_name, &spec, &api_config);
                    phase2_cmd.print_help()?;
                    Ok(())
                }
                (Ok(spec), false) => {
                    let phase2_cmd = build_operations_cmd(api_name, &spec, &api_config);
                    let op_matches = phase2_cmd.try_get_matches_from(remaining)?;
                    execute_operation(&spec, &api_config, &op_matches).await
                }
                (Err(e), true) => {
                    if let Some(cached_index) = try_load_cached_index(&entry.source) {
                        eprintln!("⚠  Could not load spec for '{}'. Showing cached operation list from {}.",
                                  api_name, cached_index.cached_at);
                        print_degraded_help(api_name, &cached_index)?;
                        Ok(())
                    } else {
                        Err(SpallCliError::SpecLoadFailed {
                            api: api_name.to_string(),
                            source: entry.source.clone(),
                            cause: e,
                        }.into())
                    }
                }
                (Err(e), false) => {
                    Err(SpallCliError::SpecLoadFailed {
                        api: api_name.to_string(),
                        source: entry.source.clone(),
                        cause: e,
                    }.into())
                }
            }
        }
        None => { /* print root help (API list) */ }
    }
}
```

### 5.8. Graceful `--help` Degradation on Unreachable Specs

When `spall github --help` fails because the spec URL is down, spall attempts a **cache-first fallback**:

1. Attempt to load the cached `SpecIndex`.
2. Print a `⚠  Could not load spec for 'github'. Showing cached operation list from <date>.` banner.
3. Print operation names, methods, and tags from the index.
4. Emit a structured `miette` diagnostic only if no cached index exists.

`SpecIndex` is always safe to cache — it contains no secrets, just routing metadata.

---

### 5.9. Spec Load Failure UX

| Failure mode | Exit code | User-facing message pattern |
|-------------|-----------|----------------------------|
| DNS resolution failure | 2 | `Could not resolve host 'intranet.example.com'. Check your network or VPN.` |
| TCP timeout (spec fetch) | 2 | `Connection timed out after 30s fetching spec from <URL>. Retried <n> times.` |
| HTTP 4xx/5xx on spec URL | 3 | `Server returned HTTP 503 from <URL>. [Body preview if text/html]` |
| Wrong Content-Type | 3 | `Server returned 'text/html', expected JSON or YAML. If this URL requires authentication, verify your credentials.` |
| YAML parse error | 3 | `Invalid YAML at line 42, col 5: expected mapping but got scalar` |
| JSON parse error | 3 | `Invalid JSON: trailing comma at line 15` |
| Dangling `$ref` | 3 | `Unresolved $ref #/components/schemas/Foo in operation 'createRepo'.` |
| Cache deserialization error | 3 | `Corrupted IR cache. Deleted; re-parsing from source...` (warn, not fail) |
| External file `$ref` | 3 | `External $ref not supported. Pre-bundle with redocly or swagger-cli.` |

These messages go through `miette` in the CLI crate.

---

### 5.10. SpallError Cross-Crate Pattern

Library crates (`spall-core`, `spall-config`) emit typed errors via `thiserror`. The CLI crate (`spall-cli`) converts them into user-facing diagnostics via `miette`. The conversion boundary is explicit:

```rust
// spall-core/src/error.rs
#[derive(thiserror::Error, Debug)]
pub enum SpallCoreError {
    #[error("spec parse failed: {source}")]
    SpecParse { source: serde_saphyr::Error, url: String },
    #[error("unresolved $ref: {path}")]
    UnresolvedRef { path: String, context: String },
    #[error("cycle detected in $ref at depth {depth}")]
    RefCycle { path: String, depth: usize },
}

// spall-cli/src/main.rs
use miette::{Diagnostic, Report};
use spall_core::SpallCoreError;
use spall_config::SpallConfigError;

#[derive(thiserror::Error, Diagnostic, Debug)]
enum SpallCliError {
    #[error("Failed to load spec for '{api}'")]
    #[diagnostic(help("Check the URL or run `spall api refresh {api}`.\nIf this API requires a VPN, ensure you're connected."))]
    SpecLoadFailed { api: String, source: String, #[source] cause: SpallCoreError },

    #[error("Config error")]
    #[diagnostic(transparent)]
    Config(#[from] SpallConfigError),
}

fn main() -> miette::Result<()> { /* ... */ }
```

**Key rules:**
- Library errors carry machine-readable context.
- CLI errors add human-readable `help:` text.
- Never `panic!` on user-controlled inputs.

---

### 6. Content Type Negotiation

When an operation supports multiple request body content types, default to `application/json`. Allow override via `--spall-content-type <type>`.

Response handling: inspect `Content-Type` header.
- `application/json` → pretty/JSON output
- `application/xml` / `text/xml` → syntax highlight if supported
- `text/plain`, `text/html` → pass through unmodified
- Binary / unknown → stream raw bytes to stdout with a `Warning: binary output` message when TTY
- With `--spall-download <path>` → write to file, bypass TTY check

### 7. Config & Spec Loading

#### Config Layout

```
~/.config/spall/
├── config.toml          # Global settings + spec sources
├── apis/
│   ├── github.toml      # Per-API overrides
│   └── myservice.toml
├── specs/               # Auto-load directory (optional)
│   ├── petstore.json
│   └── internal-api.yaml
└── cache/
    ├── github.json      # Cached remote specs
    ├── github.ir.bin    # Pre-compiled resolved IR
    └── github.meta.toml # Cache metadata
```

#### Global Config

```toml
[[api]]
name = "github"
spec = "https://raw.githubusercontent.com/.../api.github.com.json"

spec_dirs = [
    "~/.config/spall/specs",
]

[defaults]
output = "json"    # json | yaml | table | raw
color = "auto"     # auto | always | never
```

#### Source Priority

1. `[[api]]` entries in `config.toml`
2. `.toml` files in `apis/`
3. Spec files in `spec_dirs`

#### Spec Caching

**Remote spec cache:**
- Validate HTTP `Content-Type` before caching (accept JSON/YAML; reject HTML).
- TTL + ETag support.
- Stale cache + network failure → warn but use stale.

**IR cache:**
- Atomic writes: write to `github.ir.bin.tmp`, then `fs::rename`.
- Invalidation: SHA-256 of raw spec + `ir_version` field.

#### Name Derivation

Files in `spec_dirs`:
```
petstore.json          → "petstore"
my-internal-api.yaml   → "my-internal-api"
v2_billing.yml         → "v2-billing"
```

### 8. Credential Architecture

**Never accept secrets as CLI positional args.** `--spall-header` is restricted to **non-sensitive** headers. Auth via:

1. `--spall-auth "Bearer ..."` or `--spall-auth "Basic ..."` (Wave 1 pass-through)
2. Environment variables (`SPALL_<API>_TOKEN`). Hyphens → underscores: `my-api` → `SPALL_MY_API_TOKEN`.
3. OS keychain via `keyring` (Wave 3).
4. Per-API config `[auth]` section (references only).
5. Interactive prompt via `rpassword`.

All credentials wrapped in `secrecy::SecretString`. Debug logging redacts sensitive headers.

### 9. Flag Namespace Prefix

| Internal flag | Short | Purpose |
|--------------|-------|---------|
| `--spall-output` | `-O` | Output format, or `@file` to save response |
| `--spall-verbose` | `-v` | Headers + timing to stderr |
| `--spall-debug` | | Wire-level logging (redacts secrets) |
| `--spall-dry-run` | | Print curl equivalent |
| `--spall-header` | `-H` | Inject non-sensitive header (repeatable) |
| `--spall-auth` | `-A` | Pass-through auth token/header (Wave 1) |
| `--spall-server` | `-s` | Override base URL |
| `--spall-timeout` | `-t` | Request / spec fetch timeout (default: 30s) |
| `--spall-retry` | | Retry count (default: 1, max: 3) |
| `--spall-follow` | `-L` | Follow redirects (default: off) |
| `--spall-max-redirects` | | Max redirects (default: 10) |
| `--spall-time` | | Include timing in verbose output |
| `--spall-download` | `-o` | Save response body to file |
| `--spall-preview` | | Preview resolved request without sending (Wave 2) |
| `--spall-insecure` | | Skip TLS verification |
| `--spall-ca-cert` | | Custom CA cert path |
| `--spall-proxy` | | Proxy URL |
| `--spall-content-type` | `-c` | Override request content type |

**`--spall-version` / `-V`**: Standard version flag on root command.

### 10. Exit Code Convention

| Code | Meaning |
|------|---------|
| 0 | Success (2xx response) |
| 1 | CLI usage error |
| 2 | Network / connection error |
| 3 | Spec loading / parsing error |
| 4 | HTTP 4xx response |
| 5 | HTTP 5xx response |

**Wave 2 adds:**
| Code | Meaning |
|------|---------|
| 10 | Request body / parameter validation failed |

### 11. Lenient Parsing Strategy

- **Missing `type`**: Treat as `AnySchema`.
- **`$ref` siblings**: Ignore siblings, use `$ref` target.
- **Duplicate operationIds**: Warn, disambiguate with method prefix.
- **Path params not marked required**: Force to `required = true`.
- **External file `$ref`**: Emit error with bundler recommendation.
- **Empty `paths`**: Print "This API has no operations defined."
- **All operations deprecated**: Still show them, mark with `[deprecated]`.

### 12. `readOnly` / `writeOnly` Filtering

- **Request body help / validation**: Filter out `readOnly` properties.
- **Response display / validation**: Filter out `writeOnly` properties.

### 13. `x-cli-*` Extensions (Partial Restish Compatibility)

Covers naming, hiding, and grouping. Wait loops, body shorthand, and auto-config are Wave 3+.

---

## Edge Case Reference

| Scenario | Behavior |
|----------|----------|
| `spall` (no args) | Print root help (API list) |
| `spall github` (no operation) | Load spec, print operation list (same as `--help`) |
| `spall github --help` | Phase 2 loads spec, prints full operation help |
| `spall github --help` (spec unreachable) | Show cached `SpecIndex` with ⚠ banner; if no cache, emit structured error |
| `--data @file.json --data -` | Last wins. Repeatable → collect into Vec. |
| `--form file=@image.png` | Upload multipart file. Maps to reqwest `multipart::Form`. |
| `--field grant_type=client_credentials` | Send `application/x-www-form-urlencoded` body. |
| `--spall-download ./file.pdf` | Write response body to file, bypass TTY check. |
| Path param `id` + query param `id` | Internal IDs: `path-id` and `query-id`. User uses positional and `--id` respectively. |
| Non-JSON response | Pass through raw; pretty-print if recognized |
| Binary response | Stream to stdout; warn if TTY; save silently with `--spall-download` |
| Unicode in operationIds | Pass through verbatim; shell may not tab-complete |
| Network timeout during fetch | Retry up to `--spall-retry` times; fall back to stale cache |
| Cache file corrupted | Delete cache, re-parse, log warning |
| Concurrent invocations | Atomic cache writes prevent corruption |

---

## Name: Etymology

**spall** — /spɔːl/ — *noun & verb*

In materials science, a **spall** is a fragment that breaks free from a corroding or stressed metal surface and travels. The rusting process produces spalls — shaped by the chemical reaction, detached from the source, sent across space.

The metaphor: the OpenAPI spec is the oxidation pattern. `spall` shapes a fragment according to the spec and launches it across the network.

Available on crates.io. Five characters, one syllable, double-L (`curl`, `null`, `pull`).

---

## Scope Guardrails

**Wave 1 is shippable when:**
1. `spall api add petstore https://...`
2. `spall petstore --help` → shows all operations grouped by tag (or degraded cached list on failure)
3. `spall petstore get-pet-by-id 1` → GET, colored JSON, with `--spall-follow` and `--spall-time`
4. `spall petstore add-pet --data '{"name":"Rex"}'` → POST with body
5. `spall petstore upload-file --form file=@image.png` → multipart upload works
6. `spall petstore list-repos --spall-download repos.json` → save response to file
7. `spall petstore --spall-server https://staging.example.com` → request hits staging

Everything else is enhancement.

---

## Prior Art References

- [Restish](https://rest.sh/) — Gold standard. Study its OpenAPI-to-CLI mapping, `x-cli-*` extensions, and auth model.
- [Climate](https://github.com/lispyclouds/climate) — Clean Go library approach.
- [openapi-cli-generator](https://github.com/danielgtaylor/openapi-cli-generator) — Restish's predecessor.
- [Hurl](https://hurl.dev/) — Excellent Rust HTTP tooling for output formatting patterns.
