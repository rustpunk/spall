//! End-to-end tests for byte-preserving output and status-aware exit codes.
//!
//! Covers issues #31 (4xx/5xx bodies are emitted), #32 (dry-run/preview emit
//! no stdout), and #33 (binary / non-JSON bodies round-trip verbatim).

use std::process::Command;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{MockServer, ResponseTemplate};

const EXIT_HTTP_4XX: i32 = 4;
const EXIT_HTTP_5XX: i32 = 5;

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

/// A small spec with a GET /thing and a POST /thing operation.
fn spec_with_ops(port: u16) -> String {
    format!(
        r#"{{
  "openapi": "3.0.0",
  "info": {{ "title": "Test", "version": "1.0.0" }},
  "servers": [{{ "url": "http://localhost:{}" }}],
  "paths": {{
    "/thing": {{
      "get": {{
        "operationId": "get-thing",
        "responses": {{ "200": {{ "description": "OK" }} }}
      }},
      "post": {{
        "operationId": "create-thing",
        "requestBody": {{
          "content": {{ "application/json": {{ "schema": {{ "type": "object" }} }} }}
        }},
        "responses": {{ "200": {{ "description": "OK" }} }}
      }}
    }}
  }}
}}"#,
        port
    )
}

fn write_spec_and_config(temp: &TempDir, port: u16) {
    let spec = spec_with_ops(port);
    let spec_path = temp.path().join("spec.json");
    std::fs::write(&spec_path, &spec).unwrap();
    setup_config_dir(temp, spec_path.to_str().unwrap());
}

// ---- #31: 4xx/5xx response bodies are emitted, with the right exit code ----

#[tokio::test]
async fn http_404_prints_body_and_exits_4() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    write_spec_and_config(&temp, mock.address().port());

    wiremock::Mock::given(method("GET"))
        .and(path("/thing"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(serde_json::json!({"message": "Thing not found"})),
        )
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args(["testapi", "get-thing"])
        .output()
        .expect("failed to run spall");

    assert_eq!(
        output.status.code(),
        Some(EXIT_HTTP_4XX),
        "expected exit 4 for a 404, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Thing not found"),
        "404 error body should be printed to stdout, got: {:?}",
        stdout
    );
}

#[tokio::test]
async fn http_500_prints_body_and_exits_5() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    write_spec_and_config(&temp, mock.address().port());

    wiremock::Mock::given(method("GET"))
        .and(path("/thing"))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_json(serde_json::json!({"error": "boom internal"})),
        )
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args(["testapi", "get-thing"])
        .output()
        .expect("failed to run spall");

    assert_eq!(
        output.status.code(),
        Some(EXIT_HTTP_5XX),
        "expected exit 5 for a 500, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("boom internal"),
        "500 error body should be printed to stdout, got: {:?}",
        stdout
    );
}

// ---- #32: dry-run / preview emit no stdout ----

#[tokio::test]
async fn dry_run_emits_no_stdout() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    write_spec_and_config(&temp, mock.address().port());

    // No mock mounted: a dry run must not hit the network either.
    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args([
            "testapi",
            "create-thing",
            "--spall-dry-run",
            "--data",
            "{\"a\":1}",
        ])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "dry-run should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "dry-run must produce empty stdout, got: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[tokio::test]
async fn preview_emits_no_stdout() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    write_spec_and_config(&temp, mock.address().port());

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args([
            "testapi",
            "create-thing",
            "--spall-preview",
            "--data",
            "{\"a\":1}",
        ])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "preview should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "preview must produce empty stdout, got: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}

// ---- #33: binary / non-JSON bodies round-trip verbatim ----

#[tokio::test]
async fn binary_download_roundtrips_byte_for_byte() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    write_spec_and_config(&temp, mock.address().port());

    // A minimal PNG header followed by bytes that are invalid UTF-8.
    let png_bytes: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0xFF, 0xFE, 0x80, 0x01, 0x02,
    ];

    wiremock::Mock::given(method("GET"))
        .and(path("/thing"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(png_bytes.clone()),
        )
        .mount(&mock)
        .await;

    let out_file = temp.path().join("out.png");
    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args([
            "testapi",
            "get-thing",
            "--spall-download",
            out_file.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "download should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let written = std::fs::read(&out_file).expect("downloaded file should exist");
    assert_eq!(
        written, png_bytes,
        "downloaded binary must be byte-for-byte identical to the response"
    );
}

#[tokio::test]
async fn non_json_text_body_emitted_verbatim_in_raw() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    write_spec_and_config(&temp, mock.address().port());

    let html = "<html><body>not json</body></html>";

    wiremock::Mock::given(method("GET"))
        .and(path("/thing"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .set_body_string(html),
        )
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        // --spall-verbose forces Raw mode; the body must come through verbatim,
        // not quoted-and-escaped as a JSON string.
        .args(["testapi", "get-thing", "--spall-verbose"])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "request should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(html),
        "raw HTML body should be emitted verbatim, got: {:?}",
        stdout
    );
    assert!(
        !stdout.contains("\\u003c") && !stdout.starts_with('"'),
        "body must not be JSON-escaped/quoted, got: {:?}",
        stdout
    );
}
