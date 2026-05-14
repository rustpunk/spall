//! Arazzo 1.0.1 workflow runner — orchestrates step execution against
//! the programmatic request pipeline (`execute::execute_operation_programmatic`).
//!
//! Scope is v1 per issue #3:
//! - Parse the doc with `spall_core::yaml::from_str`.
//! - Load each `sourceDescription` (file or http(s)) via the existing
//!   `fetch::load_raw` + IR cache.
//! - Bind source names to spall API config entries when a name match
//!   exists (the differentiator over Redocly/Respect runners). Otherwise
//!   synthesize a bare `ApiEntry`; requests run unauthenticated.
//! - Walk steps, evaluate parameters / body via the
//!   `spall_core::arazzo::expressions` engine, send via
//!   `execute_operation_programmatic`, capture outputs, check
//!   `successCriteria`.
//!
//! Deferred to v2 (tracked in issue #5): `failureActions`, nested
//! workflows, `replay`, `operationPath`, regex/jsonpath criteria types,
//! `--spall-bind`, inputs-schema validation.

use indexmap::IndexMap;
use spall_config::registry::{ApiEntry, ApiRegistry};
use spall_core::arazzo::{
    eval, eval_condition, parse_condition, parse_expression, ActionOrRef, ArazzoDocument,
    Components, Context, ExprError, FailureActionOrRef, Parameter, ResponseSnapshot,
    SourceDescription, Step, StepResult, Workflow,
};
use spall_core::ir::{ParameterLocation, ResolvedOperation, ResolvedSpec};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use thiserror::Error;

use crate::arazzo_runner_actions::{
    dispatch_failure_chain, dispatch_success_chain, resolve_failure_chain, resolve_success_chain,
    ActionDispatchError, StepFlow,
};
use crate::execute::{
    build_url_with_path_args, execute_operation_programmatic, OperationResult, ProgrammaticArgs,
};
use crate::fetch::load_raw;
use crate::http::HttpConfig;

/// Errors raised during workflow loading or execution.
#[derive(Debug, Error)]
pub enum ArazzoRunError {
    #[error("failed to read Arazzo document {path}: {reason}")]
    ReadDoc { path: String, reason: String },

    #[error("failed to parse Arazzo document {path}: {reason}")]
    ParseDoc { path: String, reason: String },

    #[error("failed to load source '{name}' ({url}): {reason}")]
    LoadSource {
        name: String,
        url: String,
        reason: String,
    },

    #[error("source '{name}' has unsupported type '{kind}' (only 'openapi' is supported in v1)")]
    UnsupportedSourceKind { name: String, kind: String },

    #[error("workflow '{0}' not found in document")]
    WorkflowNotFound(String),

    #[error("document has no workflows")]
    NoWorkflows,

    #[error("step '{step}' uses unsupported feature: {feature}")]
    UnsupportedStepFeature { step: String, feature: String },

    #[error("step '{step}' references unknown operation '{op_id}'")]
    OperationNotFound { step: String, op_id: String },

    #[error("step '{step}' has no operationId set (operationPath and workflowId are v2)")]
    StepMissingOperation { step: String },

    #[error("step '{step}' failed successCriteria #{index}: condition '{condition}'")]
    CriterionFailed {
        step: String,
        index: usize,
        condition: String,
    },

    #[error("step '{step}' returned HTTP {status}{hint}")]
    StepHttpError {
        step: String,
        status: u16,
        hint: String,
    },

    #[error("expression error in {context}: {source}")]
    Expression {
        context: String,
        #[source]
        source: ExprError,
    },

    #[error("step '{step}': {message}")]
    UnknownSource { step: String, message: String },

    #[error("transport error: {0}")]
    Transport(String),

    #[error("step '{step}' action dispatch: {source}")]
    ActionDispatch {
        step: String,
        #[source]
        source: ActionDispatchError,
    },

    #[error("step '{step}' goto target '{target}' does not exist in workflow '{workflow}'")]
    GotoTargetMissing {
        step: String,
        target: String,
        workflow: String,
    },

    #[error("step '{step}' retry limit ({limit}) exhausted; last error: {last}")]
    RetryExhausted {
        step: String,
        limit: u32,
        last: String,
    },

    #[error("workflow '{workflow}' ended on the failure path via failureAction at step '{step}'")]
    WorkflowEndedOnFailure { workflow: String, step: String },

    #[error("workflow '{workflow}' exceeded --spall-max-steps ({limit}) — possible infinite goto loop, last step was '{step}'")]
    StepBudgetExhausted {
        workflow: String,
        step: String,
        limit: usize,
    },
}

