//! Serde model for an Arazzo 1.0.1 document.
//!
//! The doc is parsed via [`crate::yaml::from_str`] — the project's single
//! YAML chokepoint. All structs use `#[serde(rename_all = "camelCase")]` to
//! match the spec.
//!
//! v1 deliberately models only the subset of fields the runner needs.
//! Unknown fields are accepted (lenient parsing) but not preserved; the
//! one extension key the runner consults — `x-spall-api` on a
//! `SourceDescription` — is captured as a named optional field.
//!
//! v1.5 (sub-issue of #5) adds the failure-handling surface from Arazzo
//! §4.6 / §4.7: `Action`, `FailureAction`, `Criterion`, the
//! step-level `onSuccess` / `onFailure` chains, and the workflow-level
//! `successActions` / `failureActions` defaults. The other v2 items
//! tracked by #5 (nested workflows, replay, operationPath,
//! regex/jsonpath criterion types) remain deferred.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Top-level Arazzo document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArazzoDocument {
    /// Version string (e.g. "1.0.1").
    pub arazzo: String,
    pub info: Info,
    #[serde(default)]
    pub source_descriptions: Vec<SourceDescription>,
    #[serde(default)]
    pub workflows: Vec<Workflow>,
    /// Reusable named definitions referenced from steps and workflows.
    /// Per Arazzo §3.4 the spec also includes `inputs` and `parameters`
    /// here; v1 doesn't consume those, so they aren't modeled yet.
    #[serde(default)]
    pub components: Option<Components>,
}

/// Reusable named action definitions referenced from step-level
/// `onSuccess` / `onFailure` chains and workflow-level
/// `successActions` / `failureActions` via `$components.<...>.<name>`
/// references.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Components {
    #[serde(default)]
    pub success_actions: IndexMap<String, Action>,
    #[serde(default)]
    pub failure_actions: IndexMap<String, FailureAction>,
}

/// `info` block: title + version are required by the spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub title: String,
    pub version: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// One entry in `sourceDescriptions[]`. Names an OpenAPI spec by URL or
/// local path; v1 only handles `type: openapi` (omitted defaults to that).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDescription {
    pub name: String,
    pub url: String,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    /// Spall-specific extension: explicitly bind this source to a
    /// configured spall API entry (overrides the default name-match).
    #[serde(default, rename = "x-spall-api")]
    pub x_spall_api: Option<String>,
}

/// A single workflow inside the doc.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub workflow_id: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Inputs schema (JSON Schema). v1 doesn't validate against it; values
    /// from `--input k=v` are inserted as opaque strings into `$inputs`.
    #[serde(default)]
    pub inputs: Option<serde_json::Value>,
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Map of output name → expression string (evaluated after the last
    /// step completes).
    #[serde(default)]
    pub outputs: IndexMap<String, String>,
    /// Workflow-level fallback action chain applied when a step has no
    /// `onSuccess` of its own (Arazzo §4.6).
    #[serde(default)]
    pub success_actions: Vec<ActionOrRef>,
    /// Workflow-level fallback action chain applied when a step has no
    /// `onFailure` of its own (Arazzo §4.6).
    #[serde(default)]
    pub failure_actions: Vec<FailureActionOrRef>,
}

/// A single step inside a workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub step_id: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Either `operationId` or `operationPath` (or `workflowId`, v2) must
    /// be present; v1 supports only `operationId` (bare or
    /// `<sourceName>.<opId>`-qualified).
    #[serde(default)]
    pub operation_id: Option<String>,
    #[serde(default)]
    pub operation_path: Option<String>,
    #[serde(default)]
    pub workflow_id: Option<String>,
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    #[serde(default)]
    pub request_body: Option<RequestBody>,
    #[serde(default)]
    pub success_criteria: Vec<SuccessCriterion>,
    /// Map of output name → expression string. Each is evaluated against
    /// the step's response after `successCriteria` pass.
    #[serde(default)]
    pub outputs: IndexMap<String, String>,
    /// Per-step success-action chain (Arazzo §4.6.1). Overrides
    /// workflow-level `successActions` for this step when non-empty.
    #[serde(default)]
    pub on_success: Vec<ActionOrRef>,
    /// Per-step failure-action chain (Arazzo §4.6.2). Overrides
    /// workflow-level `failureActions` for this step when non-empty.
    #[serde(default)]
    pub on_failure: Vec<FailureActionOrRef>,
}

