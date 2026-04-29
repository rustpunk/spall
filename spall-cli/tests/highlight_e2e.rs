//! End-to-end test: Pretty JSON output contains ANSI escape codes from syntect.

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
    "/obj": {{
      "get": {{
        "operationId": "get-obj",
        "responses": {{
          "200": {{
            "description": "OK",
            "content": {{
              "application/json": {{
                "schema": {{ "type": "object" }}
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
async fn pretty_json_contains_ansi_escape_codes() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let port = mock.address().port();

    let spec = minimal_spec(port);
    std::fs::write(temp.path().join("spec.json"), &spec).unwrap();
    setup_config_dir(&temp, temp.path().join("spec.json").to_str().unwrap());

    wiremock::Mock::given(method("GET"))
        .and(path("/obj"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"name": "test", "count": 42})),
        )
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        // Explicitly request Pretty so highlighting runs even if stdout is not a TTY.
        .args(["testapi", "get-obj", "--spall-output", "pretty"])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("test"),
        "missing 'test' in output: {}",
        stdout
    );
    // Assert syntect injected ANSI escape codes
    assert!(
        stdout.contains("\x1b["),
        "expected ANSI escape codes in pretty JSON output: {}",
        stdout
    );
}
