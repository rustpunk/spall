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
