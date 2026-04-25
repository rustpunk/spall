# TP-001: Implement IR Cache Core

**Area:** general **Priority:** P0

## Summary

Implement spec caching in `spall-core/src/cache.rs` to eliminate repeated OpenAPI parsing/resolution overhead. After this task, `load_or_resolve` and `write_cache` must be real implementations (not `todo!`), and `load_cached_index` must return a lightweight `SpecIndex` for degraded `--help`.

## Steps

- [ ] Choose cache path strategy. Use `dirs::cache_dir()` → `spall/<hash_prefix>/spec.postcard`. Add `dirs = "5"` to `spall-core/Cargo.toml`.
- [ ] Define cache structures in `spall-core/src/ir.rs`:
  - `CachedSpec { ir_version: u32, source_hash: [u8; 32], spec: ResolvedSpec }`
  - `SpecIndex { title: String, version: String, operations: Vec<SpecIndexOp> }`
  - `SpecIndexOp { operation_id: String, method: String, path: String, summary: Option<String>, tags: Vec<String>, params: Vec<ParamIndex> }`
  - `ParamIndex { name: String, location: ParameterLocation, required: bool }`
- [ ] Implement `write_cache(source, spec, raw_hash)` in `cache.rs`:
  - `postcard::to_stdvec(&CachedSpec)` to serialize
  - Atomic write: write to `.tmp` file, then `std::fs::rename`
  - Create parent directories if needed
- [ ] Implement `load_or_resolve(source, raw_bytes)` in `cache.rs`:
  - Compute SHA-256 hash via `sha2::Sha256`
  - Build cache path from hash prefix
  - If cache exists, deserialize and validate `ir_version == IR_VERSION` and `source_hash` match
  - On hit → return `CachedSpec.spec`
  - On miss/corruption → return `None` (caller does parse+resolve, then calls `write_cache`)
- [ ] Implement `load_cached_index(source)` in `cache.rs`:
  - Look for cached `CachedSpec`, extract `spec.into_index()` or build index on the fly
  - Return `Option<SpecIndex>`
  - `ResolvedSpec` must have an `into_index()` helper method
- [ ] Add `ResolvedSpec::into_index(&self) -> SpecIndex` helper in `ir.rs`
- [ ] Add tests in `spall-core/src/cache.rs` for hit, miss, and corruption cases
- [ ] Run `cargo clippy --workspace` and fix any warnings
- [ ] Run `cargo test --workspace` to verify tests pass

## Acceptance Criteria

1. `cargo test --workspace` passes including new cache tests
2. `cargo clippy --workspace` is clean
3. `cache::load_or_resolve` does NOT panic (currently `todo!()`)
4. `cache::write_cache` creates `.postcard` files on disk
5. `cache::load_cached_index` returns `Some(...)` for any cache hit
