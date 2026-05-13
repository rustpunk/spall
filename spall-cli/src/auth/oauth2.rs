//! OAuth2 Authorization Code + PKCE flow and access-token lifecycle.
//!
//! Tokens (access + refresh + `expires_at`) are persisted under
//! `$XDG_CACHE_HOME/spall/oauth2/<api>.json` with `0600` permissions. They
//! never enter the IR cache or the hasp store — they are session state
//! owned by spall.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{distributions::Alphanumeric, Rng};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spall_config::auth::AuthConfig;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 30-second skew applied when checking access-token expiry so we never
/// inject a token that's about to expire mid-request.
const EXPIRY_SKEW_SECS: u64 = 30;

/// Inject an OAuth2 access token as `Authorization: Bearer <token>`.
pub fn apply(token: &SecretString, headers: &mut HeaderMap) {
    let value = format!("Bearer {}", token.expose_secret());
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("invalid")),
    );
}

/// Tokens persisted to disk after a successful authorization-code exchange
/// or refresh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Seconds since `UNIX_EPOCH` when `access_token` stops being valid.
    pub expires_at: u64,
    /// Original `token_url` — kept so refresh can re-target without re-reading config.
    pub token_url: String,
    /// Original `client_id` — kept for refresh-grant body.
    pub client_id: String,
}

impl OAuthTokens {
    fn is_expired_now(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now + EXPIRY_SKEW_SECS >= self.expires_at
    }
}

// ---------------------------------------------------------------------------
// Token-file storage
// ---------------------------------------------------------------------------

fn token_dir() -> PathBuf {
    dirs::cache_dir()
        .map(|d| d.join("spall").join("oauth2"))
        .unwrap_or_else(|| spall_config::sources::config_dir().join("oauth2"))
}

fn token_path(api_name: &str) -> PathBuf {
    token_dir().join(format!("{}.json", api_name))
}

