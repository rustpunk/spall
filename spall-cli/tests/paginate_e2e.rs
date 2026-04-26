//! End-to-end pagination test: Link header with 3 pages, verify concatenated array.

use std::process::Command;
use tempfile::TempDir;
use wiremock::{MockServer, ResponseTemplate};
use wiremock::matchers::{method, path};

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
                  "items": {{ "type": "object", "properties": {{ "name": {{ "type": "string" }} }} }}
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
async fn paginate_concatenates_three_pages() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let port = mock.address().port();

    let spec = minimal_spec(port);
    std::fs::write(temp.path().join("spec.json"), &spec).unwrap();
    setup_config_dir(&temp, temp.path().join("spec.json").to_str().unwrap());

    wiremock::Mock::given(method("GET"))
        .and(path("/items"))
        .and(wiremock::matchers::query_param_is_missing("page"))
        .respond_with(ResponseTemplate::new(200)
            .insert_header("link", format!("<http://localhost:{}/items?page=2>; rel=\"next\"", port))
            .set_body_json(serde_json::json!([{"name": "a"}])))
        .mount(&mock)
        .await;

    wiremock::Mock::given(method("GET"))
        .and(path("/items"))
        .and(wiremock::matchers::query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200)
            .insert_header("link", format!("<http://localhost:{}/items?page=3>; rel=\"next\"", port))
            .set_body_json(serde_json::json!([{"name": "b"}])))
        .mount(&mock)
        .await;

    wiremock::Mock::given(method("GET"))
        .and(path("/items"))
        .and(wiremock::matchers::query_param("page", "3"))
        .respond_with(ResponseTemplate::new(200)
            .set_body_json(serde_json::json!([{"name": "c"}])))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args(["testapi", "list-items", "--spall-paginate"])
        .output()
        .expect("failed to run spall");

    assert!(output.status.success(),
        "expected success, stderr: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // Should contain all three items in a single JSON array.
    assert!(stdout.contains("\"a\""), "missing a in output: {}", stdout);
    assert!(stdout.contains("\"b\""), "missing b in output: {}", stdout);
    assert!(stdout.contains("\"c\""), "missing c in output: {}", stdout);
    assert!(stdout.starts_with('['), "expected array output: {}", stdout);
}