/// Diagnostic emitted by `validate_doc` for v2-only or otherwise
/// unsupported constructs. Validation does not refuse to run — it just
/// surfaces what the runner will skip at execution time.
#[derive(Debug, Clone)]
pub struct ValidationDiagnostic {
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

/// Read and parse an Arazzo doc from disk.
#[must_use = "the parsed document is the only output"]
pub fn load_doc(path: &Path) -> Result<ArazzoDocument, ArazzoRunError> {
    let raw = std::fs::read_to_string(path).map_err(|e| ArazzoRunError::ReadDoc {
        path: path.display().to_string(),
        reason: e.to_string(),
    })?;
    spall_core::yaml::from_str::<ArazzoDocument>(&raw).map_err(|e| ArazzoRunError::ParseDoc {
        path: path.display().to_string(),
        reason: e.to_string(),
    })
}

/// Static-time validation. Returns diagnostics for v2-only constructs in
/// the doc; empty Vec means everything in the doc is v1-supported.
#[must_use = "diagnostics drive the validate subcommand's output"]
pub fn validate_doc(doc: &ArazzoDocument) -> Vec<ValidationDiagnostic> {
    let mut out: Vec<ValidationDiagnostic> = Vec::new();
    if doc.arazzo != "1.0.1" && doc.arazzo != "1.0.0" {
        out.push(ValidationDiagnostic {
            severity: Severity::Warning,
            message: format!(
                "document declares arazzo version '{}'; v1 was tested against 1.0.1",
                doc.arazzo
            ),
        });
    }
    if doc.workflows.is_empty() {
        out.push(ValidationDiagnostic {
            severity: Severity::Error,
            message: "document declares no workflows".to_string(),
        });
    }
    for wf in &doc.workflows {
        for step in &wf.steps {
            if step.operation_path.is_some() {
                out.push(ValidationDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "step '{}' uses operationPath (deferred to v2; see issue #5)",
                        step.step_id
                    ),
                });
            }
            if step.workflow_id.is_some() {
                out.push(ValidationDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "step '{}' uses workflowId (nested workflows are v2)",
                        step.step_id
                    ),
                });
            }
            if step.operation_id.is_none()
                && step.operation_path.is_none()
                && step.workflow_id.is_none()
            {
                out.push(ValidationDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "step '{}' has no operationId, operationPath, or workflowId",
                        step.step_id
                    ),
                });
            }
            for c in &step.success_criteria {
                let kind = c.kind.as_deref().unwrap_or("simple");
                if kind != "simple" {
                    out.push(ValidationDiagnostic {
                        severity: Severity::Warning,
                        message: format!(
                            "step '{}' criterion type '{}' is v2 (only 'simple' runs in v1)",
                            step.step_id, kind
                        ),
                    });
                }
            }
        }
    }
    out
}

/// A bundle of fully resolved source specs keyed by `sourceDescription.name`.
pub struct LoadedSources {
    pub specs: HashMap<String, ResolvedSpec>,
    pub entries: HashMap<String, ApiEntry>,
    /// Names of sources whose `ApiEntry` was synthesized (no matching
    /// spall API entry) — used to produce the "unbound" error hint on
    /// step failure.
    pub synthetic: std::collections::HashSet<String>,
}

/// Load every source description referenced by the doc, going through
/// the existing fetch + IR cache pipeline.
///
/// `doc_path` is used to resolve relative source URLs against the doc's
/// directory. `verbose` controls the workflow-start binding banner.
#[must_use = "the LoadedSources bundle drives every subsequent step lookup"]
pub async fn prepare_sources(
    doc: &ArazzoDocument,
    doc_path: &Path,
    registry: &ApiRegistry,
    cache_dir: &Path,
    proxy: Option<&str>,
    verbose: bool,
) -> Result<LoadedSources, ArazzoRunError> {
    let mut specs: HashMap<String, ResolvedSpec> = HashMap::new();
    let mut entries: HashMap<String, ApiEntry> = HashMap::new();
    let mut synthetic: std::collections::HashSet<String> = std::collections::HashSet::new();

    if verbose {
        eprintln!(
            "spall: loading workflow doc '{}' ({} source{}, {} workflow{})",
            doc_path.display(),
            doc.source_descriptions.len(),
            if doc.source_descriptions.len() == 1 { "" } else { "s" },
            doc.workflows.len(),
            if doc.workflows.len() == 1 { "" } else { "s" },
        );
    }

    for source in &doc.source_descriptions {
        if let Some(kind) = source.kind.as_deref() {
            if kind != "openapi" {
                return Err(ArazzoRunError::UnsupportedSourceKind {
                    name: source.name.clone(),
                    kind: kind.to_string(),
                });
            }
        }
        let url = resolve_source_url(&source.url, doc_path);
        let raw = load_raw(&url, cache_dir, proxy).await.map_err(|e| {
            ArazzoRunError::LoadSource {
                name: source.name.clone(),
                url: url.clone(),
                reason: e.to_string(),
            }
        })?;
        let spec = spall_core::cache::load_or_resolve(&url, &raw, cache_dir).map_err(|e| {
            ArazzoRunError::LoadSource {
                name: source.name.clone(),
                url: url.clone(),
                reason: e.to_string(),
            }
        })?;

        let (entry, is_synthetic) = resolve_api_entry(source, &spec, registry);
        if is_synthetic {
            synthetic.insert(source.name.clone());
            // Stderr warning on unbound source — matches the UX from the plan.
            eprintln!(
                "warning: source '{}' is not bound to a spall API \
                 (requests will run unauthenticated; bind with: spall api add {} {})",
                source.name, source.name, source.url,
            );
        } else if verbose {
            let auth_kind = entry
                .auth
                .as_ref()
                .and_then(|a| a.kind.as_ref())
                .map(|k| format!("{:?}", k))
                .unwrap_or_else(|| "unconfigured".to_string());
            eprintln!(
                "  {}  -> spall api '{}'  (auth: {})",
                source.name, entry.name, auth_kind,
            );
        }
        specs.insert(source.name.clone(), spec);
        entries.insert(source.name.clone(), entry);
    }

    Ok(LoadedSources {
        specs,
        entries,
        synthetic,
    })
}

