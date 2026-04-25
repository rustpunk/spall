# TP-003: Add ResolvedSpec.servers Field & Fix build_url

**Area:** general **Priority:** P1

## Summary

Add `servers` to `ResolvedSpec` and `ResolvedOperation` so the CLI can construct the correct base URL from the OpenAPI spec's `servers` array, not just from `--spall-server` or config `base_url`.

## Steps

- [ ] In `spall-core/src/ir.rs`, add:
  ```rust
  pub struct ResolvedServer {
      pub url: String,
      pub description: Option<String>,
  }
  ```
  And add `pub servers: Vec<ResolvedServer>` to both `ResolvedSpec` and `ResolvedOperation`.
- [ ] In `spall-core/src/resolver.rs`, populate `servers` during resolution:
  - From `OpenAPI.servers` at spec level
  - From `PathItem.servers` at path level (override spec-level)
  - From `Operation.servers` at operation level (highest priority)
  - If none, default to `[ResolvedServer { url: "/".to_string(), description: None }]`
- [ ] In `spall-cli/src/execute.rs`, update `build_url()`:
  - Priority order: `entry.base_url` (config) > `--spall-server` flag > `op.servers.first()` > `spec.servers.first()` > "/"
  - Keep existing `{name}` and `{name*}` placeholder replacement for path params
- [ ] Add a helper `resolve_server_url(spec_servers, op_servers)` for clarity
- [ ] Verify the URL construction works with the Petstore spec (it declares `servers: [{"url": "https://petstore3.swagger.io/api/v3"}]`)
- [ ] Run `cargo clippy --workspace` and fix any warnings
- [ ] Run `cargo test --workspace` to verify tests pass
- [ ] End-to-end test: `cargo run --bin spall -- petstore getpetbyid 1` (without `--spall-server`) should hit the correct URL

## Acceptance Criteria

1. `cargo test --workspace` passes
2. `cargo clippy --workspace` is clean
3. `spall petstore getpetbyid 1` (no `--spall-server`) successfully resolves to `https://petstore3.swagger.io/api/v3/pet/1`
4. The priority order for URL resolution is implemented exactly as specified above
