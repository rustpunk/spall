# Common Patterns

## Two-Phase CLI Parse

- Where it appears: `spall-cli/src/main.rs`, `spall-cli/src/command.rs`, `spall-cli/src/matches.rs`.
- Rationale: API names are known from config before specs are loaded; full operation commands require loading the selected spec.
- Copy it correctly: keep Phase 1 lightweight, use API stubs, then build the full operation tree in Phase 2. Preserve help/version fallthrough behavior.
- Common mistakes: loading every spec at startup, making `--help` terminate before Phase 2, or assuming dynamic operations can use Clap derive.
- Evidence: `CLAUDE.md`, `spall-cli/src/main.rs`, `spall-cli/src/command.rs`.

## Crate Boundary By Responsibility

- Where it appears: all workspace crates and CI.
- Rationale: keep parsing/config/neutral request contract/runtime behavior separately testable.
- Copy it correctly: put OpenAPI resolution in `spall-core`, config in `spall-config`, neutral request/streaming in `spall-openapi`, and concrete CLI/HTTP behavior in `spall-cli`.
- Common mistakes: adding transport dependencies to neutral crates or moving command-building into core.
- Evidence: root `Cargo.toml`, `CLAUDE.md`, crate docs, `.github/workflows/ci.yml`.

## Transport-Neutral Request Contract

- Where it appears: `spall-openapi/src/request.rs`, `builder.rs`, `response.rs`, `auth/*`, `spall-cli/src/transport.rs`.
- Rationale: request building and response streaming can be tested or embedded without binding to `reqwest`/Tokio/Clap.
- Copy it correctly: describe requests as `HttpRequestSpec`, `Headers`, `RequestBody`, and `MultipartField`; perform concrete network/file work in the caller.
- Common mistakes: reading files or stdin in `build_request`, resolving credentials in `spall-openapi`, or setting multipart content-type boundaries outside the transport.
- Evidence: `spall-openapi/src/lib.rs`, `spall-openapi/examples/embed.rs`, CI purity check.

## Cacheable IR Uses Closed Values

- Where it appears: `spall-core/src/ir.rs`, `value.rs`, `cache.rs`, `resolver.rs`.
- Rationale: postcard cannot deserialize arbitrary `serde_json::Value` through `deserialize_any`, so cached IR uses `SpallValue`.
- Copy it correctly: translate open JSON-shaped values into `SpallValue` at the resolver boundary; convert back to `serde_json::Value` only at consumers that need it.
- Common mistakes: adding `serde_json::Value` to `ResolvedSpec` fields or changing cache-visible structs without cache tests/version review.
- Evidence: `docs/internal/research/RESEARCH-ir-cache-deserialize-any.md`, `spall-core/src/value.rs`, `spall-core/tests/cache_roundtrip_test.rs`.

## YAML Chokepoint

- Where it appears: `spall-core/src/yaml.rs`, `spall-core/src/loader.rs`.
- Rationale: centralize `serde_saphyr` usage and enforce DoS budgets.
- Copy it correctly: call `spall_core::yaml::from_str`; do not call `serde_saphyr` directly elsewhere.
- Common mistakes: bypassing budgets or adding another YAML dependency.
- Evidence: `CLAUDE.md`, `spall-core/src/yaml.rs`.

## Secret Wrapping And Redaction

- Where it appears: `spall-config/src/auth.rs`, `spall-cli/src/auth/*`, `spall-openapi/src/auth/*`, MCP verbose redaction.
- Rationale: secrets must be held in redacting wrappers and not persisted accidentally.
- Copy it correctly: keep config/runtime secrets in `SecretString`; expose only at the boundary needed to build headers or token requests; redact diagnostics.
- Common mistakes: serializing inline secrets expecting round-trip persistence, logging headers unredacted, or storing secrets in IR/cache.
- Evidence: `spall-config/tests/auth_serde_test.rs`, `spall-cli/tests/redaction_audit.rs`, `spall-openapi/src/auth/*`.

## Operation Parameter Namespacing

- Where it appears: `spall-cli/src/command.rs`, `spall-cli/src/execute.rs`, `spall-core/src/resolver.rs`.
- Rationale: OpenAPI can have parameters with the same name in different locations, while Clap arg IDs must be unique.
- Copy it correctly: keep internal IDs like `path-*`, `query-*`, `header-*`, and `cookie-*`; keep user-facing names clean.
- Common mistakes: treating all parameters as one namespace or using raw OpenAPI names as Clap IDs.
- Evidence: `CLAUDE.md`, `spall-cli` explorer report.

## Bounded Streaming Pagination

- Where it appears: `spall-openapi/src/stream.rs`, `paginate.rs`, `spall-cli/src/execute.rs`, `spall-openapi/tests/stream_bound.rs`.
- Rationale: large paginated JSON responses must not be concatenated or buffered whole.
- Copy it correctly: use `ItemStream`/`JsonSkimmer` and preserve item/buffer caps.
- Common mistakes: collecting all pages into a `Vec<Value>` or removing stream bound tests.
- Evidence: `spall-openapi/src/lib.rs`, `spall-openapi/tests/stream_bound.rs`, `spall-cli/tests/stream_guard_e2e.rs`.

## OpenAPI Inheritance Overrides

- Where it appears: `spall-core/src/resolver.rs`.
- Rationale: OpenAPI path/operation inheritance must be collapsed into Spall's resolved IR.
- Copy it correctly: merge parameters by `(name, in)` with operation overriding path; use operation security in place of root security, including explicit empty security as no auth; server precedence is operation > path > spec > `/`.
- Common mistakes: appending security requirements or losing explicit empty action/security semantics.
- Evidence: `spall-core/src/resolver.rs`, `spall-core/tests/resolver_test.rs`.

## MCP Stdout Discipline

- Where it appears: `spall-cli/src/mcp/*`, `docs/src/operations/mcp.md`.
- Rationale: MCP stdio requires stdout to be JSON-RPC only.
- Copy it correctly: write diagnostics, warnings, and logs to stderr.
- Common mistakes: printing debug/help text to stdout from MCP paths.
- Evidence: `spall-cli/src/mcp/mod.rs`, `docs/src/operations/mcp.md`.