fn resolve_source_url(url: &str, doc_path: &Path) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    let candidate = Path::new(url);
    if candidate.is_absolute() {
        return url.to_string();
    }
    if let Some(parent) = doc_path.parent() {
        return parent.join(url).to_string_lossy().into_owned();
    }
    url.to_string()
}

fn resolve_api_entry(
    source: &SourceDescription,
    spec: &ResolvedSpec,
    registry: &ApiRegistry,
) -> (ApiEntry, bool) {
    let explicit_bind = source.x_spall_api.as_deref();
    let bind_name = explicit_bind.unwrap_or(source.name.as_str());
    if let Some(existing) = registry.resolve_profile(bind_name, None) {
        return (existing, false);
    }
    let synthetic = ApiEntry {
        name: source.name.clone(),
        source: source.url.clone(),
        config_path: None,
        base_url: Some(spec.base_url.clone()),
        default_headers: Vec::new(),
        auth: None,
        proxy: None,
        profiles: std::collections::HashMap::new(),
    };
    (synthetic, true)
}

/// Options for `run_workflow`.
pub struct RunOptions {
    pub workflow_id: Option<String>,
    pub inputs: BTreeMap<String, serde_json::Value>,
    pub dry_run: bool,
    pub verbose: bool,
    /// Hard cap on the number of step executions per workflow. Prevents
    /// `goto X` from step X with always-match criteria from locking up
    /// the runner. Counts retries and goto-revisits, not unique steps.
    pub max_steps: usize,
}

/// Default `max_steps` when `--spall-max-steps` is omitted.
pub const DEFAULT_MAX_STEPS: usize = 10_000;

/// Outcome of a single step.
///
/// `failed_via` is `Some("on-failure-end" | "on-failure-goto")` when
/// the step's HTTP/criteria failure was absorbed by a `failureAction`
/// chain (the workflow continued via `goto` or terminated via `end`).
/// `None` means the step body completed normally — either success or
/// an unhandled failure that bubbled up. Consumers of the JSON output
/// use this to distinguish "step succeeded" from "step's failure was
/// caught."
pub struct StepOutcome {
    pub step_id: String,
    pub status: u16,
    pub outputs: BTreeMap<String, serde_json::Value>,
    pub dry_run: bool,
    pub failed_via: Option<&'static str>,
}

/// Outcome of a workflow run.
pub struct RunOutcome {
    pub workflow_id: String,
    pub steps: Vec<StepOutcome>,
    pub outputs: IndexMap<String, serde_json::Value>,
}

/// Run a workflow end-to-end.
#[must_use = "the RunOutcome carries the workflow's outputs and per-step results"]
pub async fn run_workflow(
    doc: &ArazzoDocument,
    sources: &LoadedSources,
    opts: RunOptions,
    http_config: HttpConfig,
) -> Result<RunOutcome, ArazzoRunError> {
    if doc.workflows.is_empty() {
        return Err(ArazzoRunError::NoWorkflows);
    }
    let wf = pick_workflow(doc, opts.workflow_id.as_deref())?;

    if opts.verbose {
        eprintln!(
            "spall: running workflow '{}' ({} step{})",
            wf.workflow_id,
            wf.steps.len(),
            if wf.steps.len() == 1 { "" } else { "s" },
        );
    }

    let mut ctx = Context {
        inputs: opts.inputs.clone(),
        steps: BTreeMap::new(),
        current_response: None,
    };

    let mut step_outcomes: Vec<StepOutcome> = Vec::new();

    // step_id → index lookup for `goto` targets.
    let step_index: HashMap<String, usize> = wf
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.step_id.clone(), i))
        .collect();

    let mut idx: usize = 0;
    let mut step_budget: usize = 0;
    while idx < wf.steps.len() {
        let step = &wf.steps[idx];
        step_budget += 1;
        if step_budget > opts.max_steps {
            return Err(ArazzoRunError::StepBudgetExhausted {
                workflow: wf.workflow_id.clone(),
                step: step.step_id.clone(),
                limit: opts.max_steps,
            });
        }
        let flow = run_step_with_actions(
            step,
            wf,
            doc.components.as_ref(),
            sources,
            &mut ctx,
            &opts,
            &http_config,
            &mut step_outcomes,
        )
        .await?;
        match flow {
            StepFlow::Continue => idx += 1,
            StepFlow::Goto { step_id } => {
                idx = *step_index.get(&step_id).ok_or_else(|| {
                    ArazzoRunError::GotoTargetMissing {
                        step: step.step_id.clone(),
                        target: step_id.clone(),
                        workflow: wf.workflow_id.clone(),
                    }
                })?;
            }
            StepFlow::End { success } => {
                if opts.verbose {
                    eprintln!(
                        "spall: workflow '{}' ended via {}Action at step '{}'",
                        wf.workflow_id,
                        if success { "success" } else { "failure" },
                        step.step_id,
                    );
                }
                if success {
                    break;
                }
                // Failure-path `type:end` exits non-zero so CI
                // pipelines surface degraded runs. Redocly + Respect
                // runners both behave this way; flipping the sign
                // would silently green-light a workflow that took
                // its failure branch. Users absorbing a known-OK 4xx
                // should use `type:goto` to a cleanup step instead.
                return Err(ArazzoRunError::WorkflowEndedOnFailure {
                    workflow: wf.workflow_id.clone(),
                    step: step.step_id.clone(),
                });
            }
            // Retry returned here means the dispatcher requested a
            // retry but no execution occurred. The retry loop is
            // internal to `run_step_with_actions`; reaching this arm
            // would be a logic bug.
            StepFlow::Retry { .. } => {
                return Err(ArazzoRunError::Transport(format!(
                    "internal: step '{}' surfaced Retry to the outer loop",
                    step.step_id
                )));
            }
        }
    }

    // Workflow-level outputs.
    let mut wf_outputs: IndexMap<String, serde_json::Value> = IndexMap::new();
    for (k, expr_str) in &wf.outputs {
        let expr = parse_expression(expr_str).map_err(|e| ArazzoRunError::Expression {
            context: format!("workflow output '{}'", k),
            source: e,
        })?;
        let v = eval(&expr, &ctx).map_err(|e| ArazzoRunError::Expression {
            context: format!("workflow output '{}'", k),
            source: e,
        })?;
        wf_outputs.insert(k.clone(), v);
    }

    Ok(RunOutcome {
        workflow_id: wf.workflow_id.clone(),
        steps: step_outcomes,
        outputs: wf_outputs,
    })
}