/// Persist tokens to disk, creating the directory if necessary.
///
/// On Unix the file is chmod-ed to `0600` so other local users cannot read it.
pub fn save_tokens(api_name: &str, tokens: &OAuthTokens) -> Result<(), crate::SpallCliError> {
    let dir = token_dir();
    std::fs::create_dir_all(&dir).map_err(|e| {
        crate::SpallCliError::AuthResolution {
            api: api_name.to_string(),
            message: format!("create token dir: {}", e),
        }
    })?;
    let path = token_path(api_name);
    let json = serde_json::to_vec_pretty(tokens).map_err(|e| {
        crate::SpallCliError::AuthResolution {
            api: api_name.to_string(),
            message: format!("serialize tokens: {}", e),
        }
    })?;
    std::fs::write(&path, json).map_err(|e| crate::SpallCliError::AuthResolution {
        api: api_name.to_string(),
        message: format!("write token file: {}", e),
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Load tokens from disk. Returns `None` when no tokens are stored.
pub fn load_tokens(api_name: &str) -> Option<OAuthTokens> {
    let path = token_path(api_name);
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

// ---------------------------------------------------------------------------
// PKCE helpers (RFC 7636)
// ---------------------------------------------------------------------------

/// A PKCE verifier + S256 challenge pair.
#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

/// Generate a fresh PKCE pair with a 64-char URL-safe verifier and an
/// `S256`-derived challenge.
#[must_use]
pub fn generate_pkce() -> Pkce {
    let verifier: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();
    let challenge = challenge_s256(&verifier);
    Pkce {
        verifier,
        challenge,
    }
}

/// Derive an `S256` PKCE challenge from a verifier.
#[must_use]
pub fn challenge_s256(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

// ---------------------------------------------------------------------------
// Authorization-code flow
// ---------------------------------------------------------------------------

/// Run the full Authorization Code + PKCE flow:
///
/// 1. Bind a TCP listener on a random local port.
/// 2. Construct the authorization URL and print it (also try to open in browser).
/// 3. Wait for the browser callback, extract `code` + verify `state`.
/// 4. Exchange the code at `token_url` with the PKCE verifier.
/// 5. Persist the resulting tokens to disk.
///
/// On success the user is told to re-run the command they were trying to run.
pub async fn run_login(api_name: &str, cfg: &AuthConfig) -> Result<(), crate::SpallCliError> {
    let client_id = cfg
        .client_id
        .as_deref()
        .ok_or_else(|| usage_err(api_name, "`auth.client_id` is required for OAuth2 login"))?;
    let auth_url = cfg
        .auth_url
        .as_deref()
        .ok_or_else(|| usage_err(api_name, "`auth.auth_url` is required for OAuth2 login"))?;
    let token_url = cfg
        .token_url
        .as_deref()
        .ok_or_else(|| usage_err(api_name, "`auth.token_url` is required for OAuth2 login"))?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(|e| {
        crate::SpallCliError::Network(format!("bind redirect listener: {}", e))
    })?;
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let redirect_uri = format!("http://127.0.0.1:{}/callback", port);

    let pkce = generate_pkce();
    let state: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();

    let scopes_str = cfg
        .scopes
        .as_ref()
        .map(|s| s.join(" "))
        .unwrap_or_default();
    let auth_url_full = build_authorize_url(
        auth_url,
        client_id,
        &redirect_uri,
        &scopes_str,
        &pkce.challenge,
        &state,
    );

    eprintln!("Open the following URL in your browser to authorize spall:");
    eprintln!();
    eprintln!("    {}", auth_url_full);
    eprintln!();
    eprintln!("Waiting for the authorization callback on 127.0.0.1:{} ...", port);

    let (code, callback_state) = wait_for_callback(&listener).await?;

    if callback_state != state {
        return Err(crate::SpallCliError::AuthResolution {
            api: api_name.to_string(),
            message: "state mismatch in OAuth2 callback (possible CSRF)".to_string(),
        });
    }

    let tokens = exchange_code(
        api_name,
        token_url,
        client_id,
        &redirect_uri,
        &code,
        &pkce.verifier,
    )
    .await?;

    save_tokens(api_name, &tokens)?;
    eprintln!("Successfully signed in to '{}'. Tokens stored locally.", api_name);
    Ok(())
}

fn build_authorize_url(
    auth_url: &str,
    client_id: &str,
    redirect_uri: &str,
    scopes: &str,
    challenge: &str,
    state: &str,
) -> String {
    let sep = if auth_url.contains('?') { '&' } else { '?' };
    let mut url = format!(
        "{}{}response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state={}",
        auth_url,
        sep,
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(challenge),
        urlencoding::encode(state),
    );
    if !scopes.is_empty() {
        url.push_str("&scope=");
        url.push_str(&urlencoding::encode(scopes));
    }
    url
}

/// Accept exactly one HTTP request on the listener, parse it as
/// `GET /callback?code=...&state=...`, write a tiny HTML "you can close
/// this window" response, and return `(code, state)`.
async fn wait_for_callback(
    listener: &tokio::net::TcpListener,
) -> Result<(String, String), crate::SpallCliError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (mut stream, _addr) = listener.accept().await.map_err(|e| {
        crate::SpallCliError::Network(format!("accept callback: {}", e))
    })?;

    // Read until the end of the request line / headers (small buffer is fine).
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await.map_err(|e| {
        crate::SpallCliError::Network(format!("read callback: {}", e))
    })?;
    let req = String::from_utf8_lossy(&buf[..n]);

    let body = b"<!doctype html><meta charset=\"utf-8\"><title>spall</title>\
                 <body style=\"font-family:sans-serif;padding:2em\">\
                 <h1>Signed in</h1><p>You can close this window.</p></body>";
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.write_all(body).await;
    let _ = stream.shutdown().await;

    let first_line = req.lines().next().unwrap_or("");
    let query = first_line
        .split_whitespace()
        .nth(1)
        .and_then(|path| path.split_once('?'))
        .map(|(_, q)| q)
        .ok_or_else(|| crate::SpallCliError::AuthResolution {
            api: "oauth2".to_string(),
            message: "callback request had no query string".to_string(),
        })?;

    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            let decoded = urlencoding::decode(v).map(|c| c.into_owned()).ok();
            match k {
                "code" => code = decoded,
                "state" => state = decoded,
                _ => {}
            }
        }
    }
    match (code, state) {
        (Some(c), Some(s)) => Ok((c, s)),
        _ => Err(crate::SpallCliError::AuthResolution {
            api: "oauth2".to_string(),
            message: format!("callback missing code or state: {}", query),
        }),
    }
}

async fn exchange_code(
    api_name: &str,
    token_url: &str,
    client_id: &str,
    redirect_uri: &str,
    code: &str,
    verifier: &str,
) -> Result<OAuthTokens, crate::SpallCliError> {
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", verifier),
    ];
    post_token(api_name, token_url, &params, client_id).await
}

/// Refresh an expired access token using the stored `refresh_token`.
pub async fn refresh(
    api_name: &str,
    tokens: &OAuthTokens,
) -> Result<OAuthTokens, crate::SpallCliError> {
    let refresh_token = tokens.refresh_token.as_deref().ok_or_else(|| {
        crate::SpallCliError::AuthResolution {
            api: api_name.to_string(),
            message: "no refresh_token stored; run `spall auth login` again".to_string(),
        }
    })?;
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", tokens.client_id.as_str()),
    ];
    let mut fresh = post_token(api_name, &tokens.token_url, &params, &tokens.client_id).await?;
    // Many servers omit refresh_token on refresh; keep the original.
    if fresh.refresh_token.is_none() {
        fresh.refresh_token = tokens.refresh_token.clone();
    }
    Ok(fresh)
}

