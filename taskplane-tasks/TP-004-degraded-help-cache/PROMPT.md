# TP-004: Degraded --help from Cache in CLI

**Area:** general **Priority:** P1
**Depends on:** TP-002

## Summary

Enable `spall <api> --help` and `spall <api> <op> --help` to work even when the spec file is temporarily unavailable, by using the cached `SpecIndex`.

## Steps

- [ ] In `spall-core/src/command.rs`, add `build_operations_cmd_from_index(api_name: &str, index: &SpecIndex) -> Command`:
  - Group operations by tag (single-tag flattening same as `build_operations_cmd`)
  - Build `Arg` for each operation using `ParamIndex` (name, location, required)
  - Add body args (`--data`, `--form`, `--field`, `--no-data`) based on whether the operation list suggests bodies exist (Wave 1: always add them if any op in the index had a body)
  - Add `--help` for individual operations
- [ ] Refactor shared logic between `build_operations_cmd` and `build_operations_cmd_from_index` into a private helper that takes an iterator of operation descriptors
- [ ] In `spall-cli/src/main.rs`, update `show_api_help`:
  - If spec load fails, try `spall_core::cache::load_cached_index(&entry.source)`
  - If cached index available, build phase2 from index and print command tree help
  - Print warning: `Warning: spec unavailable; showing cached operations (may be stale).`
- [ ] In `spall-cli/src/main.rs`, update `handle_api_operation`:
  - If spec load fails, try cached index for degraded help listing
  - Only return an error if both spec load AND cached index fail
- [ ] Run `cargo clippy --workspace` and fix any warnings
- [ ] Run `cargo test --workspace` to verify tests pass
- [ ] Manual test: rename the petstore spec file temporarily; `spall petstore --help` should still show commands (with stale warning)

## Acceptance Criteria

1. `cargo test --workspace` passes
2. `cargo clippy --workspace` is clean
3. With spec file missing, `spall petstore --help` still prints operation tree
4. Degraded help prints a visible warning about stale data
