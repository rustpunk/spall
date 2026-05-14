//! Parse the three vendored Arazzo fixtures and assert their structural
//! shape. This is intentionally light — full evaluation is covered by
//! `spall-core/src/arazzo/expressions.rs` unit tests and by the
//! `spall-cli` wiremock e2e.

use spall_core::arazzo::{ActionOrRef, ArazzoDocument, FailureActionOrRef};
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
fn with_retry_fixture_parses() {
    let doc = parse("with-retry.arazzo.yaml");
    let wf = &doc.workflows[0];
    assert_eq!(wf.workflow_id, "retryFlow");
    let step = &wf.steps[0];
    assert_eq!(step.step_id, "probeStep");
    let chain = step
        .on_failure
        .as_ref()
        .expect("explicit onFailure should round-trip as Some(vec)");
    assert_eq!(chain.len(), 1);
    match &chain[0] {
        FailureActionOrRef::Inline(a) => {
            assert_eq!(a.name, "try-again");
            assert_eq!(a.kind, "retry");
            assert_eq!(a.retry_after, Some(0.0));
            assert_eq!(a.retry_limit, Some(2));
        }
        FailureActionOrRef::Reference(_) => panic!("expected inline action, got reference"),
    }
}

#[test]
fn with_goto_fixture_parses() {
    let doc = parse("with-goto.arazzo.yaml");
    let wf = &doc.workflows[0];
    assert_eq!(wf.workflow_id, "gotoFlow");
    // 3 steps, first has onFailure redirect, last is the goto target.
    let step_ids: Vec<&str> = wf.steps.iter().map(|s| s.step_id.as_str()).collect();
    assert_eq!(step_ids, vec!["maybeFail", "shouldBeSkipped", "cleanupStep"]);
    let chain = wf.steps[0]
        .on_failure
        .as_ref()
        .expect("first step has onFailure");
    match &chain[0] {
        FailureActionOrRef::Inline(a) => {
            assert_eq!(a.kind, "goto");
            assert_eq!(a.step_id.as_deref(), Some("cleanupStep"));
        }
        FailureActionOrRef::Reference(_) => panic!("expected inline goto"),
    }
    // Steps without onFailure should round-trip as None, NOT Some(vec![]).
    assert!(wf.steps[1].on_failure.is_none());
    assert!(wf.steps[2].on_failure.is_none());
}

#[test]
fn with_component_actions_fixture_parses() {
    let doc = parse("with-component-actions.arazzo.yaml");
    let components = doc.components.as_ref().expect("components block");
    assert_eq!(components.success_actions.len(), 1);
    assert_eq!(components.failure_actions.len(), 1);
    assert_eq!(components.failure_actions["bail"].kind, "end");
    assert_eq!(components.success_actions["keepGoing"].kind, "end");

    let wf = &doc.workflows[0];
    let step = &wf.steps[0];
    let on_failure = step.on_failure.as_ref().expect("step has onFailure");
    match &on_failure[0] {
        FailureActionOrRef::Reference(r) => {
            assert_eq!(r.reference, "$components.failureActions.bail");
        }
        FailureActionOrRef::Inline(_) => panic!("expected reference, got inline"),
    }
    let on_success = step.on_success.as_ref().expect("step has onSuccess");
    match &on_success[0] {
        ActionOrRef::Reference(r) => {
            assert_eq!(r.reference, "$components.successActions.keepGoing");
        }
        ActionOrRef::Inline(_) => panic!("expected reference, got inline"),
    }
}

#[test]
fn explicit_empty_on_success_round_trips_as_some_empty_vec() {
    // Symmetric regression guard with the on_failure case below: the
    // absent-vs-empty distinction matters for both sides of the
    // dispatcher. A serde change that quietly drops it on EITHER
    // field would silently regress the runner's "step opts out of
    // workflow-level fallback" semantics.
    let yaml = r#"
arazzo: "1.0.1"
info:
  title: Empty-onSuccess probe
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: ./openapi.json
    type: openapi
workflows:
  - workflowId: probe
    successActions:
      - name: shout
        type: end
    steps:
      - stepId: optOut
        operationId: getThing
        onSuccess: []
      - stepId: useDefault
        operationId: getThing
"#;
    let doc = spall_core::yaml::from_str::<ArazzoDocument>(yaml)
        .expect("explicit-empty fixture parses");
    let wf = &doc.workflows[0];
    assert_eq!(
        wf.steps[0].on_success.as_ref().map(|v| v.len()),
        Some(0),
        "onSuccess: [] must parse as Some(vec![])",
    );
    assert!(
        wf.steps[1].on_success.is_none(),
        "absent onSuccess must parse as None",
    );
}

#[test]
fn explicit_empty_on_failure_round_trips_as_some_empty_vec() {
    // Regression guard for the Option<Vec<_>> change: `onFailure: []`
    // in YAML must parse to Some(vec![]) so the runner can distinguish
    // "opt out of workflow-level fallback" from "no override".
    let yaml = r#"
arazzo: "1.0.1"
info:
  title: Empty-chain probe
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: ./openapi.json
    type: openapi
workflows:
  - workflowId: probe
    failureActions:
      - name: bail
        type: end
    steps:
      - stepId: optOut
        operationId: getThing
        onFailure: []
      - stepId: useDefault
        operationId: getThing
"#;
    let doc = spall_core::yaml::from_str::<ArazzoDocument>(yaml)
        .expect("explicit-empty fixture parses");
    let wf = &doc.workflows[0];
    let opt_out = &wf.steps[0];
    let use_default = &wf.steps[1];
    assert_eq!(
        opt_out.on_failure.as_ref().map(|v| v.len()),
        Some(0),
        "onFailure: [] must parse as Some(vec![])",
    );
    assert!(
        use_default.on_failure.is_none(),
        "absent onFailure must parse as None",
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