async fn post_token(
    api_name: &str,
    token_url: &str,
    params: &[(&str, &str)],
    client_id: &str,
) -> Result<OAuthTokens, crate::SpallCliError> {
    let body = params
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    let client = crate::http::build_fetch_client(crate::http::resolve_env_proxy().as_deref())
        .map_err(|e| crate::SpallCliError::Network(e.to_string()))?;
    let resp = client
        .post(token_url)
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(reqwest::header::ACCEPT, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| crate::SpallCliError::Network(format!("token request: {}", e)))?;

    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| crate::SpallCliError::Network(e.to_string()))?;
    if !status.is_success() {
        return Err(crate::SpallCliError::AuthResolution {
            api: api_name.to_string(),
            message: format!(
                "token endpoint returned {}: {}",
                status,
                String::from_utf8_lossy(&bytes)
            ),
        });
    }

    let json: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        crate::SpallCliError::AuthResolution {
            api: api_name.to_string(),
            message: format!("parse token response: {}", e),
        }
    })?;

    let access_token = json
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| crate::SpallCliError::AuthResolution {
            api: api_name.to_string(),
            message: "token response missing access_token".to_string(),
        })?
        .to_string();
    let refresh_token = json
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(String::from);
    let expires_in = json
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(3600);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(OAuthTokens {
        access_token,
        refresh_token,
        expires_at: now + expires_in,
        token_url: token_url.to_string(),
        client_id: client_id.to_string(),
    })
}

fn usage_err(api: &str, msg: &str) -> crate::SpallCliError {
    crate::SpallCliError::Usage(format!("OAuth2 login for '{}': {}", api, msg))
}

// ---------------------------------------------------------------------------
// Resolution-time helper
// ---------------------------------------------------------------------------

/// Return a fresh access token for `api_name`, refreshing on the fly if
/// the cached one has expired (or is within the skew window).
///
/// Returns `None` when no tokens are on disk yet (the user hasn't run
/// `spall auth login`).
pub async fn ensure_fresh_token(api_name: &str) -> Result<Option<SecretString>, crate::SpallCliError> {
    let Some(tokens) = load_tokens(api_name) else {
        return Ok(None);
    };
    if !tokens.is_expired_now() {
        return Ok(Some(SecretString::new(tokens.access_token.clone().into())));
    }
    let fresh = refresh(api_name, &tokens).await?;
    save_tokens(api_name, &fresh)?;
    Ok(Some(SecretString::new(fresh.access_token.into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_s256_matches_rfc7636_test_vector() {
        // From RFC 7636 Appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = challenge_s256(verifier);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn generated_pkce_pair_round_trips() {
        let p = generate_pkce();
        assert!(p.verifier.len() >= 43 && p.verifier.len() <= 128);
        assert_eq!(p.challenge, challenge_s256(&p.verifier));
    }

    #[test]
    fn tokens_serialize_round_trip() {
        let t = OAuthTokens {
            access_token: "AT".to_string(),
            refresh_token: Some("RT".to_string()),
            expires_at: 1_700_000_000,
            token_url: "https://idp.example.com/token".to_string(),
            client_id: "client-id".to_string(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: OAuthTokens = serde_json::from_str(&json).unwrap();
        assert_eq!(back.access_token, "AT");
        assert_eq!(back.refresh_token, Some("RT".to_string()));
    }

    #[test]
    fn expiry_skew_treats_near_expiry_as_expired() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let t = OAuthTokens {
            access_token: "x".to_string(),
            refresh_token: None,
            expires_at: now + 10, // within 30-second skew
            token_url: String::new(),
            client_id: String::new(),
        };
        assert!(t.is_expired_now());
    }

    #[test]
    fn build_authorize_url_appends_query_separator_correctly() {
        let url = build_authorize_url(
            "https://idp/auth",
            "cid",
            "http://127.0.0.1:9999/callback",
            "repo user",
            "challenge",
            "state",
        );
        assert!(url.starts_with("https://idp/auth?response_type=code"));
        assert!(url.contains("scope=repo%20user"));
        assert!(url.contains("code_challenge=challenge"));

        let url2 = build_authorize_url(
            "https://idp/auth?prompt=login",
            "cid",
            "http://x/cb",
            "",
            "ch",
            "st",
        );
        assert!(url2.contains("?prompt=login&response_type=code"));
    }
}
