//! Source-description loading helpers.
//!
//! This module is *pure* — it doesn't do I/O. The caller (in spall-cli) is
//! responsible for fetching the raw bytes (via local file read or HTTP). The
//! helper here delegates to the existing IR cache so repeat runs are fast.

use crate::cache::load_or_resolve;
use crate::error::SpallCoreError;
use crate::ir::ResolvedSpec;
use std::path::Path;

/// Resolve raw OpenAPI bytes for a named source description into a fully
/// resolved `ResolvedSpec`, going through the standard IR cache.
///
/// `source` is the spec's original URL/path string (used as the cache key);
/// `raw_bytes` are the spec bytes already fetched by the caller.
///
/// Errors propagate `SpallCoreError` unchanged so the CLI layer can wrap
/// them with `miette` context.
#[must_use = "the resolved spec is the only useful output"]
pub fn resolve_source_from_bytes(
    source: &str,
    raw_bytes: &[u8],
    cache_dir: &Path,
) -> Result<ResolvedSpec, SpallCoreError> {
    load_or_resolve(source, raw_bytes, cache_dir)
}
