use crate::SpallCliError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const RAW_CACHE_TTL_SECS: u64 = 3600;

/// Load raw spec bytes. For URLs, checks TTL cache and conditional GET.
pub async fn load_raw(source: &str, cache_dir: &Path) -> Result<Vec<u8>, SpallCliError> {
    if source.starts_with("http://") || source.starts_with("https://") {
        fetch_url(source, cache_dir).await
    } else {
        std::fs::read(source).map_err(|e| SpallCliError::Network(e.to_string()))
    }
}

/// Force network re-fetch of a URL source, update raw cache, invalidate IR cache.
pub async fn refresh(source: &str, cache_dir: &Path) -> Result<Vec<u8>, SpallCliError> {
    if !source.starts_with("http://") && !source.starts_with("https://") {
        return Err(SpallCliError::Usage(format!(
            "refresh only applies to remote specs: {}",
            source
        )));
    }
    let bytes = fetch_url_force(source, cache_dir).await?;
    let _ = spall_core::cache::invalidate(source, cache_dir);
    Ok(bytes)
}

async fn fetch_url(source: &str, cache_dir: &Path) -> Result<Vec<u8>, SpallCliError> {
    if let Some(bytes) = read_raw_cache(source, cache_dir) {
        if let Ok(meta_bytes) = std::fs::read(raw_meta_path(source, cache_dir)) {
            if let Ok(meta) = postcard::from_bytes::<RawMeta>(&meta_bytes) {
                let now = now_secs();
                if now < meta.expires_at {
                    return Ok(bytes);
                }
                return conditional_get(source, cache_dir, &meta).await;
            }
        }
        return fetch_url_force(source, cache_dir).await;
    }
    fetch_url_force(source, cache_dir).await
}

async fn conditional_get(
    source: &str,
    cache_dir: &Path,
    meta: &RawMeta,
) -> Result<Vec<u8>, SpallCliError> {
    let client = reqwest::Client::new();
    let mut req = client.get(source);
    if let Some(etag) = &meta.etag {
        req = req.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    match req.send().await {
        Ok(resp) if resp.status() == reqwest::StatusCode::NOT_MODIFIED => {
            let new_meta = RawMeta {
                etag: meta.etag.clone(),
                fetched_at: meta.fetched_at,
                expires_at: now_secs() + RAW_CACHE_TTL_SECS,
            };
            let _ = write_raw_meta(source, cache_dir, &new_meta);
            read_raw_cache(source, cache_dir)
                .ok_or_else(|| SpallCliError::Network("cached raw bytes disappeared".to_string()))
        }
        Ok(resp) if resp.status().is_success() => {
            let etag = extract_etag(&resp);
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| SpallCliError::Network(e.to_string()))?
                .to_vec();
            write_raw_cache(source, cache_dir, &bytes, etag.as_deref())?;
            Ok(bytes)
        }
        Ok(resp) => stale_fallback(source, cache_dir, format!("HTTP {}", resp.status())).await,
        Err(e) => stale_fallback(source, cache_dir, e.to_string()).await,
    }
}

async fn fetch_url_force(source: &str, cache_dir: &Path) -> Result<Vec<u8>, SpallCliError> {
    let client = reqwest::Client::new();
    match client.get(source).send().await {
        Ok(resp) if resp.status().is_success() => {
            let etag = extract_etag(&resp);
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| SpallCliError::Network(e.to_string()))?
                .to_vec();
            write_raw_cache(source, cache_dir, &bytes, etag.as_deref())?;
            Ok(bytes)
        }
        Ok(resp) => {
            stale_fallback(source, cache_dir, format!("HTTP {}", resp.status())).await
        }
        Err(e) => stale_fallback(source, cache_dir, e.to_string()).await,
    }
}

fn extract_etag(resp: &reqwest::Response) -> Option<String> {
    resp.headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

async fn stale_fallback(
    source: &str,
    cache_dir: &Path,
    reason: String,
) -> Result<Vec<u8>, SpallCliError> {
    if let Some(bytes) = read_raw_cache(source, cache_dir) {
        eprintln!(
            "Warning: failed to fetch '{}': {}. Using stale cached copy.",
            source, reason
        );
        Ok(bytes)
    } else {
        Err(SpallCliError::Network(reason))
    }
}

fn read_raw_cache(source: &str, cache_dir: &Path) -> Option<Vec<u8>> {
    std::fs::read(raw_path(source, cache_dir)).ok()
}

fn write_raw_cache(
    source: &str,
    cache_dir: &Path,
    bytes: &[u8],
    etag: Option<&str>,
) -> Result<(), SpallCliError> {
    let meta = RawMeta {
        etag: etag.map(|s| s.to_string()),
        fetched_at: now_secs(),
        expires_at: now_secs() + RAW_CACHE_TTL_SECS,
    };
    let meta_bytes = postcard::to_allocvec(&meta)
        .map_err(|e| SpallCliError::Cache(e.to_string()))?;
    atomic_write(&raw_path(source, cache_dir), bytes)?;
    atomic_write(&raw_meta_path(source, cache_dir), &meta_bytes)?;
    Ok(())
}

fn write_raw_meta(
    source: &str,
    cache_dir: &Path,
    meta: &RawMeta,
) -> Result<(), SpallCliError> {
    let meta_bytes = postcard::to_allocvec(meta)
        .map_err(|e| SpallCliError::Cache(e.to_string()))?;
    atomic_write(&raw_meta_path(source, cache_dir), &meta_bytes)?;
    Ok(())
}

fn raw_path(source: &str, cache_dir: &Path) -> PathBuf {
    let hash = spall_core::cache::source_hash(source);
    let hex = to_hex(&hash);
    cache_dir.join(format!("{}.raw", hex))
}

fn raw_meta_path(source: &str, cache_dir: &Path) -> PathBuf {
    let hash = spall_core::cache::source_hash(source);
    let hex = to_hex(&hash);
    cache_dir.join(format!("{}.raw-meta", hex))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn atomic_write(path: &Path, data: &[u8]) -> Result<(), SpallCliError> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data).map_err(|e| SpallCliError::Network(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| SpallCliError::Network(e.to_string()))?;
    Ok(())
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct RawMeta {
    etag: Option<String>,
    fetched_at: u64,
    expires_at: u64,
}
