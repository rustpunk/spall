//! End-to-end tests for `--spall-chain` request chaining.
//!
//! Covers issues #30 (the flag before the API name no longer loops),
//! #34 (a captured value feeds a path parameter), #35 (`--spall-dry-run`
//! short-circuits the chain — zero network), #36 (a dash-prefixed chained
//! value is accepted), and #40 (a cross-API / unknown chain target produces an
//! actionable usage error).

use std::process::Command;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{MockServer, ResponseTemplate};

const EXIT_USAGE: i32 = 1;

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

/// A producer `get-thing` (GET /thing) returning an id, plus two consumers:
/// `get-by-id` (GET /thing/{id}, a path-param target) and `search` (GET
/// /search?offset=, a query-param target used to exercise dash-prefixed
/// values).
fn spec_with_chain_ops(port: u16) -> String {
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
      }}
    }},
    "/thing/{{id}}": {{
      "get": {{
        "operationId": "get-by-id",
        "parameters": [
          {{ "name": "id", "in": "path", "required": true, "schema": {{ "type": "string" }} }}
        ],
        "responses": {{ "200": {{ "description": "OK" }} }}
      }}
    }},
    "/search": {{
      "get": {{
        "operationId": "search",
        "parameters": [
          {{ "name": "offset", "in": "query", "required": false, "schema": {{ "type": "string" }} }}
        ],
        "responses": {{ "200": {{ "description": "OK" }} }}
      }}
    }}
  }}
}}"#,
        port
    )
}

fn write_spec_and_config(temp: &TempDir, port: u16) {
    let spec = spec_with_chain_ops(port);
    let spec_path = temp.path().join("spec.json");
    std::fs::write(&spec_path, &spec).unwrap();
    setup_config_dir(temp, spec_path.to_str().unwrap());
}

// ---- #34: a captured id chains into a path parameter, end-to-end ----

#[tokio::test]
async fn chain_into_path_param_hits_resolved_path() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    write_spec_and_config(&temp, mock.address().port());

    // Producer returns an id.
    wiremock::Mock::given(method("GET"))
        .and(path("/thing"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "42"})))
        .expect(1)
        .mount(&mock)
        .await;

    // Consumer: the chained path must resolve to /thing/42 exactly once.
    wiremock::Mock::given(method("GET"))
        .and(path("/thing/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .expect(1)
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args([
            "testapi",
            "get-thing",
            "--spall-chain",
            "get-by-id --id id",
        ])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "chain into a path param should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // wiremock verifies the .expect(1) hit counts on drop.
}

// ---- #36: a dash-prefixed captured value is accepted by the chained op ----

#[tokio::test]
async fn chain_dash_prefixed_value_is_accepted() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    write_spec_and_config(&temp, mock.address().port());

    // Producer returns a negative offset.
    wiremock::Mock::given(method("GET"))
        .and(path("/thing"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"offset": -5})))
        .expect(1)
        .mount(&mock)
        .await;

    // Consumer: the chained query value `-5` must reach /search?offset=-5.
    wiremock::Mock::given(method("GET"))
        .and(path("/search"))
        .and(wiremock::matchers::query_param("offset", "-5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .expect(1)
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args([
            "testapi",
            "get-thing",
            "--spall-chain",
            "search --offset offset",
        ])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "chain with a dash-prefixed value should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---- #35: --spall-dry-run short-circuits the chain (zero network) ----

#[tokio::test]
async fn dry_run_chain_issues_zero_requests() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    write_spec_and_config(&temp, mock.address().port());

    // No mock is mounted with an .expect(>=1); any request would 404. We assert
    // success + zero received requests below.
    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args([
            "testapi",
            "get-thing",
            "--spall-dry-run",
            "--spall-chain",
            "get-by-id --id id",
        ])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "dry-run chain should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "dry-run chain must produce no stdout, got: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );

    let received = mock.received_requests().await.unwrap_or_default();
    assert!(
        received.is_empty(),
        "dry-run with a chain must issue zero network requests, got {} request(s)",
        received.len()
    );
}

// ---- #30: --spall-chain before the API name does not loop ----

#[tokio::test]
async fn chain_before_api_name_does_not_loop() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    write_spec_and_config(&temp, mock.address().port());

    // If the before-the-API placement looped, /thing/42 would be hit
    // unboundedly. The guard rejects the placement up front, so neither
    // endpoint is touched.
    wiremock::Mock::given(method("GET"))
        .and(path("/thing"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "42"})))
        .mount(&mock)
        .await;
    wiremock::Mock::given(method("GET"))
        .and(path("/thing/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args([
            "--spall-chain",
            "get-by-id --id id",
            "testapi",
            "get-thing",
        ])
        .output()
        .expect("failed to run spall");

    // The placement is rejected with an actionable usage error (exit 1), and
    // crucially the process terminates rather than looping forever.
    assert_eq!(
        output.status.code(),
        Some(EXIT_USAGE),
        "before-the-API --spall-chain should be a usage error, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("after the operation"),
        "error should advise placing --spall-chain after the operation, got: {}",
        stderr
    );

    let received = mock.received_requests().await.unwrap_or_default();
    assert!(
        received.is_empty(),
        "a rejected before-the-API chain must not loop or hit the network, got {} request(s)",
        received.len()
    );
}

// ---- #30 (companion): a legal chain performs exactly one hop and terminates ----

#[tokio::test]
async fn chain_after_op_performs_exactly_one_hop() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    write_spec_and_config(&temp, mock.address().port());

    wiremock::Mock::given(method("GET"))
        .and(path("/thing"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "42"})))
        .expect(1)
        .mount(&mock)
        .await;
    // .expect(1) asserts exactly one hop — a re-trigger loop would exceed it.
    wiremock::Mock::given(method("GET"))
        .and(path("/thing/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .expect(1)
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args([
            "testapi",
            "get-thing",
            "--spall-chain",
            "get-by-id --id id",
        ])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "a legal chain should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let received = mock.received_requests().await.unwrap_or_default();
    assert_eq!(
        received.len(),
        2,
        "exactly one producer hop + one chained hop expected, got {} request(s)",
        received.len()
    );
}

// ---- #40: an unknown chain target yields an actionable usage error ----

#[tokio::test]
async fn chain_unknown_target_op_is_usage_error() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    write_spec_and_config(&temp, mock.address().port());

    wiremock::Mock::given(method("GET"))
        .and(path("/thing"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "42"})))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args([
            "testapi",
            "get-thing",
            "--spall-chain",
            "not-an-operation --id id",
        ])
        .output()
        .expect("failed to run spall");

    assert_eq!(
        output.status.code(),
        Some(EXIT_USAGE),
        "an unknown chain target should be a usage error, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not-an-operation") && stderr.contains("API"),
        "error should name the missing target and the single-API constraint, got: {}",
        stderr
    );
}