fn pick_workflow<'a>(
    doc: &'a ArazzoDocument,
    requested: Option<&str>,
) -> Result<&'a Workflow, ArazzoRunError> {
    if let Some(id) = requested {
        return doc
            .workflows
            .iter()
            .find(|w| w.workflow_id == id)
            .ok_or_else(|| {
                let available = doc
                    .workflows
                    .iter()
                    .map(|w| w.workflow_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                ArazzoRunError::WorkflowNotFound(format!(
                    "'{}' (available: {})",
                    id, available
                ))
            });
    }
    if doc.workflows.len() == 1 {
        return Ok(&doc.workflows[0]);
    }
    // Multi-workflow doc with no --workflow specified — list the choices.
    let available = doc
        .workflows
        .iter()
        .map(|w| w.workflow_id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Err(ArazzoRunError::WorkflowNotFound(format!(
        "(none specified; multiple workflows in doc — pass --workflow <id>; available: {})",
        available
    )))
}

/// Look up the operation referenced by a step, returning the source name
/// (for `entries`/`specs` lookup), the operation's containing spec, and
/// the operation itself.
fn resolve_step_operation<'a>(
    step: &'a Step,
    sources: &'a LoadedSources,
) -> Result<(&'a str, &'a ResolvedSpec, &'a ResolvedOperation), ArazzoRunError> {
    if step.operation_path.is_some() {
        return Err(ArazzoRunError::UnsupportedStepFeature {
            step: step.step_id.clone(),
            feature: "operationPath (v2, see issue #5)".to_string(),
        });
    }
    if step.workflow_id.is_some() {
        return Err(ArazzoRunError::UnsupportedStepFeature {
            step: step.step_id.clone(),
            feature: "workflowId (nested workflows are v2)".to_string(),
        });
    }
    let op_ref = step
        .operation_id
        .as_deref()
        .ok_or_else(|| ArazzoRunError::StepMissingOperation {
            step: step.step_id.clone(),
        })?;

    // Qualified form: `<sourceName>.operationId`.
    if let Some((src_name, op_id)) = op_ref.split_once('.') {
        if let Some(spec) = sources.specs.get(src_name) {
            if let Some(op) = find_op(spec, op_id) {
                return Ok((find_source_key(sources, src_name), spec, op));
            }
            return Err(ArazzoRunError::OperationNotFound {
                step: step.step_id.clone(),
                op_id: op_ref.to_string(),
            });
        }
        // Fall through: bare opId might contain a `.` legitimately (rare).
    }

    // Unqualified: scan all sources, error on ambiguity.
    let mut hits: Vec<(&str, &ResolvedSpec, &ResolvedOperation)> = Vec::new();
    for (name, spec) in &sources.specs {
        if let Some(op) = find_op(spec, op_ref) {
            hits.push((name.as_str(), spec, op));
        }
    }
    match hits.len() {
        1 => {
            let (name, spec, op) = hits[0];
            Ok((find_source_key(sources, name), spec, op))
        }
        0 => Err(ArazzoRunError::OperationNotFound {
            step: step.step_id.clone(),
            op_id: op_ref.to_string(),
        }),
        _ => Err(ArazzoRunError::UnknownSource {
            step: step.step_id.clone(),
            message: format!(
                "operationId '{}' is ambiguous across multiple sources; use '<sourceName>.{}'",
                op_ref, op_ref
            ),
        }),
    }
}

/// Look up an operation by id, matching either the raw spec-time
/// operationId or the resolver's normalized (lowercase + `_`/`.`/space
/// → `-`) form. The IR stores normalized; Arazzo authors write the raw
/// form they see in the OpenAPI doc — this helper bridges them.
fn find_op<'a>(spec: &'a ResolvedSpec, op_id: &str) -> Option<&'a ResolvedOperation> {
    if let Some(op) = spec.operations.iter().find(|o| o.operation_id == op_id) {
        return Some(op);
    }
    let normalized = normalize_op_id(op_id);
    spec.operations
        .iter()
        .find(|o| o.operation_id == normalized)
}

