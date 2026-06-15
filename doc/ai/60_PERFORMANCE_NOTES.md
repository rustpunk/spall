# Performance Notes

## Two-Phase CLI Startup

- Area: `spall-cli/src/main.rs`, `spall-cli/src/command.rs`, `spall-config/src/registry.rs`.
- Why sensitive: startup and top-level help should not parse every registered spec.
- Existing choices: registry-first Phase 1, then spec-load Phase 2 for selected API/help.
- Avoid: scanning specs or loading remote URLs during registry construction.
- Tests/hooks: CLI e2e tests, smoke script cache/help checks.
- Confidence: High.
- Evidence: `CLAUDE.md`, `spall-cli` explorer report, `spall-config` explorer report.

## API Registry Loading

- Area: `spall-config/src/registry.rs`, `spall-config/src/sources.rs`.
- Why sensitive: registry loads on every CLI invocation.
- Existing choices: comments target lightweight construction and no spec parsing.
- Avoid: adding OpenAPI parse/network work to `ApiRegistry::load`.
- Tests/hooks: `cargo test -p spall-config`, `sources_test.rs`, `registry_test.rs`.
- Confidence: High.
- Evidence: `spall-config/src/registry.rs`, tests.

## IR Cache

- Area: `spall-core/src/cache.rs`, `ir.rs`, `value.rs`.
- Why sensitive: resolved specs can be expensive to parse/resolve; cache is the hot path for repeat invocations and help.
- Existing choices: postcard serialization, SHA-256 source/spec hashes, `IR_VERSION`, separate `SpecIndex`.
- Avoid: cache-visible `serde_json::Value`, non-atomic writes, or changing IR layout without version/test review.
- Tests/hooks: `spall-core/tests/cache_roundtrip_test.rs`, `e2e/smoke.sh`.
- Confidence: High.
- Evidence: `spall-core` explorer report, internal cache research.

## YAML Parsing

- Area: `spall-core/src/yaml.rs`.
- Why sensitive: untrusted YAML can be large or pathological.
- Existing choices: single parser chokepoint and hard budgets for input/structure.
- Avoid: direct `serde_saphyr` calls outside the chokepoint or adding another YAML parser.
- Tests/hooks: module tests in `yaml.rs`.
- Confidence: High.
- Evidence: `CLAUDE.md`, `spall-core/src/yaml.rs`.

## Resolver Cold Path

- Area: `spall-core/src/resolver.rs`.
- Why sensitive: `$ref` resolution, schema traversal, and parameter/security/server inheritance happen on cold parse.
- Existing choices: recursion/depth guard, cycle placeholders, ordered maps.
- Avoid: unbounded recursion, external ref behavior changes without design, or order-destroying map changes.
- Tests/hooks: `spall-core/tests/resolver_test.rs`.
- Confidence: Medium-High.
- Evidence: `spall-core` explorer report.

## Bounded JSON Streaming

- Area: `spall-openapi/src/stream.rs`, `paginate.rs`, `spall-cli/src/execute.rs`.
- Why sensitive: paginated API responses may be large enough to OOM if buffered.
- Existing choices: `JsonSkimmer`, `ItemStream`, `StreamLimits`, max nesting, max item bytes, max buffered bytes, lazy page fetching.
- Avoid: collecting whole pages or all paginated items before output.
- Tests/hooks: `spall-openapi/tests/stream_bound.rs`, `spall-cli/tests/stream_guard_e2e.rs`, pagination tests.
- Confidence: High.
- Evidence: `spall-openapi/src/lib.rs`, stream tests.

## Multipart Retries

- Area: `spall-cli/src/execute.rs`, `transport.rs`.
- Why sensitive: multipart form bodies are consumed by reqwest and cannot be retried blindly.
- Existing choices: CLI explorer reports multipart attempts are capped at one.
- Avoid: enabling generic retry loops for multipart without rebuilding form state.
- Tests/hooks: CLI e2e tests around multipart/output if present.
- Confidence: Medium.
- Evidence: `spall-cli` explorer report.

## MCP Tool Exposure

- Area: `spall-cli/src/mcp/*`, `docs/src/operations/mcp.md`.
- Why sensitive: large OpenAPI specs can expose more tools than MCP clients handle well.
- Existing choices: warning around client tool caps, include/exclude/max-tools support, HTTP body/session limits.
- Avoid: removing tool-count controls or writing diagnostics to stdout.
- Tests/hooks: `spall-cli/tests/mcp_e2e.rs`, `mcp_verbose_e2e.rs`.
- Confidence: High.
- Evidence: `spall-cli` and repo-level explorer reports.

## No Benchmarks Found

Discovery did not find a `benches/` directory or Criterion-style benchmark target. Treat performance claims as test-guarded or design-inferred unless a benchmark is added.
