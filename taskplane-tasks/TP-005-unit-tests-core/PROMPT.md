# TP-005: Unit Tests for Core Resolver & Command Builder

**Area:** general **Priority:** P1

## Summary

Add unit tests for `spall-core`'s resolver, command builder, and tag grouping. Use small inline OpenAPI JSON strings to avoid external file dependencies.

## Steps

- [ ] In `spall-core/tests/resolver_test.rs` (or inline in `resolver.rs` with `#[cfg(test)]`):
  - Test `$ref` resolution with 1 level of nesting: `{"$ref": "#/components/schemas/Pet"}` resolves correctly
  - Test cycle detection: a schema referencing itself should return an error (not infinite loop)
  - Test depth limit: chain of 11 `$ref`s should hit `MAX_REF_DEPTH`
  - Test parameter merging: two params with same `(name, location)` should be deduplicated (later wins)
  - Test security inheritance: operation without security inherits path-level security
- [ ] In `spall-core/tests/command_test.rs` (or inline in `command.rs`):
  - Test single-tag flattening: operations register directly under root, not under a tag subcommand
  - Test multi-tag grouping: operations appear under both tag and root
  - Test positional arg generation for path params: `Arg::get_id()` should equal `path-{name}`
  - Test query flag generation: `--{name}` (not namespaced)
  - Test header flag generation: `--header-{kebab-name}`
  - Test body arg conflicts: `--data`, `--form`, `--field` are in a mutually exclusive group
- [ ] In `spall-core/src/command.rs`, add `#[cfg(test)]` helpers to build small specs programmatically for testing (avoid loading real files)
- [ ] Run `cargo test --workspace` and verify the new tests compile and pass
- [ ] Run `cargo clippy --workspace` and fix any warnings

## Acceptance Criteria

1. `cargo test --workspace` passes including all new tests
2. `cargo clippy --workspace` is clean
3. At least 8 new tests exist across resolver and command builder
4. Each test is deterministic and does not depend on external files or network
