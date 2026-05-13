//! End-to-end test for `Retry-After` honoring on 429/503.

use std::process::Command;
use std::time::Instant;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

fn install_api(temp: &TempDir, spec_path: &str) {
    let apis_dir = temp.path().join("spall").join("apis");
    std::fs::create_dir_all(&apis_dir).unwrap();
    std::fs::write(
        apis_dir.join("testapi.toml"),
        format!(r#"source = "{}""#, spec_path),
    )
    .unwrap();
}

#[tokio::test]
async fn retry_after_429_delta_seconds_waits_then_succeeds() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("spec.json");
    std::fs::write(&spec_path, minimal_spec(mock.address().port())).unwrap();
    install_api(&temp, spec_path.to_str().unwrap());

    // First call: 429 + Retry-After: 1. Second call: 200.
    Mock::given(method("GET"))
        .and(path("/items"))
        .respond_with(
            ResponseTemplate::new(429).insert_header("Retry-After", "1"),
        )
        .up_to_n_times(1)
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/items"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"ok\":true}"))
        .mount(&mock)
        .await;

    let started = Instant::now();
    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args([
            "testapi",
            "get-items",
            "--spall-retry",
            "1",
        ])
        .output()
        .expect("failed to run spall");
    let elapsed = started.elapsed();

    assert!(
        output.status.success(),
        "expected eventual success; stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        elapsed.as_millis() >= 900,
        "expected to wait ~1s for Retry-After, only waited {} ms",
        elapsed.as_millis()
    );
}

#[tokio::test]
async fn retry_after_exceeds_clamp_returns_429_without_waiting() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("spec.json");
    std::fs::write(&spec_path, minimal_spec(mock.address().port())).unwrap();
    install_api(&temp, spec_path.to_str().unwrap());

    // 429 with Retry-After: 600 (10 minutes). Clamp is 2s → fall through.
    Mock::given(method("GET"))
        .and(path("/items"))
        .respond_with(
            ResponseTemplate::new(429).insert_header("Retry-After", "600"),
        )
        .mount(&mock)
        .await;

    let started = Instant::now();
    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args([
            "testapi",
            "get-items",
            "--spall-retry",
            "2",
            "--spall-retry-max-wait",
            "2",
        ])
        .output()
        .expect("failed to run spall");
    let elapsed = started.elapsed();

    assert!(
        !output.status.success(),
        "expected 4xx exit; stderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        elapsed.as_secs() < 5,
        "should not have waited for clamped Retry-After; elapsed={:?}",
        elapsed
    );
}
