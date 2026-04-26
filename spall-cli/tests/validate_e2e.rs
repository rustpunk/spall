//! End-to-end validation test: invalid parameter → exit code 10.

use std::process::Command;
use tempfile::TempDir;
use wiremock::Match;
use wiremock::{MockServer, ResponseTemplate};
use wiremock::matchers::method;

fn bin_path() -> String {
    std::env::var("CARGO_BIN_EXE_spall")
        .unwrap_or_else(|_| String::from("target/debug/spall"))
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
    "/items/{{id}}": {{
      "get": {{
        "operationId": "get-item",
        "parameters": [
          {{
            "name": "id",
            "in": "path",
            "required": true,
            "schema": {{ "type": "integer" }}
          }}
        ],
        "responses": {{ "200": {{ "description": "OK" }} }}
      }}
    }}
  }}
}}"#,
        port
    )
}

#[tokio::test]
async fn invalid_param_exits_10() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();

    let spec = minimal_spec(mock.address().port());
    std::fs::write(temp.path().join("spec.json"), &spec).unwrap();
    setup_config_dir(&temp, temp.path().join("spec.json").to_str().unwrap());

    wiremock::Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(&spec))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args(["testapi", "get-item", "not-an-integer"])
        .output()
        .expect("failed to run spall");

    assert!(
        !output.status.success(),
        "expected failure, got stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        output.status.code(),
        Some(10),
        "expected exit code 10, got stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
