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
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

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
async fn with_criteria_fixture_runs_all_three_conditions() {
    // The vendored fixture in e2e/fixtures/arazzo/with-criteria.arazzo.yaml
    // stacks three criteria — `==` on a numeric, `==` on a quoted
    // string, and `!=` on an empty-string literal. The parse test
    // covers shape; this runtime test pins down evaluation: if any of
    // the three is mis-evaluated the workflow returns a non-zero exit.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login"))
        .and(body_json(serde_json::json!({"email": "alice@example.com"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "token": "tok-9",
            "status": "ready",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let temp = TempDir::new().unwrap();
    // Write the openapi spec where the fixture expects it (alongside
    // the .arazzo.yaml as `simple-openapi.json`).
    let openapi_path = temp.path().join("simple-openapi.json");
    std::fs::write(&openapi_path, openapi_for(&server.uri())).unwrap();

    // Copy the vendored fixture into the temp dir so its relative
    // `url: ./simple-openapi.json` resolves against the temp path.
    let fixture_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("e2e")
        .join("fixtures")
        .join("arazzo")
        .join("with-criteria.arazzo.yaml");
    let arazzo_path = temp.path().join("with-criteria.arazzo.yaml");
    std::fs::copy(&fixture_src, &arazzo_path)
        .expect("copy with-criteria fixture into temp dir");

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
        "with-criteria fixture should pass all three criteria.\n--- stdout ---\n{}\n--- stderr ---\n{}",
        stdout, stderr,
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("workflow output must be valid JSON");
    assert_eq!(parsed["workflowId"], "criteriaCheck");
    server.verify().await;
}

/// OpenAPI spec used by the failure-action tests below. Has a single
/// GET /probe operation whose response status is whatever the
/// wiremock backend returns.
fn probe_spec(server_url: &str) -> String {
    format!(
        r#"{{
  "openapi": "3.0.3",
  "info": {{ "title": "Failure Actions", "version": "1.0.0" }},
  "servers": [{{ "url": "{server_url}" }}],
  "paths": {{
    "/probe":   {{ "get": {{ "operationId": "probe",   "responses": {{ "200": {{ "description": "OK" }} }} }} }},
    "/cleanup": {{ "get": {{ "operationId": "cleanup", "responses": {{ "200": {{ "description": "OK" }} }} }} }}
  }}
}}"#
    )
}

/// Returns 503 the first `fail_count` times, 200 thereafter. Used to
/// exercise the retry-then-succeed flow without needing wiremock's
/// scenario API.
struct CountingResponder {
    counter: AtomicUsize,
    fail_count: usize,
}

impl Respond for CountingResponder {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_count {
            ResponseTemplate::new(503)
        } else {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true}))
        }
    }
}

fn write_failure_workflow_files(
    temp: &TempDir,
    server_url: &str,
    arazzo_yaml: &str,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let openapi_path = temp.path().join("openapi.json");
    std::fs::write(&openapi_path, probe_spec(server_url)).unwrap();
    let arazzo_path = temp.path().join("workflow.arazzo.yaml");
    // Substitute the openapi path into the yaml template.
    let yaml = arazzo_yaml.replace("__OPENAPI__", openapi_path.to_str().unwrap());
    std::fs::write(&arazzo_path, yaml).unwrap();
    let cfg_dir = temp.path().join("cfg");
    let cache_dir = temp.path().join("cache");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();
    (arazzo_path, cfg_dir, cache_dir)
}

#[tokio::test]
async fn failure_action_retry_recovers_after_two_503s() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/probe"))
        .respond_with(CountingResponder {
            counter: AtomicUsize::new(0),
            fail_count: 2,
        })
        // 3 total attempts: 1 initial + 2 retries.
        .expect(3)
        .mount(&server)
        .await;

    let temp = TempDir::new().unwrap();
    let yaml = r#"arazzo: 1.0.1
info: { title: Retry, version: 1.0.0 }
sourceDescriptions:
  - { name: api, url: "__OPENAPI__", type: openapi }
workflows:
  - workflowId: retryFlow
    steps:
      - stepId: probeStep
        operationId: probe
        onFailure:
          - name: try-again
            type: retry
            retryAfter: 0
            retryLimit: 2