fn normalize_op_id(raw: &str) -> String {
    raw.replace(['_', ' ', '.'], "-").to_lowercase()
}

fn find_source_key<'a>(sources: &'a LoadedSources, name: &'a str) -> &'a str {
    sources
        .specs
        .keys()
        .find(|k| k.as_str() == name)
        .map(|s| s.as_str())
        .unwrap_or(name)
}

/// Wrap [`run_step`] with the action-chain machinery from Arazzo §4.6.
///
/// On step success: record the outcome and dispatch `step.onSuccess`
/// (or `workflow.successActions` when step-level is empty).
///
/// On a recoverable step failure (HTTP 4xx/5xx or successCriteria
/// fail): dispatch `step.onFailure` (or `workflow.failureActions`).
/// If the chain returns `Retry`, sleep + re-run the same step until
/// the retry limit is reached. If no action in the chain matches
/// (`Continue`), bubble the underlying step error up.
#[allow(clippy::too_many_arguments)]
async fn run_step_with_actions(
    step: &Step,
    workflow: &Workflow,
    components: Option<&Components>,
    sources: &LoadedSources,
    ctx: &mut Context,
    opts: &RunOptions,
    http_config: &HttpConfig,
    step_outcomes: &mut Vec<StepOutcome>,
) -> Result<StepFlow, ArazzoRunError> {
    let success_chain = effective_success_chain(step, workflow);
    let failure_chain = effective_failure_chain(step, workflow);
    let success_actions = resolve_success_chain(success_chain, components)
        .map_err(|e| ArazzoRunError::ActionDispatch {
            step: step.step_id.clone(),
            source: e,
        })?;
    let failure_actions = resolve_failure_chain(failure_chain, components)
        .map_err(|e| ArazzoRunError::ActionDispatch {
            step: step.step_id.clone(),
            source: e,
        })?;

    if opts.dry_run {
        print_action_chain_preview(step, &success_actions, &failure_actions);
    }

    let mut attempt: u32 = 0;
    loop {
        match run_step(step, sources, ctx, opts, http_config).await {
            Ok(outcome) => {
                step_outcomes.push(outcome);
                return dispatch_success_chain(&success_actions, ctx).map_err(|e| {
                    ArazzoRunError::ActionDispatch {
                        step: step.step_id.clone(),
                        source: e,
                    }
                });
            }
            Err(err) if is_recoverable_step_error(&err) => {
                let flow = dispatch_failure_chain(&failure_actions, ctx).map_err(|e| {
                    ArazzoRunError::ActionDispatch {
                        step: step.step_id.clone(),
                        source: e,
                    }
                })?;
                match flow {
                    StepFlow::Continue => return Err(err),
                    StepFlow::Goto { .. } | StepFlow::End { .. } => {
                        // Record a synthetic outcome AND populate
                        // ctx.steps with the response snapshot (or a
                        // zero-status placeholder when the failure was
                        // criteria-only) so downstream expressions
                        // like $steps.<failed-id>.response.statusCode
                        // resolve instead of erroring with "step never
                        // ran". outputs stays empty because step.outputs
                        // never ran.
                        let via = match &flow {
                            StepFlow::Goto { .. } => "on-failure-goto",
                            StepFlow::End { .. } => "on-failure-end",
                            _ => unreachable!(),
                        };
                        step_outcomes.push(synthetic_failure_outcome(step, ctx, via));
                        let snapshot = ctx.current_response.clone().unwrap_or_else(|| {
                            spall_core::arazzo::ResponseSnapshot {
                                status: 0,
                                headers: BTreeMap::new(),
                                body: serde_json::Value::Null,
                            }
                        });
                        ctx.steps.insert(
                            step.step_id.clone(),
                            StepResult {
                                response: snapshot,
                                outputs: BTreeMap::new(),
                            },
                        );
                        return Ok(flow);
                    }
                    StepFlow::Retry { after, limit } => {
                        if attempt >= limit {
                            return Err(ArazzoRunError::RetryExhausted {
                                step: step.step_id.clone(),
                                limit,
                                last: err.to_string(),
                            });
                        }
                        attempt += 1;
                        // Clamp the sleep against the runner's
                        // safety-net so a buggy spec with
                        // `retryAfter: 999999` doesn't hang the
                        // workflow indefinitely.
                        let max_wait = std::time::Duration::from_secs(
                            crate::arazzo_runner_actions::MAX_RETRY_WAIT_SECS,
                        );
                        let after = std::cmp::min(after, max_wait);
                        if opts.verbose {
                            eprintln!(
                                "spall: step '{}' retry {}/{} after {:.3}s ({})",
                                step.step_id,
                                attempt,
                                limit,
                                after.as_secs_f64(),
                                err,
                            );
                        }
                        if !after.is_zero() {
                            tokio::time::sleep(after).await;
                        }
                        // ctx.current_response stays populated from the
                        // failed attempt; run_step overwrites it on
                        // the next try.
                        continue;
                    }
                }
            }
            Err(hard) => return Err(hard),
        }
    }
}

