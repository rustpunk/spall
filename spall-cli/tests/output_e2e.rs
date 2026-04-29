//! End-to-end output format test: table and CSV modes from JSON array.

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
    "/items": {{
      "get": {{
        "operationId": "list-items",
        "responses": {{
          "200": {{
            "description": "OK",
            "content": {{
              "application/json": {{
                "schema": {{
                  "type": "array",
                  "items": {{ "type": "object", "properties": {{ "name": {{ "type": "string" }}, "count": {{ "type": "integer" }} }} }}
                }}
              }}
            }}
          }}
        }}
      }}
    }}
  }}
}}"#,
        port
    )
}

#[tokio::test]
async fn json_array_produces_table() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let port = mock.address().port();

    let spec = minimal_spec(port);
    std::fs::write(temp.path().join("spec.json"), &spec).unwrap();
    setup_config_dir(&temp, temp.path().join("spec.json").to_str().unwrap());

    wiremock::Mock::given(method("GET"))
        .and(path("/items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"name": "alpha", "count": 1},
            {"name": "beta", "count": 2}
        ])))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args(["testapi", "list-items", "--spall-output", "table"])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("alpha"),
        "missing alpha in table: {}",
        stdout
    );
    assert!(stdout.contains("beta"), "missing beta in table: {}", stdout);
    assert!(stdout.contains("┌"), "expected table borders: {}", stdout);
}

#[tokio::test]
async fn json_array_produces_csv() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let port = mock.address().port();

    let spec = minimal_spec(port);
    std::fs::write(temp.path().join("spec.json"), &spec).unwrap();
    setup_config_dir(&temp, temp.path().join("spec.json").to_str().unwrap());

    wiremock::Mock::given(method("GET"))
        .and(path("/items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"name": "alpha", "count": 1},
            {"name": "beta", "count": 2}
        ])))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args(["testapi", "list-items", "--spall-output", "csv"])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("alpha"), "missing alpha in csv: {}", stdout);
    assert!(stdout.contains("beta"), "missing beta in csv: {}", stdout);
    assert!(stdout.contains(','), "expected commas in csv: {}", stdout);
}