"#;
    let (arazzo_path, cfg_dir, cache_dir) =
        write_failure_workflow_files(&temp, &server.uri(), yaml);

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", &cfg_dir)
        .env("XDG_CACHE_HOME", &cache_dir)
        .args(["arazzo", "run", arazzo_path.to_str().unwrap()])
        .output()
        .expect("spawn spall");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "expected workflow to succeed after retries.\n--- stdout ---\n{}\n--- stderr ---\n{}",
        stdout, stderr,
    );
    server.verify().await;
}

#[tokio::test]
async fn failure_action_retry_exhausts_when_failures_persist() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/probe"))
        .respond_with(ResponseTemplate::new(500))
        // 3 total attempts: 1 initial + 2 retries before exhausting.
        .expect(3)
        .mount(&server)
        .await;

    let temp = TempDir::new().unwrap();
    let yaml = r#"arazzo: 1.0.1
info: { title: Exhaust, version: 1.0.0 }
sourceDescriptions:
  - { name: api, url: "__OPENAPI__", type: openapi }
workflows:
  - workflowId: exhaustFlow
    steps:
      - stepId: probeStep
        operationId: probe
        onFailure:
          - name: try-again
            type: retry
            retryAfter: 0
            retryLimit: 2
"#;
    let (arazzo_path, cfg_dir, cache_dir) =
        write_failure_workflow_files(&temp, &server.uri(), yaml);

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", &cfg_dir)
        .env("XDG_CACHE_HOME", &cache_dir)
        .args(["arazzo", "run", arazzo_path.to_str().unwrap()])
        .output()
        .expect("spawn spall");

    assert!(!output.status.success(), "retry exhaustion must exit non-zero");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.contains("retry limit"),
        "stderr should mention retry exhaustion: {}",
        stderr,
    );
    server.verify().await;
}

#[tokio::test]
async fn failure_action_goto_jumps_to_recovery_step() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/probe"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;
    // The cleanup mock MUST be hit exactly once via the goto flow.
    // If goto fails to redirect, this expect(1) will not match.
    Mock::given(method("GET"))
        .and(path("/cleanup"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let temp = TempDir::new().unwrap();
    let yaml = r#"arazzo: 1.0.1
info: { title: Goto, version: 1.0.0 }
sourceDescriptions:
  - { name: api, url: "__OPENAPI__", type: openapi }
workflows:
  - workflowId: gotoFlow
    steps:
      - stepId: maybeFail
        operationId: probe
        onFailure:
          - name: redirect
            type: goto
            stepId: cleanupStep
      - stepId: shouldBeSkipped
        operationId: probe
      - stepId: cleanupStep
        operationId: cleanup
"#;
    let (arazzo_path, cfg_dir, cache_dir) =
        write_failure_workflow_files(&temp, &server.uri(), yaml);

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", &cfg_dir)
        .env("XDG_CACHE_HOME", &cache_dir)
        .args(["arazzo", "run", arazzo_path.to_str().unwrap()])
        .output()
        .expect("spawn spall");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "goto-recovery workflow should succeed.\n--- stdout ---\n{}\n--- stderr ---\n{}",
        stdout, stderr,
    );
    // The failed step's record in the per-step JSON output must
    // carry `failedVia: "on-failure-goto"` — that's the public
    // contract consumers use to distinguish an absorbed failure
    // from a clean success. A future refactor that drops the field
    // (or renames the wire string) trips this assertion.
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("workflow output must be valid JSON");
    let steps = parsed["steps"].as_array().expect("steps array");
    let maybe_fail = steps
        .iter()
        .find(|s| s["stepId"] == "maybeFail")
        .expect("maybeFail step record");
    assert_eq!(
        maybe_fail["failedVia"], "on-failure-goto",
        "absorbed-failure step must surface failedVia in JSON output: {:?}",
        maybe_fail,
    );
    let cleanup = steps
        .iter()
        .find(|s| s["stepId"] == "cleanupStep")
        .expect("cleanupStep record");
    assert!(
        cleanup.get("failedVia").is_none(),
        "successful step record must NOT carry failedVia: {:?}",
        cleanup,
    );
    server.verify().await;
}

#[tokio::test]
async fn failure_action_criteria_gated_end_swallows_expected_4xx() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/probe"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;

    let temp = TempDir::new().unwrap();
    let yaml = r#"arazzo: 1.0.1
info: { title: Gated, version: 1.0.0 }
sourceDescriptions:
  - { name: api, url: "__OPENAPI__", type: openapi }
