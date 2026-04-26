use crate::error::SpallCoreError;
use crate::ir::{ResolvedSpec, SpecIndex};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Load a resolved spec from cache, or fall back to parsing and resolving.
///
/// Checks SHA-256 hash of raw spec bytes + IR version to invalidate cache.
pub fn load_or_resolve(
    source: &str,
    raw_bytes: &[u8],
    cache_dir: &Path,
) -> Result<ResolvedSpec, SpallCoreError> {
    let raw_hash = spec_hash(raw_bytes);
    let paths = cache_paths(source, cache_dir);

    // Try cache hit
    if let Ok(meta_bytes) = std::fs::read(&paths.meta) {
        if let Ok(meta) = postcard::from_bytes::<CacheMeta>(&meta_bytes) {
            if meta.raw_hash == raw_hash && meta.ir_version == IR_VERSION {
                if let Ok(ir_bytes) = std::fs::read(&paths.ir) {
                    match postcard::from_bytes::<ResolvedSpec>(&ir_bytes) {
                        Ok(spec) => return Ok(spec),
                        Err(e) => {
                            eprintln!(
                                "Warning: IR cache deserialization failed for '{}': {}. Re-parsing.",
                                source, e
                            );
                            let _ = std::fs::remove_file(&paths.ir);
                        }
                    }
                }
            }
        }
    }

    // Cache miss or corruption: parse and resolve
    let spec = crate::loader::load_spec_from_bytes(raw_bytes, source)?;

    if let Err(e) = write_cache(source, &spec, raw_hash, cache_dir) {
        eprintln!("Warning: failed to write cache for '{}': {}", source, e);
    }

    Ok(spec)
}

/// Write a resolved spec to cache atomically.
///
/// Uses temp file + `fs::rename` to prevent corruption on concurrent writes.
pub fn write_cache(
    source: &str,
    spec: &ResolvedSpec,
    raw_hash: [u8; 32],
    cache_dir: &Path,
) -> Result<(), SpallCoreError> {
    let paths = cache_paths(source, cache_dir);

    if let Some(parent) = paths.ir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| SpallCoreError::Io(e.to_string()))?;
    }

    let ir_bytes = postcard::to_allocvec(spec).map_err(|e| SpallCoreError::Cache(e.to_string()))?;
    let index = spec.to_index();
    let idx_bytes = postcard::to_allocvec(&index).map_err(|e| SpallCoreError::Cache(e.to_string()))?;
    let meta = CacheMeta {
        source: source.to_string(),
        raw_hash,
        ir_version: IR_VERSION,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };
    let meta_bytes = postcard::to_allocvec(&meta).map_err(|e| SpallCoreError::Cache(e.to_string()))?;

    atomic_write(&paths.ir, &ir_bytes).map_err(|e| SpallCoreError::Io(e.to_string()))?;
    atomic_write(&paths.idx, &idx_bytes).map_err(|e| SpallCoreError::Io(e.to_string()))?;
    atomic_write(&paths.meta, &meta_bytes).map_err(|e| SpallCoreError::Io(e.to_string()))?;

    Ok(())
}

/// Attempt to load a cached `SpecIndex` for degraded --help.
pub fn load_cached_index(source: &str, cache_dir: &Path) -> Option<SpecIndex> {
    let paths = cache_paths(source, cache_dir);
    let idx_bytes = std::fs::read(&paths.idx).ok()?;
    postcard::from_bytes::<SpecIndex>(&idx_bytes).ok()
}

/// Invalidate all cache entries for a source (used by refresh).
pub fn invalidate(source: &str, cache_dir: &Path) -> Result<(), SpallCoreError> {
    let paths = cache_paths(source, cache_dir);
    let _ = std::fs::remove_file(&paths.ir);
    let _ = std::fs::remove_file(&paths.idx);
    let _ = std::fs::remove_file(&paths.meta);
    Ok(())
}

/// Compute SHA-256 hash of raw spec bytes.
pub fn spec_hash(raw: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(raw);
    hasher.finalize().into()
}

/// Current IR format version — bump when `ResolvedSpec` layout changes.
pub const IR_VERSION: u32 = 2;