/// Pick the action chain that applies to a step's success path.
///
/// Three-way semantics (Arazzo §4.6): step's `onSuccess: …` wins over
/// workflow-level `successActions` when present; an explicit
/// `onSuccess: []` in the step suppresses workflow-level fallback
/// (the step opts out of the global default); absent `onSuccess`
/// falls back to workflow-level.
fn effective_success_chain<'a>(step: &'a Step, workflow: &'a Workflow) -> &'a [ActionOrRef] {
    match &step.on_success {
        Some(chain) => chain.as_slice(),
        None => workflow.success_actions.as_slice(),
    }
}

/// Pick the action chain that applies to a step's failure path. Same
/// absent-vs-empty distinction as [`effective_success_chain`].
fn effective_failure_chain<'a>(
    step: &'a Step,
    workflow: &'a Workflow,
) -> &'a [FailureActionOrRef] {
    match &step.on_failure {
        Some(chain) => chain.as_slice(),
        None => workflow.failure_actions.as_slice(),
    }
}

/// True for errors that a `failureAction` chain is allowed to swallow.
/// Expression / dispatch / workflow-shape errors always bubble up so a
/// typo can't silently disappear via `type:end`. Transport errors are
/// recoverable — flaky DNS / connection-reset is exactly the case
/// `type:retry` exists for.
fn is_recoverable_step_error(err: &ArazzoRunError) -> bool {
    matches!(
        err,
        ArazzoRunError::StepHttpError { .. }
            | ArazzoRunError::CriterionFailed { .. }
            | ArazzoRunError::Transport(_)
    )
}

/// In `--dry-run` mode, print the resolved success / failure action
/// chains for a step alongside its request preview. Pure-stderr so it
/// stays out of the workflow's JSON output. No-op when both chains
/// are empty (the common case for v1 workflows).
fn print_action_chain_preview(
    step: &Step,
    success: &[spall_core::arazzo::Action],
    failure: &[spall_core::arazzo::FailureAction],
) {
    if success.is_empty() && failure.is_empty() {
        return;
    }
    eprintln!("[dry-run] step '{}' resolved actions:", step.step_id);
    for a in success {
        eprintln!(
            "            onSuccess: {} (type={}{}{})",
            a.name,
            a.kind,
            a.step_id
                .as_deref()
                .map(|s| format!(", stepId={}", s))
                .unwrap_or_default(),
            if a.criteria.is_empty() {
                String::new()
            } else {
                format!(", criteria={}", a.criteria.len())
            },
        );
    }
    for a in failure {
        eprintln!(
            "            onFailure: {} (type={}{}{}{})",
            a.name,
            a.kind,
            a.step_id
                .as_deref()
                .map(|s| format!(", stepId={}", s))
                .unwrap_or_default(),
            a.retry_after
                .map(|s| format!(", retryAfter={}s", s))
                .unwrap_or_default(),
            if a.criteria.is_empty() {
                String::new()
            } else {
                format!(", criteria={}", a.criteria.len())
            },
        );
    }
}

/// Outcome record for a step whose failure was absorbed by a
/// failureAction. `failed_via` distinguishes the two absorption paths
/// so JSON consumers can tell "step succeeded with status N" from
/// "step's HTTP returned N but a failureAction caught it." The status
/// field carries whatever the wire actually returned (or 0 when the
/// failure was criteria-only).
fn synthetic_failure_outcome(step: &Step, ctx: &Context, via: &'static str) -> StepOutcome {
    let status = ctx
        .current_response
        .as_ref()
        .map(|s| s.status)
        .unwrap_or(0);
    StepOutcome {
        step_id: step.step_id.clone(),
        status,
        outputs: BTreeMap::new(),
        dry_run: false,
        failed_via: Some(via),
    }
}

