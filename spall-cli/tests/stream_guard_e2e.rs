//! End-to-end #44 byte-guard test: a giant single (non-array) object served on
//! the `--spall-paginate` path is rejected as not record-streamable with an
//! actionable message, instead of being buffered into an OOM.
//!
//! The default top-level data path applies `concat_results` leniency, so a
//! non-array page is captured whole — that whole-value capture is bounded by
//! `--spall-max-buffer-bytes`. A small cap here makes the guard fire
//! deterministically without needing a literally huge body.

use std::process::Command;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{MockServer, ResponseTemplate};

fn bin_path() -> String {
    std::env::var("CARGO_BIN_EXE_spall").unwrap_or_else(|_| String::from("target/debug/spall"))
}

fn setup_config_dir(temp: &TempDir, spec_path: &str) {
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
    "/blob": {{
      "get": {{
        "operationId": "get-blob",
        "responses": {{
          "200": {{ "description": "OK" }}
        }}
      }}
    }}
  }}
}}"#,
        port
    )
}

#[tokio::test]
async fn oversized_single_object_on_paginate_surfaces_actionable_message() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let port = mock.address().port();

    let spec = minimal_spec(port);
    std::fs::write(temp.path().join("spec.json"), &spec).unwrap();
    setup_config_dir(&temp, temp.path().join("spec.json").to_str().unwrap());

    // One indivisible non-array JSON object whose single string field is much
    // larger than the buffer cap we will pass below.
    let big_value = "z".repeat(64 * 1024);
    let body = serde_json::json!({ "blob": big_value });

    wiremock::Mock::given(method("GET"))
        .and(path("/blob"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args([
            "testapi",
            "get-blob",
            "--spall-paginate",
            // 4 KiB cap << the 64 KiB blob, so the whole-value capture aborts.
            "--spall-max-buffer-bytes",
            "4096",
        ])
        .output()
        .expect("failed to run spall");

    assert!(
        !output.status.success(),
        "expected failure exit code for an oversized non-streamable response"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not record-streamable"),
        "stderr must say the response is not record-streamable; got: {stderr}"
    );
    // Names the cap that fired.
    assert!(
        stderr.contains("4096"),
        "stderr must name the cap that fired; got: {stderr}"
    );
    // Points at the real raw-body-to-file escape hatch.
    assert!(
        stderr.contains("--spall-download"),
        "stderr must point at the --spall-download raw-body-to-file escape; got: {stderr}"
    );
}

#[tokio::test]
async fn oversized_single_array_element_on_paginate_is_rejected() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let port = mock.address().port();

    let spec = minimal_spec(port);
    std::fs::write(temp.path().join("spec.json"), &spec).unwrap();
    setup_config_dir(&temp, temp.path().join("spec.json").to_str().unwrap());

    // A top-level array whose one element is a giant object, exceeding the
    // per-item cap. The item capture aborts mid-flight.
    let big_value = "q".repeat(64 * 1024);
    let body = serde_json::json!([{ "rec": big_value }]);

    wiremock::Mock::given(method("GET"))
        .and(path("/blob"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args([
            "testapi",
            "get-blob",
            "--spall-paginate",
            "--spall-max-item-bytes",
            "4096",
        ])
        .output()
        .expect("failed to run spall");

    assert!(
        !output.status.success(),
        "expected failure for an oversized single record"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not record-streamable") && stderr.contains("4096"),
        "stderr must report the oversized record and name the cap; got: {stderr}"
    );
}

#[tokio::test]
async fn normal_array_under_caps_streams_unaffected() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let port = mock.address().port();

    let spec = minimal_spec(port);
    std::fs::write(temp.path().join("spec.json"), &spec).unwrap();
    setup_config_dir(&temp, temp.path().join("spec.json").to_str().unwrap());

    // A normal small array streams fine even with explicit (generous) caps.
    let body = serde_json::json!([{ "name": "a" }, { "name": "b" }]);
    wiremock::Mock::given(method("GET"))
        .and(path("/blob"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args([
            "testapi",
            "get-blob",
            "--spall-paginate",
            "--spall-max-item-bytes",
            "1048576",
        ])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "expected success for a normal small array, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"a\""), "missing a: {stdout}");
    assert!(stdout.contains("\"b\""), "missing b: {stdout}");
}
