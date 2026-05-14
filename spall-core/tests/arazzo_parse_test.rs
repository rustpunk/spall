//! Parse the three vendored Arazzo fixtures and assert their structural
//! shape. This is intentionally light — full evaluation is covered by
//! `spall-core/src/arazzo/expressions.rs` unit tests and by the
//! `spall-cli` wiremock e2e.

use spall_core::arazzo::ArazzoDocument;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("e2e")
        .join("fixtures")
        .join("arazzo")
        .join(name)
}

fn parse(name: &str) -> ArazzoDocument {
    let path = fixture(name);
    let raw = std::fs::read_to_string(&path).expect("fixture file");
    spall_core::yaml::from_str::<ArazzoDocument>(&raw)
        .unwrap_or_else(|e| panic!("parse {}: {}", path.display(), e))
}

#[test]
fn simple_fixture_parses() {
    let doc = parse("simple.arazzo.yaml");
    assert_eq!(doc.arazzo, "1.0.1");
    assert_eq!(doc.info.title, "Simple two-step workflow");
    assert_eq!(doc.source_descriptions.len(), 1);
    assert_eq!(doc.source_descriptions[0].name, "api");
    assert!(doc.source_descriptions[0]
        .url
        .ends_with("simple-openapi.json"));
    assert_eq!(doc.workflows.len(), 1);
    let wf = &doc.workflows[0];
    assert_eq!(wf.workflow_id, "simpleLoginAndFetch");
    assert_eq!(wf.steps.len(), 2);
    assert_eq!(wf.steps[0].step_id, "doLogin");
    assert_eq!(wf.steps[0].operation_id.as_deref(), Some("login"));
    assert_eq!(wf.steps[1].step_id, "fetchMe");
    assert_eq!(wf.steps[1].operation_id.as_deref(), Some("getMe"));
}

#[test]
fn with_criteria_fixture_parses() {
    let doc = parse("with-criteria.arazzo.yaml");
    let wf = &doc.workflows[0];
    assert_eq!(wf.workflow_id, "criteriaCheck");
    let step = &wf.steps[0];
    assert_eq!(step.success_criteria.len(), 3);
    let conditions: Vec<&str> = step
        .success_criteria
        .iter()
        .map(|c| c.condition.as_str())
        .collect();
    assert!(conditions.iter().any(|c| c.contains("statusCode == 200")));
    assert!(conditions.iter().any(|c| c.contains("status == \"ready\"")));
    assert!(conditions.iter().any(|c| c.contains("token != \"\"")));
}

#[test]
fn x_spall_api_extension_parses_on_source_description() {
    // Regression guard: the runner reads `x-spall-api` to override the
    // default name-match binding from doc-source name → spall API
    // entry. A future serde rename or `#[serde(flatten)]` change could
    // silently drop this field; this test catches that.
    let yaml = r#"
arazzo: "1.0.1"
info:
  title: Bind override probe
  version: "1.0.0"
sourceDescriptions:
  - name: petstore-prod
    url: ./openapi.json
    type: openapi
    x-spall-api: petstore
workflows:
  - workflowId: probe
    steps:
      - stepId: only
        operationId: getPet
"#;
    let doc = spall_core::yaml::from_str::<ArazzoDocument>(yaml)
        .expect("x-spall-api fixture parses");
    let src = &doc.source_descriptions[0];
    assert_eq!(src.name, "petstore-prod");
    assert_eq!(
        src.x_spall_api.as_deref(),
        Some("petstore"),
        "x-spall-api extension must round-trip into SourceDescription.x_spall_api",
    );
}

#[test]
fn with_outputs_fixture_parses() {
    let doc = parse("with-outputs.arazzo.yaml");
    let wf = &doc.workflows[0];
    assert_eq!(wf.workflow_id, "loginAndUseToken");
    assert_eq!(wf.steps.len(), 2);
    let step0 = &wf.steps[0];
    assert_eq!(step0.outputs.get("token").map(|s| s.as_str()),
        Some("$response.body#/token"));
    let step1 = &wf.steps[1];
    let auth_param = step1
        .parameters
        .iter()
        .find(|p| p.name == "Authorization")
        .expect("Authorization parameter");
    assert_eq!(auth_param.location.as_deref(), Some("header"));
    assert_eq!(
        auth_param.value.as_str(),
        Some("$steps.doLogin.outputs.token")
    );
    // Workflow-level outputs.
    assert_eq!(
        wf.outputs.get("user_id").map(|s| s.as_str()),
        Some("$steps.fetchMe.outputs.user_id")
    );
}
