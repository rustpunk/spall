//! End-to-end tests for `--spall-follow <rel>` (hypermedia link following).

use std::process::Command;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
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
    "/start": {{
      "get": {{
        "operationId": "get-start",
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

fn write_spec(temp: &TempDir, mock_port: u16) -> String {
    let path = temp.path().join("spec.json");
    std::fs::write(&path, minimal_spec(mock_port)).unwrap();
    path.to_string_lossy().into_owned()
}

#[tokio::test]
async fn follow_link_via_rfc5988_link_header() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = write_spec(&temp, mock.address().port());
    install_api(&temp, &spec_path);

    let next_url = format!("{}/next", mock.uri());
    let link_header = format!(r#"<{}>; rel="next""#, next_url);

    wiremock::Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("link", link_header.as_str())
                .set_body_string("{}"),
        )
        .mount(&mock)
        .await;

    wiremock::Mock::given(method("GET"))
        .and(path("/next"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "followed": true,
        })))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args(["testapi", "get-start", "--spall-follow", "next"])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"followed\""),
        "expected followed-page body in stdout, got: {}",
        stdout
    );
}

#[tokio::test]
async fn follow_link_via_hal() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = write_spec(&temp, mock.address().port());
    install_api(&temp, &spec_path);

    let next_url = format!("{}/page2", mock.uri());

    wiremock::Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "_links": {
                    "next": {"href": next_url}
                },
                "items": []
            })),
        )
        .mount(&mock)
        .await;

    wiremock::Mock::given(method("GET"))
        .and(path("/page2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "page": 2,
        })))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args(["testapi", "get-start", "--spall-follow", "next"])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"page\""),
        "expected page-2 body in stdout, got: {}",
        stdout
    );
}

#[tokio::test]
async fn follow_link_missing_rel_is_not_an_error() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = write_spec(&temp, mock.address().port());
    install_api(&temp, &spec_path);

    wiremock::Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"items": []})))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args(["testapi", "get-start", "--spall-follow", "next"])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"items\""),
        "primary response should be returned when no matching link exists, got: {}",
        stdout
    );
}