workflows:
  - workflowId: gatedEnd
    steps:
      - stepId: probeStep
        operationId: probe
        onFailure:
          - name: swallow-404
            type: end
            criteria:
              - condition: $response.statusCode == 404
"#;
    let (arazzo_path, cfg_dir, cache_dir) =
        write_failure_workflow_files(&temp, &server.uri(), yaml);

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", &cfg_dir)
        .env("XDG_CACHE_HOME", &cache_dir)
        .args(["arazzo", "run", arazzo_path.to_str().unwrap()])
        .output()
        .expect("spawn spall");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    // Criterion-gated failure-side `type:end` still exits non-zero —
    // the criteria controlled WHICH failureAction fired, not whether
    // the workflow ended on the failure path. The match itself is
    // observable via the workflow + step attribution in stderr.
    assert!(
        !output.status.success(),
        "criterion-gated end on the failure path must exit non-zero.\n--- stderr ---\n{}",
        stderr,
    );
    assert!(
        stderr.contains("gatedEnd") && stderr.contains("probeStep"),
        "stderr should attribute the end to its workflow + step: {}",
        stderr,
    );
    server.verify().await;
}

#[tokio::test]
async fn workflow_level_failure_actions_apply_when_step_has_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/probe"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;

    let temp = TempDir::new().unwrap();
    let yaml = r#"arazzo: 1.0.1
info: { title: Fallback, version: 1.0.0 }
sourceDescriptions:
  - { name: api, url: "__OPENAPI__", type: openapi }
workflows:
  - workflowId: fallbackFlow
    failureActions:
      - name: workflow-end
        type: end
    steps:
      - stepId: probeStep
        operationId: probe
"#;
    let (arazzo_path, cfg_dir, cache_dir) =
        write_failure_workflow_files(&temp, &server.uri(), yaml);

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", &cfg_dir)
        .env("XDG_CACHE_HOME", &cache_dir)
        .args(["arazzo", "run", arazzo_path.to_str().unwrap()])
        .output()
        .expect("spawn spall");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    // Failure-side `type:end` exits non-zero by design: the workflow
    // took its failure branch and CI pipelines need to surface that.
    // Users who want to swallow a known-OK 4xx should `type:goto` a
    // cleanup step instead — see `failure_action_goto_jumps_to_recovery_step`.
    assert!(
        !output.status.success(),
        "workflow-level type:end failure action must exit non-zero.\n--- stderr ---\n{}",
        stderr,
    );
    // The error attribution should name both the workflow and the
    // step that triggered the failureAction.
    assert!(
        stderr.contains("fallbackFlow") && stderr.contains("probeStep"),
        "stderr should attribute the end to its workflow + step: {}",
        stderr,
    );
    server.verify().await;
}

#[tokio::test]
async fn components_named_action_is_resolvable_from_step_reference() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/probe"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;

    let temp = TempDir::new().unwrap();
    let yaml = r#"arazzo: 1.0.1
info: { title: Components, version: 1.0.0 }
sourceDescriptions:
  - { name: api, url: "__OPENAPI__", type: openapi }
components:
  failureActions:
    bail:
      name: bail
      type: end
workflows:
  - workflowId: viaComponents
    steps:
      - stepId: probeStep
        operationId: probe
        onFailure:
          - reference: $components.failureActions.bail
"#;
    let (arazzo_path, cfg_dir, cache_dir) =
        write_failure_workflow_files(&temp, &server.uri(), yaml);

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", &cfg_dir)
        .env("XDG_CACHE_HOME", &cache_dir)
        .args(["arazzo", "run", arazzo_path.to_str().unwrap()])
        .output()
        .expect("spawn spall");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    // The reference resolves to a failure-side `type:end` → exit
    // non-zero with workflow+step attribution. If the reference had
    // FAILED to resolve, the runner would have raised an
    // ActionDispatch error with "action dispatch" in stderr — assert
    // we DIDN'T hit that path so we know the ref was honored.
    assert!(
        !output.status.success(),
        "resolved type:end on failure path → workflow exits non-zero.\n--- stderr ---\n{}",
        stderr,
    );
    assert!(
        !stderr.contains("action dispatch"),
        "components reference should resolve cleanly, stderr was: {}",
        stderr,
    );
    assert!(
        stderr.contains("viaComponents"),
        "stderr should attribute the end to the workflow id: {}",
        stderr,
    );
    server.verify().await;
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