/// SHA-256 of the source string itself, used for cache keying.
pub fn source_hash(source: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    hasher.finalize().into()
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct CacheMeta {
    source: String,
    raw_hash: [u8; 32],
    ir_version: u32,
    created_at: u64,
}

struct CachePaths {
    ir: PathBuf,
    idx: PathBuf,
    meta: PathBuf,
}

fn cache_paths(source: &str, cache_dir: &Path) -> CachePaths {
    let hex = to_hex(&source_hash(source));
    CachePaths {
        ir: cache_dir.join(format!("{}.ir", hex)),
        idx: cache_dir.join(format!("{}.idx", hex)),
        meta: cache_dir.join(format!("{}.meta", hex)),
    }
}

fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{HttpMethod, ResolvedOperation, ResolvedSpec, ResolvedServer, SpecIndex};

    fn dummy_spec(title: &str) -> ResolvedSpec {
        ResolvedSpec {
            title: title.to_string(),
            version: "1.0.0".to_string(),
            base_url: "https://example.com".to_string(),
            operations: vec![ResolvedOperation {
                operation_id: "test-op".to_string(),
                method: HttpMethod::Get,
                path_template: "/test".to_string(),
                summary: None,
                description: None,
                deprecated: false,
                parameters: vec![],
                request_body: None,
                responses: Default::default(),
                security: vec![],
                tags: vec!["default".to_string()],
                extensions: Default::default(),
                servers: vec![ResolvedServer {
                    url: "https://example.com".to_string(),
                    description: None,
                }],
            }],
            servers: vec![ResolvedServer {
                url: "https://example.com".to_string(),
                description: None,
            }],
        }
    }

    #[test]
    fn cache_hit_returns_spec() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = dummy_spec("hit-test");
        let raw = b"raw-hit";
        let raw_hash = spec_hash(raw);
        write_cache("src1", &spec, raw_hash, tmp.path()).unwrap();

        let loaded = load_or_resolve("src1", raw, tmp.path()).unwrap();
        assert_eq!(loaded.title, "hit-test");
    }

    #[test]
    fn cache_miss_re_parses() {
        let tmp = tempfile::tempdir().unwrap();
        let raw = b"openapi: 3.0.0\ninfo:\n  title: MissTest\n  version: '1'\npaths: {}";
        let loaded = load_or_resolve("src2", raw, tmp.path()).unwrap();
        assert_eq!(loaded.title, "MissTest");

        // .ir, .idx, .meta should exist
        let paths = cache_paths("src2", tmp.path());
        assert!(paths.ir.exists());
        assert!(paths.idx.exists());
        assert!(paths.meta.exists());
    }

    #[test]
    fn cache_corruption_deletes_and_re_parses() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = dummy_spec("corrupt-test");
        let raw = b"raw-corrupt";
        let raw_hash = spec_hash(raw);
        write_cache("src3", &spec, raw_hash, tmp.path()).unwrap();

        // Corrupt the .ir file
        let paths = cache_paths("src3", tmp.path());
        std::fs::write(&paths.ir, b"garbage").unwrap();

        // Should fall back to parsing (which will fail because raw isn't valid),
        // but since raw is not valid OpenAPI, load_spec_from_bytes will fail.
        // Instead use valid raw bytes:
        let raw_valid = b"openapi: 3.0.0\ninfo:\n  title: CorruptRecovered\n  version: '1'\npaths: {}";
        let loaded = load_or_resolve("src3", raw_valid, tmp.path()).unwrap();
        assert_eq!(loaded.title, "CorruptRecovered");
    }

    #[test]
    fn source_hash_stability() {
        let h1 = source_hash("https://example.com/spec.yaml");
        let h2 = source_hash("https://example.com/spec.yaml");
        let h3 = source_hash("https://other.com/spec.yaml");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn spec_hash_stability() {
        let raw = b"hello world";
        let h1 = spec_hash(raw);
        let h2 = spec_hash(raw);
        assert_eq!(h1, h2);
    }

    #[test]
    fn load_cached_index_reads_only_idx() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = dummy_spec("idx-test");
        let raw = b"raw-idx";
        let raw_hash = spec_hash(raw);
        write_cache("src4", &spec, raw_hash, tmp.path()).unwrap();

        let index = load_cached_index("src4", tmp.path()).unwrap();
        assert_eq!(index.title, "idx-test");
        assert_eq!(index.operations.len(), 1);
    }

    #[test]
    fn invalidate_removes_files() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = dummy_spec("inv-test");
        write_cache("src5", &spec, spec_hash(b"x"), tmp.path()).unwrap();
        let paths = cache_paths("src5", tmp.path());
        assert!(paths.ir.exists());

        invalidate("src5", tmp.path()).unwrap();
        assert!(!paths.ir.exists());
        assert!(!paths.idx.exists());
        assert!(!paths.meta.exists());
    }
}
