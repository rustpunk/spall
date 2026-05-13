//! End-to-end test: a previously-stored OAuth2 access token is loaded from
//! disk and applied as `Authorization: Bearer ...` on the next request.
//!
//! This exercises the `auth::resolve` → `oauth2::ensure_fresh_token` →
//! `save_tokens`/`load_tokens` round-trip without driving the browser flow.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{MockServer, ResponseTemplate};

fn bin_path() -> String {
    std::env::var("CARGO_BIN_EXE_spall").unwrap_or_else(|_| String::from("target/debug/spall"))
}

fn minimal_spec(port: u16) -> String {
    format!(
        r#"{{
  "openapi": "3.0.0",
  "info": {{ "title": "Test", "version": "1.0.0" }},
  "servers": [{{ "url": "http://localhost:{}" }}],
  "paths": {{
    "/me": {{
      "get": {{
        "operationId": "get-me",
        "responses": {{ "200": {{ "description": "OK" }} }}
      }}
    }}
  }}
}}"#,
        port
    )
}

#[tokio::test]
async fn stored_oauth2_access_token_is_used_for_bearer() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();

    let spec_path = temp.path().join("spec.json");
    std::fs::write(&spec_path, minimal_spec(mock.address().port())).unwrap();

    let apis_dir = temp.path().join("spall").join("apis");
    std::fs::create_dir_all(&apis_dir).unwrap();
    std::fs::write(
        apis_dir.join("idp.toml"),
        format!(
            r#"source = "{}"

[auth]
kind = "oauth2"
client_id = "test-client"
auth_url = "https://idp.example/authorize"
token_url = "https://idp.example/token"
"#,
            spec_path.display()
        ),
    )
    .unwrap();

    // Write a token file directly into the spall cache dir under the temp HOME.
    let cache_dir = temp.path().join(".cache").join("spall").join("oauth2");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let tokens = serde_json::json!({
        "access_token": "stored-access-tkn",
        "refresh_token": "stored-refresh-tkn",
        "expires_at": now + 3600,
        "token_url": "https://idp.example/token",
        "client_id": "test-client"
    });
    std::fs::write(
        cache_dir.join("idp.json"),
        serde_json::to_vec_pretty(&tokens).unwrap(),
    )
    .unwrap();

    // Server should receive the stored token.
    wiremock::Mock::given(method("GET"))
        .and(path("/me"))
        .and(header("Authorization", "Bearer stored-access-tkn"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"ok\":true}"))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        // dirs::cache_dir() on Linux honors XDG_CACHE_HOME.
        .env("XDG_CACHE_HOME", temp.path().join(".cache"))
        .args(["idp", "get-me"])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
}

#[tokio::test]
async fn no_stored_token_means_oauth2_kind_is_none() {
    // When no token file exists, resolution falls through to None and the
    // request goes out without Authorization. wiremock will accept anyway
    // because we don't constrain on the header.

    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();

    let spec_path = temp.path().join("spec.json");
    std::fs::write(&spec_path, minimal_spec(mock.address().port())).unwrap();

    let apis_dir = temp.path().join("spall").join("apis");
    std::fs::create_dir_all(&apis_dir).unwrap();
    std::fs::write(
        apis_dir.join("idp.toml"),
        format!(
            r#"source = "{}"

[auth]
kind = "oauth2"
client_id = "test-client"
auth_url = "https://idp.example/authorize"
token_url = "https://idp.example/token"
"#,
            spec_path.display()
        ),
    )
    .unwrap();

    wiremock::Mock::given(method("GET"))
        .and(path("/me"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"ok\":true}"))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join(".cache"))
        .args(["idp", "get-me"])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "expected unauthenticated request to still succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}
