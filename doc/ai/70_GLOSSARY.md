# Glossary

| Term | Meaning In This Repo | Where It Appears | Related Terms | Confidence |
| --- | --- | --- | --- | --- |
| Spall | Dynamic Rust OpenAPI CLI and workspace name | `README.md`, crate names | OpenAPI CLI | High |
| OpenAPI | API specification format parsed at runtime | `spall-core`, docs | OAS, spec | High |
| Arazzo | Workflow description support modeled in core and run by CLI | `spall-core/src/arazzo`, `spall-cli/src/arazzo_runner.rs` | workflow, step | High |
| MCP | Model Context Protocol server surface for exposing operations as tools | `spall-cli/src/mcp`, docs | tools, JSON-RPC | High |
| IR | Resolved internal representation of an OpenAPI spec | `spall-core/src/ir.rs` | `ResolvedSpec`, cache | High |
| `ResolvedSpec` | Full resolved OpenAPI representation used by CLI | `spall-core/src/ir.rs` | `SpecIndex` | High |
| `SpecIndex` | Lightweight cache/index used for degraded help or operation listing | `spall-core/src/ir.rs`, `cache.rs` | `.idx` | High |
| `SpallValue` | Closed JSON-shaped enum for cacheable IR dynamic values | `spall-core/src/value.rs` | postcard, IR | High |
| Phase 1 parse | CLI pass over built-ins and registered API names before spec loading | `spall-cli/src/main.rs` | registry, stubs | High |
| Phase 2 parse | CLI pass after loading selected API spec and building operations | `spall-cli/src/main.rs` | dynamic commands | High |
| API registry | Lightweight index of configured APIs | `spall-config/src/registry.rs` | `ApiRegistry`, `ApiEntry` | High |
| `AuthConfig` | TOML-facing auth configuration | `spall-config/src/auth.rs` | `ResolvedAuth` | High |
| `ResolvedAuth` | Runtime auth representation using secret wrappers | `spall-config/src/auth.rs`, CLI auth | `SecretString` | High |
| `SecretString` | Secret wrapper used to avoid accidental exposure | config/auth/openapi auth | secrecy | High |
| Hasp | Optional credential backend integration in CLI | `spall-cli/Cargo.toml`, `plan-hasp-integration.md` | secret URL | Medium |
| `HttpRequestSpec` | Transport-neutral request descriptor | `spall-openapi/src/request.rs` | transport, reqwest | High |
| `ResponseStream` | Transport-neutral streaming response body | `spall-openapi/src/response.rs` | `ItemStream` | High |
| `ItemStream` | Lazy iterator over JSON items, including paginated pages | `spall-openapi/src/stream.rs` | `JsonSkimmer` | High |
| `JsonSkimmer` | Forward-only bounded JSON parser/skimmer | `spall-openapi/src/stream.rs` | stream limits | High |
| `DataPath` | Location of array/items in response body | `spall-openapi/src/datapath.rs` | JSON Pointer | High |
| RFC 5988 Link | Header parsing used for `rel=next` pagination | `links.rs`, `paginate.rs` | pagination | High |
| `--spall-*` | Reserved internal CLI flag namespace | docs, `spall-cli` | global flags | High |
| `ResponseContext` | Mutable response state used for REPL/chain flow | `spall-cli/src/execute.rs` | chaining | High |
| `docs/src` | mdBook source docs | `docs/src/SUMMARY.md` | user docs | High |
| `docs/book` | generated mdBook output | `docs/book/` | generated docs | High |
| `doc/ai` | durable AI onboarding docs | this directory | AGENTS | High |
| `reserve` | crates.io name reservation placeholder | `reserve/README.md` | not implementation | High |