async fn run_step(
    step: &Step,
    sources: &LoadedSources,
    ctx: &mut Context,
    opts: &RunOptions,
    http_config: &HttpConfig,
) -> Result<StepOutcome, ArazzoRunError> {
    let (source_name, spec, op) = resolve_step_operation(step, sources)?;
    let entry = sources
        .entries
        .get(source_name)
        .expect("source name maps to an ApiEntry (sources prepared together)");

    let mut args = ProgrammaticArgs::new();
    args.http = http_config.clone();

    // Step parameters.
    for p in &step.parameters {
        let value = evaluate_value(&p.value, ctx, &format!("step '{}' parameter '{}'", step.step_id, p.name))?;
        let str_val = json_to_string(&value);
        let location = parameter_location(p, op, &step.step_id)?;
        match location.as_str() {
            "path" => {
                args.path.insert(p.name.clone(), str_val);
            }
            "query" => {
                args.query.insert(p.name.clone(), str_val);
            }
            "header" => {
                args.header.insert(p.name.clone(), str_val);
            }
            "cookie" => {
                args.cookie.insert(p.name.clone(), str_val);
            }
            other => {
                return Err(ArazzoRunError::UnsupportedStepFeature {
                    step: step.step_id.clone(),
                    feature: format!("parameter 'in: {}'", other),
                });
            }
        }
    }

    // Request body.
    if let Some(body) = &step.request_body {
        let payload = evaluate_value(
            &body.payload,
            ctx,
            &format!("step '{}' requestBody", step.step_id),
        )?;
        args.body = Some(payload);
        if let Some(ct) = &body.content_type {
            args.header.insert("Content-Type".to_string(), ct.clone());
        }
    }

    if opts.dry_run {
        // Reuse the shared URL builder so the dry-run render exactly
        // matches what would go on the wire — no double-slash bugs from
        // hand-formatted strings.
        let url = build_url_with_path_args(
            op,
            spec,
            entry,
            args.server_override.as_deref(),
            &args.path,
        );
        eprintln!("[dry-run] step '{}': {} {}", step.step_id, op.method, url);
        if !args.query.is_empty() {
            eprintln!("            query: {:?}", args.query);
        }
        if !args.header.is_empty() {
            eprintln!("            header: {:?}", args.header);
        }
        if let Some(b) = &args.body {
            eprintln!("            body: {}", b);
        }
        return Ok(StepOutcome {
            step_id: step.step_id.clone(),
            status: 0,
            outputs: BTreeMap::new(),
            dry_run: true,
            failed_via: None,
        });
    }

    // The programmatic path returns Ok for any HTTP status. Take the
    // result + build the snapshot up front so `ctx.current_response`
    // is populated even when we end up returning `StepHttpError` —
    // on_failure dispatch needs to see `$response.statusCode` etc.
    let op_result = match execute_operation_programmatic(op, spec, entry, &args).await {
        Ok(res) => res,
        Err(e) => return Err(step_http_error(step, source_name, sources, &e)),
    };

    let snapshot = response_snapshot_from(&op_result);
    ctx.current_response = Some(snapshot.clone());

    // 4xx / 5xx lifts into a step-failure error AFTER the snapshot is
    // recorded. This is the spot where the v1 path used to call
    // `raise_for_status`; inlining keeps the snapshot observable on
    // failure for the on_failure dispatch.
    let code = op_result.status.as_u16();
    if op_result.status.is_client_error() || op_result.status.is_server_error() {
        let synthetic = sources.synthetic.contains(source_name);
        let hint = if (code == 401 || code == 403) && synthetic {
            let source = sources
                .entries
                .get(source_name)
                .map(|e| e.source.as_str())
                .unwrap_or("<unknown>");
            format!(
                " — source '{}' is unbound; try: spall api add {} {} && spall auth login {}",
                source_name, source_name, source, source_name
            )
        } else {
            String::new()
        };
        return Err(ArazzoRunError::StepHttpError {
            step: step.step_id.clone(),
            status: code,
            hint,
        });
    }

    // successCriteria.
    for (idx, c) in step.success_criteria.iter().enumerate() {
        if c.kind.as_deref().unwrap_or("simple") != "simple" {
            // v2 criterion type — skipped with a warning.
            if opts.verbose {
                eprintln!(
                    "  step '{}' criterion #{}: type '{}' skipped (v2; see issue #5)",
                    step.step_id,
                    idx,
                    c.kind.as_deref().unwrap_or("?")
                );
            }
            continue;
        }
        let cond =
            parse_condition(&c.condition).map_err(|e| ArazzoRunError::Expression {
                context: format!("step '{}' criterion #{}", step.step_id, idx),
                source: e,
            })?;
        let ok = eval_condition(&cond, ctx).map_err(|e| ArazzoRunError::Expression {
            context: format!("step '{}' criterion #{}", step.step_id, idx),
            source: e,
        })?;
        if !ok {
            return Err(ArazzoRunError::CriterionFailed {
                step: step.step_id.clone(),
                index: idx,
                condition: c.condition.clone(),
            });
        }
    }

    // step.outputs.
    let mut outputs: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for (k, expr_str) in &step.outputs {
        let expr = parse_expression(expr_str).map_err(|e| ArazzoRunError::Expression {
            context: format!("step '{}' output '{}'", step.step_id, k),
            source: e,
        })?;
        let v = eval(&expr, ctx).map_err(|e| ArazzoRunError::Expression {
            context: format!("step '{}' output '{}'", step.step_id, k),
            source: e,
        })?;
        outputs.insert(k.clone(), v);
    }

    let status = snapshot.status;
    ctx.steps.insert(
        step.step_id.clone(),
        StepResult {
            response: snapshot,
            outputs: outputs.clone(),
        },
    );
    ctx.current_response = None;

    Ok(StepOutcome {
        step_id: step.step_id.clone(),
        status,
        outputs,
        dry_run: false,
        failed_via: None,
    })
}

