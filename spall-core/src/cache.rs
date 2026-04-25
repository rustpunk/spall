use crate::error::SpallCoreError;
use crate::ir::{ResolvedSpec, SpecIndex};
use sha2::{Digest, Sha256};

/// Load a resolved spec from cache, or fall back to parsing and resolving.
///
/// Checks SHA-256 hash of raw spec bytes + IR version to invalidate cache.
/// Wave 1.5: implement cache hit/miss logic.
pub fn load_or_resolve(
    _source: &str,
    _raw_bytes: &[u8],
) -> Result<ResolvedSpec, SpallCoreError> {
    // TODO(Wave 1.5): check cache, hash invalidation, serialize/deserialize.
    todo!("load_or_resolve")
}

/// Write a resolved spec to cache atomically.
///
/// Uses temp file + `fs::rename` to prevent corruption on concurrent writes.
pub fn write_cache(
    _source: &str,
    _spec: &ResolvedSpec,
    _raw_hash: [u8; 32],
) -> Result<(), SpallCoreError> {
    // TODO(Wave 1.5): serialize to temp file and atomic rename.
    todo!("write_cache")
}

/// Compute SHA-256 hash of raw spec bytes.
pub fn spec_hash(raw: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(raw);
    hasher.finalize().into()
}

/// Current IR format version — bump when `ResolvedSpec` layout changes.
pub const IR_VERSION: u32 = 1;

/// Attempt to load a cached `SpecIndex` for degraded --help.
pub fn load_cached_index(_source: &str) -> Option<SpecIndex> {
    // TODO(Wave 1.5): attempt to load lightweight index from cache.
    None
}
