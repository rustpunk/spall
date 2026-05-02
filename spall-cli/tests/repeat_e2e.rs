//! End-to-end tests for `--spall-repeat`.

use std::process::Command;
use tempfile::TempDir;
use wiremock::matchers::method;
use wiremock::{MockServer, ResponseTemplate};

fn bin_path() -> String {
    std::env::var("CARGO_BIN_EXE_spall").unwrap_or_else(|_| String::from("target/debug/spall"))
}

fn setup_api_config(temp: &TempDir, spec_path: &str) {
    let config_dir = temp.path().join("spall");
    let apis_dir = config_dir.join("apis");
    std::fs::create_dir_all(&apis_dir).unwrap();
    let api_toml = format!(r#"source = "{}""#, spec_path);
    std::fs::write(apis_dir.join("testapi.toml"), api_toml).unwrap();
}

fn minimal_spec(port: u16) -> String {
    format!(
        r#"{{
  "openapi": "3.0.0",
  "info": {{ "title": "Test", "version": "1.0.0" }},
  "servers": [{{ "url": "http://localhost:{}" }}],
  "paths": {{
    "/items": {{
      "get": {{
        "operationId": "get-items",
        "responses": {{ "200": {{ "description": "OK" }} }}
      }}
    }}
  }}
}}"#,
        port
    )
}

#[tokio::test]
async fn repeat_replays_most_recent_request() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec = minimal_spec(mock.address().port());
    let spec_path = temp.path().join("spec.json").to_str().unwrap().to_string();
    std::fs::write(&spec_path, &spec).unwrap();
    setup_api_config(&temp, &spec_path);

    // Seed the mock with two distinct responses so we can tell replay happened.
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let _cc = call_count.clone();

    wiremock::Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"call": 1})))
        .mount(&mock)
        .await;

    // First request.
    let _ = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .args(["testapi", "get-items"])
        .output()
        .expect("failed to run spall");

    // Clear and mount second response.
    mock.reset().await;
    wiremock::Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"call": 2})))
        .expect(1)
        .mount(&mock)
        .await;

    // Replay.
    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .args(["--spall-repeat"])
        .output()
        .expect("failed to run spall");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("call"),
        "Expected replay output, got: {}",
        stdout
    );
}

#[tokio::test]
async fn repeat_with_history_show_id() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec = minimal_spec(mock.address().port());
    let spec_path = temp.path().join("spec.json").to_str().unwrap().to_string();
    std::fs::write(&spec_path, &spec).unwrap();
    setup_api_config(&temp, &spec_path);

    wiremock::Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"n": 1})))
        .mount(&mock)
        .await;

    // Make first request (ID = 1).
    let _ = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .args(["testapi", "get-items"])
        .output()
        .expect("failed to run spall");

    // Mount different response.
    mock.reset().await;
    wiremock::Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"n": 99})))
        .expect(1)
        .mount(&mock)
        .await;

    // Replay specific ID.
    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .args(["history", "show", "1", "--spall-repeat"])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