/// Walk a JSON value; string leaves that start with one of the known
/// Arazzo expression namespaces (`$inputs.`, `$workflow.`, `$steps.`,
/// `$response.`) are parsed and replaced with their evaluated value.
/// Strings that merely start with `$` (e.g. `"$10 fee"`, `"$VAR"`) are
/// kept verbatim — only valid expression prefixes trigger parsing.
fn evaluate_value(
    value: &serde_json::Value,
    ctx: &Context,
    context_label: &str,
) -> Result<serde_json::Value, ArazzoRunError> {
    match value {
        serde_json::Value::String(s) => {
            if looks_like_expression(s) {
                let e = parse_expression(s).map_err(|err| ArazzoRunError::Expression {
                    context: context_label.to_string(),
                    source: err,
                })?;
                eval(&e, ctx).map_err(|err| ArazzoRunError::Expression {
                    context: context_label.to_string(),
                    source: err,
                })
            } else {
                Ok(serde_json::Value::String(s.clone()))
            }
        }
        serde_json::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                out.push(evaluate_value(
                    item,
                    ctx,
                    &format!("{}[{}]", context_label, i),
                )?);
            }
            Ok(serde_json::Value::Array(out))
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), evaluate_value(v, ctx, &format!("{}.{}", context_label, k))?);
            }
            Ok(serde_json::Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

/// True when a string starts with one of the known Arazzo expression
/// namespaces. Avoids treating literals like `"$10 fee"` or `"$VAR"` as
/// malformed expressions.
fn looks_like_expression(s: &str) -> bool {
    s.starts_with("$inputs.")
        || s.starts_with("$workflow.")
        || s.starts_with("$steps.")
        || s == "$response.statusCode"
        || s.starts_with("$response.body")
        || s.starts_with("$response.header.")
}

/// Pick the parameter's location: prefer the step-level `in:` override,
/// else look up the operation's parameter by name. Errors hard if the
/// step parameter doesn't match any operation parameter and has no
/// `in:` override — silent fallback would mask Arazzo doc typos.
fn parameter_location(
    p: &Parameter,
    op: &ResolvedOperation,
    step_id: &str,
) -> Result<String, ArazzoRunError> {
    if let Some(loc) = &p.location {
        return Ok(loc.clone());
    }
    if let Some(op_p) = op.parameters.iter().find(|x| x.name == p.name) {
        return Ok(match op_p.location {
            ParameterLocation::Path => "path".to_string(),
            ParameterLocation::Query => "query".to_string(),
            ParameterLocation::Header => "header".to_string(),
            ParameterLocation::Cookie => "cookie".to_string(),
        });
    }
    Err(ArazzoRunError::UnsupportedStepFeature {
        step: step_id.to_string(),
        feature: format!(
            "parameter '{}' has no 'in' field and is not declared on operation '{}' \
             (silent query fallback would mask doc typos; add 'in: query|path|header|cookie')",
            p.name, op.operation_id
        ),
    })
}

fn json_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn response_snapshot_from(res: &OperationResult) -> ResponseSnapshot {
    ResponseSnapshot {
        status: res.status.as_u16(),
        // execute_operation_programmatic returns response headers
        // already lowercased; the expression evaluator looks them up
        // case-insensitively so this is the right shape directly.
        headers: res.headers.clone(),
        body: res.value.clone(),
    }
}

fn step_http_error(
    step: &Step,
    source_name: &str,
    sources: &LoadedSources,
    err: &crate::SpallCliError,
) -> ArazzoRunError {
    let (status, hint) = match err {
        crate::SpallCliError::Http4xx(s) | crate::SpallCliError::Http5xx(s) => {
            let is_synthetic = sources.synthetic.contains(source_name);
            let hint = if (*s == 401 || *s == 403) && is_synthetic {
                let source = sources
                    .entries
                    .get(source_name)
                    .map(|e| e.source.as_str())
                    .unwrap_or("<unknown>");
                format!(
                    " — source '{}' is unbound; try: spall api add {} {} && spall auth login {}",
                    source_name, source_name, source, source_name
                )
            } else {
                String::new()
            };
            (*s, hint)
        }
        other => return ArazzoRunError::Transport(other.to_string()),
    };
    ArazzoRunError::StepHttpError {
        step: step.step_id.clone(),
        status,
        hint,
    }
}

/// Map `RunOutcome` into a JSON value suitable for stdout emission.
#[must_use = "the rendered JSON is the only output"]
pub fn outcome_to_json(outcome: &RunOutcome) -> serde_json::Value {
    let mut outputs = serde_json::Map::new();
    for (k, v) in &outcome.outputs {
        outputs.insert(k.clone(), v.clone());
    }
    let mut steps: Vec<serde_json::Value> = Vec::with_capacity(outcome.steps.len());
    for s in &outcome.steps {
        let mut step_outputs = serde_json::Map::new();
        for (k, v) in &s.outputs {
            step_outputs.insert(k.clone(), v.clone());
        }
        let mut obj = serde_json::Map::new();
        obj.insert("stepId".to_string(), serde_json::json!(s.step_id));
        obj.insert("status".to_string(), serde_json::json!(s.status));
        obj.insert("dryRun".to_string(), serde_json::json!(s.dry_run));
        obj.insert(
            "outputs".to_string(),
            serde_json::Value::Object(step_outputs),
        );
        if let Some(via) = s.failed_via {
            obj.insert("failedVia".to_string(), serde_json::json!(via));
        }
        steps.push(serde_json::Value::Object(obj));
    }
    serde_json::json!({
        "workflowId": outcome.workflow_id,
        "outputs": serde_json::Value::Object(outputs),
        "steps": serde_json::Value::Array(steps),
    })
}

/// Parse `--input k=v` strings into a JSON map. Values that parse as
/// JSON (numbers, bools, null, objects) keep their type; everything else
/// is stored as a string.
#[must_use = "the parsed inputs are the only output"]
pub fn parse_inputs(raw: &[String]) -> Result<BTreeMap<String, serde_json::Value>, ArazzoRunError> {
    let mut out: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for s in raw {
        let (k, v) = s.split_once('=').ok_or_else(|| ArazzoRunError::ParseDoc {
            path: "--input".to_string(),
            reason: format!("expected key=value, got '{}'", s),
        })?;
        let value = serde_json::from_str::<serde_json::Value>(v)
            .unwrap_or_else(|_| serde_json::Value::String(v.to_string()));
        out.insert(k.to_string(), value);
    }
    Ok(out)
}


