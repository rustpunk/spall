//! Request preview: print resolved request details without sending.

use reqwest::header::HeaderMap;

/// Print a framed request summary to stderr.
pub fn print_preview(method: &str, url: &str, headers: &HeaderMap, body: Option<&[u8]>) {
    eprintln!("┌─ Request Preview");
    eprintln!("│ {} {}", method, url);
    for (k, v) in headers.iter() {
        let val = if is_sensitive(k) {
            "[REDACTED]".to_string()
        } else {
            v.to_str().unwrap_or("?").to_string()
        };
        eprintln!("│ {}: {}", k, val);
    }
    if let Some(b) = body {
        let preview = String::from_utf8_lossy(b);
        let truncated = if preview.len() > 512 {
            &preview[..512]
        } else {
            &preview
        };
        eprintln!("│ Body ({} bytes):", b.len());
        for line in truncated.lines() {
            eprintln!("│   {}", line);
        }
        if preview.len() > 512 {
            eprintln!("│   ... (truncated)");
        }
    }
    eprintln!("└─ End Preview");
}

/// Return true for headers likely to contain secrets.
fn is_sensitive(name: &reqwest::header::HeaderName) -> bool {
    let lower = name.as_str().to_ascii_lowercase();
    lower.contains("auth")
        || lower.contains("cookie")
        || lower.contains("token")
        || lower.contains("key")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_detection() {
        assert!(is_sensitive(&reqwest::header::AUTHORIZATION));
        assert!(is_sensitive(&reqwest::header::COOKIE));
        assert!(is_sensitive(
            &reqwest::header::HeaderName::from_static("x-api-key")
        ));
        assert!(!is_sensitive(&reqwest::header::CONTENT_TYPE));
    }
}
