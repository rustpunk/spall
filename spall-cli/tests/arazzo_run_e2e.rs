//! End-to-end test for `spall arazzo run` against a wiremock backend.
//!
//! Asserts the three core contracts from issue #3's "Tests" section:
//!
//! 1. Step 1's `outputs.token` flows into step 2's `Authorization`
//!    header parameter via the `$steps.<id>.outputs.<name>` expression.
//! 2. A `successCriteria` of the form
//!    `$response.body#/status == "ready"` is evaluated correctly.
//! 3. The mocked backend receives step 2 with the expected
//!    `Authorization: Bearer <token>` header.
//! 4. The workflow's final output JSON has the values produced by step
//!    2's outputs.

use std::process::Command;
use tempfile::TempDir;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn bin_path() -> String {
    std::env::var("CARGO_BIN_EXE_spall").unwrap_or_else(|_| String::from("target/debug/spall"))
}

fn openapi_for(server_url: &str) -> String {
    format!(
        r#"{{
  "openapi": "3.0.3",
  "info": {{ "title": "Arazzo E2E", "version": "1.0.0" }},
  "servers": [ {{ "url": "{server_url}" }} ],
  "paths": {{
    "/login": {{
      "post": {{
        "operationId": "login",
        "requestBody": {{
          "required": true,
          "content": {{
            "application/json": {{
              "schema": {{
                "type": "object",
                "properties": {{ "email": {{ "type": "string" }} }},
                "required": ["email"]
              }}
            }}
          }}
        }},
        "responses": {{ "200": {{ "description": "OK" }} }}
      }}
    }},
    "/me": {{
      "get": {{
        "operationId": "getMe",
        "parameters": [
          {{ "name": "Authorization", "in": "header", "required": true,
             "schema": {{ "type": "string" }} }}
        ],
        "responses": {{ "200": {{ "description": "OK" }} }}
      }}
    }}
  }}
}}"#
    )
}

fn arazzo_doc(openapi_path: &str) -> String {
    // Using the runtime-condition form '$response.body#/status == "ready"' and
    // step-output passthrough so the workflow exercises both expression
    // shapes from the issue's Tests section.
    format!(
        r#"arazzo: 1.0.1
info:
  title: E2E
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: {openapi_path}
    type: openapi
workflows:
  - workflowId: loginAndFetch
    inputs:
      type: object
      properties:
        email: {{ type: string }}
    steps:
      - stepId: doLogin
        operationId: login
        requestBody:
          contentType: application/json
          payload:
            email: $inputs.email
        successCriteria:
          - condition: $response.statusCode == 200
          - condition: $response.body#/status == "ready"
        outputs:
          token: $response.body#/token
      - stepId: fetchMe
        operationId: getMe
        parameters:
          - name: Authorization
            in: header
            value: $steps.doLogin.outputs.token
        successCriteria:
          - condition: $response.statusCode == 200
        outputs:
          user_id: $response.body#/user_id
    outputs:
      token: $steps.doLogin.outputs.token
      user_id: $steps.fetchMe.outputs.user_id
"#
    )
}

#[tokio::test]
async fn end_to_end_login_then_fetch_passes_token_to_step_two() {
    let server = MockServer::start().await;

    // Step 1: POST /login receives the email from --input and returns a
    // token + status that the workflow checks via successCriteria.
    Mock::given(method("POST"))
        .and(path("/login"))
        .and(body_json(serde_json::json!({"email": "alice@example.com"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "token": "Bearer abc123",
            "status": "ready",
        })))
        .expect(1)
        .mount(&server)
        .await;

    // Step 2: GET /me must arrive with the Authorization header carrying
    // the exact token that step 1's body returned. This is the central
    // contract of the test.
    Mock::given(method("GET"))
        .and(path("/me"))
        .and(header("authorization", "Bearer abc123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "user_id": 42,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let temp = TempDir::new().unwrap();
    let openapi_path = temp.path().join("openapi.json");
    std::fs::write(&openapi_path, openapi_for(&server.uri())).unwrap();
    let arazzo_path = temp.path().join("workflow.arazzo.yaml");
    std::fs::write(
        &arazzo_path,
        arazzo_doc(openapi_path.to_str().expect("utf-8 path")),
    )
    .unwrap();

    // Isolate config + cache to a fresh dir so a developer's local spall
    // config doesn't influence the run.
    let cfg_dir = temp.path().join("cfg");
    let cache_dir = temp.path().join("cache");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", &cfg_dir)
        .env("XDG_CACHE_HOME", &cache_dir)
        .args([
            "arazzo",
            "run",
            arazzo_path.to_str().unwrap(),
            "--input",
            "email=alice@example.com",
        ])
        .output()
        .expect("spawn spall");

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "spall arazzo run failed.\n--- stdout ---\n{}\n--- stderr ---\n{}",
        stdout,
        stderr,
    );

    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("workflow output should be valid JSON");
    assert_eq!(parsed["workflowId"], "loginAndFetch");
    assert_eq!(parsed["outputs"]["token"], "Bearer abc123");
    assert_eq!(parsed["outputs"]["user_id"], 42);

    // Each mock was set with .expect(1); MockServer panics on drop if
    // the expected count wasn't met. Verify explicitly for a clear
    // failure message in case the wiremock version changes that.
    server.verify().await;
}

#[tokio::test]
async fn step_failure_on_unmet_criterion_exits_nonzero() {
    let server = MockServer::start().await;
    // Mock returns `status: "broken"` so the workflow's
    // `$response.body#/status == "ready"` criterion fails.
    Mock::given(method("POST"))
        .and(path("/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "token": "x",
            "status": "broken",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let temp = TempDir::new().unwrap();
    let openapi_path = temp.path().join("openapi.json");
    std::fs::write(&openapi_path, openapi_for(&server.uri())).unwrap();
    let arazzo_path = temp.path().join("workflow.arazzo.yaml");
    std::fs::write(
        &arazzo_path,
        arazzo_doc(openapi_path.to_str().expect("utf-8 path")),
    )
    .unwrap();

    let cfg_dir = temp.path().join("cfg");
    let cache_dir = temp.path().join("cache");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", &cfg_dir)
        .env("XDG_CACHE_HOME", &cache_dir)
        .args([
            "arazzo",
            "run",
            arazzo_path.to_str().unwrap(),
            "--input",
            "email=alice@example.com",
        ])
        .output()
        .expect("spawn spall");

    assert!(
        !output.status.success(),
        "expected non-zero exit on criterion failure"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.contains("criterion") || stderr.contains("doLogin"),
        "stderr should mention the failed criterion / step: {}",
        stderr
    );
}

#[tokio::test]
async fn validate_subcommand_reports_clean_v1_doc() {
    let temp = TempDir::new().unwrap();
    let arazzo_path = temp.path().join("workflow.arazzo.yaml");
    std::fs::write(
        &arazzo_path,
        arazzo_doc("./openapi.json"), // path doesn't need to exist for validate
    )
    .unwrap();

    let cfg_dir = temp.path().join("cfg");
    let cache_dir = temp.path().join("cache");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", &cfg_dir)
        .env("XDG_CACHE_HOME", &cache_dir)
        .args(["arazzo", "validate", arazzo_path.to_str().unwrap()])
        .output()
        .expect("spawn spall");

    assert!(
        output.status.success(),
        "validate of a clean v1 doc must succeed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(stderr.contains("parses cleanly"), "stderr: {}", stderr);
}
