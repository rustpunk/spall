//! End-to-end test: URL spec loading works via `spall_cli::fetch`.

use std::process::Command;
use tempfile::TempDir;
use wiremock::{MockServer, ResponseTemplate};
use wiremock::matchers::{method, path};

fn bin_path() -> String {
    std::env::var("CARGO_BIN_EXE_spall")
        .unwrap_or_else(|_| String::from("target/debug/spall"))
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

fn setup_api_config(temp: &TempDir, url: &str) {
    let config_dir = temp.path().join("spall");
    let apis_dir = config_dir.join("apis");
    std::fs::create_dir_all(&apis_dir).unwrap();
    let api_toml = format!(r#"source = "{}""#, url);
    std::fs::write(apis_dir.join("testapi.toml"), api_toml).unwrap();
}

#[tokio::test]
async fn url_spec_is_fetched_and_api_works() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let port = mock.address().port();

    let spec = minimal_spec(port);
    let spec_url = format!("http://localhost:{}/openapi.json", port);

    wiremock::Mock::given(method("GET"))
        .and(path("/openapi.json"))
        .respond_with(ResponseTemplate::new(200)
            .set_body_string(&spec)
            .insert_header("content-type", "application/json"))
        .mount(&mock)
        .await;

    wiremock::Mock::given(method("GET"))
        .and(path("/items"))
        .respond_with(ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({"items": [1, 2, 3]})))
        .mount(&mock)
        .await;

    setup_api_config(&temp, &spec_url);

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .args(["testapi", "get-items"])
        .output()
        .expect("failed to run spall");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected success. stdout: {}\nstderr: {}", stdout, stderr
    );
    assert!(stdout.contains("items"), "expected 'items' in output, got: {}", stdout);
}
