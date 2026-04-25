# TP-002: Wire Cache into Loader & Build SpecIndex

**Area:** general **Priority:** P0
**Depends on:** TP-001

## Summary

Wire the IR cache into `spall-core/src/loader.rs` so that `load_spec` transparently uses cached resolved specs on repeat invocations. Also add `SpecIndex` definition to `spall-core/src/ir.rs` for degraded `--help`.

## Steps

- [ ] Add `pub fn load_spec(source: &str) -> Result<ResolvedSpec, SpallCoreError>` function to `loader.rs` that:
  1. Reads raw bytes from the source path
  2. Calls `cache::load_or_resolve(source, &raw_bytes)`
  3. If cache hit (Some), return the spec directly
  4. If cache miss, parse JSON/YAML, resolve `$ref`, then call `cache::write_cache(source, &spec, raw_hash)`
- [ ] Move the existing `load_spec` logic (from main.rs or wherever it was inline) into `loader.rs` as `parse_and_resolve_spec(raw_bytes)` helper
- [ ] In `spall-cli/src/main.rs`, replace any inline spec loading with `spall_core::loader::load_spec(&entry.source)`
- [ ] In `spall-core/src/ir.rs`, add `#[derive(Clone, Debug, Serialize, Deserialize)]` `SpecIndex` and `SpecIndexOp` structs with all fields needed for degraded help
- [ ] Add `impl ResolvedSpec { pub fn to_index(&self) -> SpecIndex { ... } }`
- [ ] Ensure `spall-core/src/cache.rs` can deserialize `CachedSpec` containing `ResolvedSpec` — `postcard` requires all types in `ResolvedSpec` to implement `Serialize + Deserialize` (they should via existing derives)
- [ ] Run `cargo clippy --workspace` and fix any warnings
- [ ] Run `cargo test --workspace` to verify tests pass
- [ ] Verify: running `spall petstore getpetbyid 1` twice should be faster on the second run (check with `time`)

## Acceptance Criteria

1. `cargo test --workspace` passes
2. `cargo clippy --workspace` is clean
3. Second invocation of a spec-based command is measurably faster (no re-parse)
4. `spec.postcard` cache files exist in the system cache directory after first run