/// A step parameter. `value` is a JSON value that may be:
/// - a literal (string / number / bool / object / array), passed through; or
/// - a string starting with `$` — a workflow expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Parameter {
    pub name: String,
    /// `path` | `query` | `header` | `cookie`. When omitted, defer to the
    /// operation's own parameter definition.
    #[serde(default, rename = "in")]
    pub location: Option<String>,
    /// Required by the spec. Accepts any JSON value including expression
    /// strings.
    pub value: serde_json::Value,
}

/// A step's request body. `payload` is a JSON value that may itself contain
/// expression strings (whole-value or string-leaf).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestBody {
    #[serde(default)]
    pub content_type: Option<String>,
    pub payload: serde_json::Value,
}

/// One success-criterion entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuccessCriterion {
    /// The runtime-condition string (see [`crate::arazzo::expressions`]).
    pub condition: String,
    /// Base context for `type: jsonpath` / `type: regex`. v1 only
    /// implements the implicit `simple` form, so this is parsed but ignored.
    #[serde(default)]
    pub context: Option<String>,
    /// `simple` (default), `jsonpath`, `regex`. v1 supports `simple` only.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

/// Generic criterion used by `Action.criteria` and `FailureAction.criteria`.
/// Same shape as [`SuccessCriterion`]; the v1.5 runner evaluates only
/// `kind == None | Some("simple")` and rejects `jsonpath` / `regex` at
/// dispatch time with an explicit error linking back to issue #5.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Criterion {
    pub condition: String,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

/// A reference to a named action in
/// [`Components::success_actions`] / [`Components::failure_actions`].
/// Per Arazzo §4.6 the reference string takes the form
/// `$components.successActions.<name>` or
/// `$components.failureActions.<name>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionRef {
    pub reference: String,
}

/// A success-side action chain entry. Either an inline [`Action`] or
/// a `$components.successActions.<name>` reference, distinguished by
/// the presence of a `reference` field. `untagged` order matters:
/// `ActionRef` is tried first because it has a strictly smaller and
/// more identifying shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ActionOrRef {
    Reference(ActionRef),
    Inline(Action),
}

/// A failure-side action chain entry. See [`ActionOrRef`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FailureActionOrRef {
    Reference(ActionRef),
    Inline(FailureAction),
}

/// A success action (Arazzo §4.7.1). When all `criteria` evaluate to
/// true (or `criteria` is empty), the runner applies `kind`:
///
/// - `end`  — terminate the workflow with success.
/// - `goto` — jump to the workflow step named by `step_id` (or, in v2,
///   into the workflow named by `workflow_id`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub workflow_id: Option<String>,
    #[serde(default)]
    pub step_id: Option<String>,
    #[serde(default)]
    pub criteria: Vec<Criterion>,
}

/// A failure action (Arazzo §4.7.2). When all `criteria` evaluate to
/// true (or `criteria` is empty), the runner applies `kind`:
///
/// - `end`   — terminate the workflow (user-handled; exits zero).
/// - `retry` — re-run the current step after `retry_after` seconds,
///   up to `retry_limit` times.
/// - `goto`  — jump to the step named by `step_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureAction {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub workflow_id: Option<String>,
    #[serde(default)]
    pub step_id: Option<String>,
    /// Seconds (fractional) to wait before retrying. Spec allows a
    /// number; we accept any non-negative value.
    #[serde(default)]
    pub retry_after: Option<f64>,
    /// Maximum number of retry attempts beyond the first run.
    #[serde(default)]
    pub retry_limit: Option<u32>,
    #[serde(default)]
    pub criteria: Vec<Criterion>,
}
