//! Arazzo 1.0.1 §4.6 / §4.7 action dispatcher.
//!
//! The v1 runner stopped on the first step failure with
//! `StepFailed`. v1.5 (sub-issue of #5) adds the full failure-action
//! surface: step-level `onSuccess` / `onFailure` chains, workflow-level
//! `successActions` / `failureActions` defaults, and `$components`
//! references. This module is the pure side of that work — it walks
//! a chain of resolved actions, evaluates each one's criteria against
//! the runner's `Context`, and returns a [`StepFlow`] telling the
//! outer loop how to proceed.
//!
//! The expression / condition language is shared with v1's
//! `successCriteria` (`parse_condition` + `eval_condition` in
//! `spall_core::arazzo::expressions`). Out-of-scope criterion types —
//! `jsonpath` and `regex` — are rejected here with an explicit error
//! that points back to issue #5 so partial implementations don't
//! sneak in via test fixtures.

use spall_core::arazzo::{
    eval_condition, parse_condition, Action, ActionOrRef, Components, Context, Criterion,
    ExprError, FailureAction, FailureActionOrRef,
};
use std::time::Duration;
use thiserror::Error;

/// Default `retryLimit` when a `type: retry` FailureAction omits it.
/// Picked small so a misconfigured spec can't loop forever; users who
/// need more should set the field explicitly.
const DEFAULT_RETRY_LIMIT: u32 = 1;

/// Errors raised while resolving or evaluating an action chain.
#[derive(Debug, Error)]
pub enum ActionDispatchError {
    #[error("action reference '{reference}' is not in $components")]
    UnknownReference { reference: String },

    #[error("action reference '{reference}' has unsupported form (expected '$components.successActions.<name>' or '$components.failureActions.<name>')")]
    MalformedReference { reference: String },

    #[error("action '{name}' has unknown type '{kind}' (expected 'end' | 'goto' | 'retry')")]
    UnknownActionKind { name: String, kind: String },

    #[error("action '{name}' type=goto is missing required stepId")]
    GotoMissingStepId { name: String },

    #[error("action '{name}' has unsupported criterion type '{kind}' (jsonpath/regex are v2 — see issue #5)")]
    UnsupportedCriterionType { name: String, kind: String },

    #[error("action '{name}' criterion #{index}: {source}")]
    CriterionExpression {
        name: String,
        index: usize,
        #[source]
        source: ExprError,
    },

    #[error("action '{name}': type=workflowId is not supported in v1.5 (nested workflows are v2)")]
    NestedWorkflowGoto { name: String },
}

/// What the outer step loop should do after evaluating a chain.
#[derive(Debug, Clone)]
pub enum StepFlow {
    /// Proceed to the next step in spec order.
    Continue,
    /// Jump to the step with this ID. The outer loop is responsible
    /// for mapping the ID to an index.
    Goto { step_id: String },
    /// Terminate the workflow with the indicated outcome.
    End { success: bool },
    /// Re-run the current step after sleeping for `after`. Used only by
    /// failure-side dispatch; success-side dispatch never produces this.
    Retry { after: Duration, limit: u32 },
}

/// Resolve every `$components.successActions.<name>` reference in an
/// `Action` chain into an inline `Action`. Missing references and
/// malformed reference paths error out before evaluation so the
/// dispatch loop stays infallible.
pub fn resolve_success_chain(
    chain: &[ActionOrRef],
    components: Option<&Components>,
) -> Result<Vec<Action>, ActionDispatchError> {
    let mut out: Vec<Action> = Vec::with_capacity(chain.len());
    for entry in chain {
        match entry {
            ActionOrRef::Inline(a) => out.push(a.clone()),
            ActionOrRef::Reference(r) => {
                let name = parse_success_ref(&r.reference)?;
                let action = components
                    .and_then(|c| c.success_actions.get(name))
                    .ok_or_else(|| ActionDispatchError::UnknownReference {
                        reference: r.reference.clone(),
                    })?;
                out.push(action.clone());
            }
        }
    }
    Ok(out)
}

/// Resolve every `$components.failureActions.<name>` reference in a
/// `FailureAction` chain into an inline `FailureAction`.
pub fn resolve_failure_chain(
    chain: &[FailureActionOrRef],
    components: Option<&Components>,
) -> Result<Vec<FailureAction>, ActionDispatchError> {
    let mut out: Vec<FailureAction> = Vec::with_capacity(chain.len());
    for entry in chain {
        match entry {
            FailureActionOrRef::Inline(a) => out.push(a.clone()),
            FailureActionOrRef::Reference(r) => {
                let name = parse_failure_ref(&r.reference)?;
                let action = components
                    .and_then(|c| c.failure_actions.get(name))
                    .ok_or_else(|| ActionDispatchError::UnknownReference {
                        reference: r.reference.clone(),
                    })?;
                out.push(action.clone());
            }
        }
    }
    Ok(out)
}

fn parse_success_ref(reference: &str) -> Result<&str, ActionDispatchError> {
    reference
        .strip_prefix("$components.successActions.")
        .ok_or_else(|| ActionDispatchError::MalformedReference {
            reference: reference.to_string(),
        })
}

fn parse_failure_ref(reference: &str) -> Result<&str, ActionDispatchError> {
    reference
        .strip_prefix("$components.failureActions.")
        .ok_or_else(|| ActionDispatchError::MalformedReference {
            reference: reference.to_string(),
        })
}

/// Walk a resolved success chain. The first action whose criteria all
/// evaluate to true wins; its `kind` determines the returned flow.
/// An action with no criteria always applies. When no action matches,
/// `Continue` is returned.
pub fn dispatch_success_chain(
    actions: &[Action],
    ctx: &Context,
) -> Result<StepFlow, ActionDispatchError> {
    for a in actions {
        if !criteria_pass(&a.name, &a.criteria, ctx)? {
            continue;
        }
        return match a.kind.as_str() {
            "end" => Ok(StepFlow::End { success: true }),
            "goto" => goto_flow(a),
            other => Err(ActionDispatchError::UnknownActionKind {
                name: a.name.clone(),
                kind: other.to_string(),
            }),
        };
    }
    Ok(StepFlow::Continue)
}

/// Walk a resolved failure chain. Like [`dispatch_success_chain`], but
/// also handles `type: retry` and reports `End { success: false }` for
/// `type: end`.
pub fn dispatch_failure_chain(
    actions: &[FailureAction],
    ctx: &Context,
) -> Result<StepFlow, ActionDispatchError> {
    for a in actions {
        if !criteria_pass(&a.name, &a.criteria, ctx)? {
            continue;
        }
        return match a.kind.as_str() {
            "end" => Ok(StepFlow::End { success: false }),
            "goto" => failure_goto_flow(a),
            "retry" => {
                let secs = a.retry_after.unwrap_or(0.0).max(0.0);
                let after = Duration::from_secs_f64(secs);
                let limit = a.retry_limit.unwrap_or(DEFAULT_RETRY_LIMIT);
                Ok(StepFlow::Retry { after, limit })
            }
            other => Err(ActionDispatchError::UnknownActionKind {
                name: a.name.clone(),
                kind: other.to_string(),
            }),
        };
    }
    // No action matched — let the failure bubble up to the caller.
    Ok(StepFlow::Continue)
}

fn goto_flow(a: &Action) -> Result<StepFlow, ActionDispatchError> {
    if a.workflow_id.is_some() {
        return Err(ActionDispatchError::NestedWorkflowGoto {
            name: a.name.clone(),
        });
    }
    let step_id = a
        .step_id
        .clone()
        .ok_or_else(|| ActionDispatchError::GotoMissingStepId {
            name: a.name.clone(),
        })?;
    Ok(StepFlow::Goto { step_id })
}

fn failure_goto_flow(a: &FailureAction) -> Result<StepFlow, ActionDispatchError> {
    if a.workflow_id.is_some() {
        return Err(ActionDispatchError::NestedWorkflowGoto {
            name: a.name.clone(),
        });
    }
    let step_id = a
        .step_id
        .clone()
        .ok_or_else(|| ActionDispatchError::GotoMissingStepId {
            name: a.name.clone(),
        })?;
    Ok(StepFlow::Goto { step_id })
}

/// Evaluate every criterion; ALL must pass for the action to fire.
/// Empty criteria list passes by definition (the action is
/// unconditional). v2 criterion types are explicitly rejected here so
/// they don't silently pass-through; the hard-fail discipline keeps
/// users from depending on a partial implementation that lands later.
fn criteria_pass(
    action_name: &str,
    criteria: &[Criterion],
    ctx: &Context,
) -> Result<bool, ActionDispatchError> {
    for (idx, c) in criteria.iter().enumerate() {
        let kind = c.kind.as_deref().unwrap_or("simple");
        if kind != "simple" {
            return Err(ActionDispatchError::UnsupportedCriterionType {
                name: action_name.to_string(),
                kind: kind.to_string(),
            });
        }
        let cond = parse_condition(&c.condition).map_err(|e| {
            ActionDispatchError::CriterionExpression {
                name: action_name.to_string(),
                index: idx,
                source: e,
            }
        })?;
        let ok = eval_condition(&cond, ctx).map_err(|e| {
            ActionDispatchError::CriterionExpression {
                name: action_name.to_string(),
                index: idx,
                source: e,
            }
        })?;
        if !ok {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spall_core::arazzo::ActionRef;
    use std::collections::BTreeMap;

    fn empty_ctx() -> Context {
        Context {
            inputs: BTreeMap::new(),
            steps: BTreeMap::new(),
            current_response: None,
        }
    }

    fn unconditional_end_action() -> Action {
        Action {
            name: "wrap".into(),
            kind: "end".into(),
            workflow_id: None,
            step_id: None,
            criteria: Vec::new(),
        }
    }

    fn unconditional_end_failure() -> FailureAction {
        FailureAction {
            name: "wrap".into(),
            kind: "end".into(),
            workflow_id: None,
            step_id: None,
            retry_after: None,
            retry_limit: None,
            criteria: Vec::new(),
        }
    }

    #[test]
    fn success_chain_with_unconditional_end_terminates() {
        let actions = vec![unconditional_end_action()];
        let flow = dispatch_success_chain(&actions, &empty_ctx()).unwrap();
        matches!(flow, StepFlow::End { success: true })
            .then_some(())
            .unwrap_or_else(|| panic!("expected End, got {:?}", flow));
    }

    #[test]
    fn empty_chain_returns_continue() {
        let flow = dispatch_success_chain(&[], &empty_ctx()).unwrap();
        matches!(flow, StepFlow::Continue)
            .then_some(())
            .unwrap_or_else(|| panic!("expected Continue, got {:?}", flow));
    }

    #[test]
    fn goto_missing_step_id_errors() {
        let action = Action {
            name: "g".into(),
            kind: "goto".into(),
            workflow_id: None,
            step_id: None,
            criteria: Vec::new(),
        };
        let err = dispatch_success_chain(&[action], &empty_ctx()).unwrap_err();
        assert!(matches!(err, ActionDispatchError::GotoMissingStepId { .. }));
    }

    #[test]
    fn workflow_id_in_goto_rejects_as_nested_v2() {
        let action = Action {
            name: "g".into(),
            kind: "goto".into(),
            workflow_id: Some("other".into()),
            step_id: Some("x".into()),
            criteria: Vec::new(),
        };
        let err = dispatch_success_chain(&[action], &empty_ctx()).unwrap_err();
        assert!(matches!(err, ActionDispatchError::NestedWorkflowGoto { .. }));
    }

    #[test]
    fn jsonpath_criterion_kind_is_rejected_hard() {
        let action = Action {
            name: "g".into(),
            kind: "end".into(),
            workflow_id: None,
            step_id: None,
            criteria: vec![Criterion {
                condition: "$.foo".into(),
                context: None,
                kind: Some("jsonpath".into()),
            }],
        };
        let err = dispatch_success_chain(&[action], &empty_ctx()).unwrap_err();
        assert!(
            matches!(err, ActionDispatchError::UnsupportedCriterionType { ref kind, .. } if kind == "jsonpath"),
            "got {:?}",
            err,
        );
    }

    #[test]
    fn regex_criterion_kind_is_rejected_hard() {
        let action = FailureAction {
            name: "g".into(),
            kind: "end".into(),
            workflow_id: None,
            step_id: None,
            retry_after: None,
            retry_limit: None,
            criteria: vec![Criterion {
                condition: "ok.*".into(),
                context: None,
                kind: Some("regex".into()),
            }],
        };
        let err = dispatch_failure_chain(&[action], &empty_ctx()).unwrap_err();
        assert!(matches!(err, ActionDispatchError::UnsupportedCriterionType { .. }));
    }

    #[test]
    fn retry_default_limit_kicks_in_when_field_omitted() {
        let action = FailureAction {
            name: "r".into(),
            kind: "retry".into(),
            workflow_id: None,
            step_id: None,
            retry_after: Some(0.5),
            retry_limit: None,
            criteria: Vec::new(),
        };
        let flow = dispatch_failure_chain(&[action], &empty_ctx()).unwrap();
        match flow {
            StepFlow::Retry { after, limit } => {
                assert_eq!(after, Duration::from_millis(500));
                assert_eq!(limit, DEFAULT_RETRY_LIMIT);
            }
            other => panic!("expected Retry, got {:?}", other),
        }
    }

    #[test]
    fn first_matching_action_wins_in_order() {
        // Two actions: first matches and ends, second is end-with-different-success.
        let actions = vec![
            unconditional_end_failure(),
            FailureAction {
                kind: "retry".into(),
                ..unconditional_end_failure()
            },
        ];
        let flow = dispatch_failure_chain(&actions, &empty_ctx()).unwrap();
        assert!(matches!(flow, StepFlow::End { success: false }));
    }

    #[test]
    fn unknown_action_kind_errors_with_name() {
        let action = Action {
            name: "bogus".into(),
            kind: "abort".into(), // not a known kind
            workflow_id: None,
            step_id: None,
            criteria: Vec::new(),
        };
        let err = dispatch_success_chain(&[action], &empty_ctx()).unwrap_err();
        match err {
            ActionDispatchError::UnknownActionKind { name, kind } => {
                assert_eq!(name, "bogus");
                assert_eq!(kind, "abort");
            }
            other => panic!("expected UnknownActionKind, got {:?}", other),
        }
    }

    #[test]
    fn refs_resolve_against_components() {
        let mut comps = Components::default();
        comps.success_actions.insert(
            "named-end".to_string(),
            unconditional_end_action(),
        );
        let chain = vec![ActionOrRef::Reference(ActionRef {
            reference: "$components.successActions.named-end".to_string(),
        })];
        let resolved = resolve_success_chain(&chain, Some(&comps)).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].kind, "end");
    }

    #[test]
    fn unknown_reference_errors_with_path() {
        let chain = vec![ActionOrRef::Reference(ActionRef {
            reference: "$components.successActions.ghost".to_string(),
        })];
        let err = resolve_success_chain(&chain, Some(&Components::default())).unwrap_err();
        assert!(matches!(err, ActionDispatchError::UnknownReference { .. }));
    }

    #[test]
    fn malformed_reference_path_errors() {
        let chain = vec![ActionOrRef::Reference(ActionRef {
            reference: "components.bogus.path".to_string(), // no $ prefix
        })];
        let err = resolve_success_chain(&chain, None).unwrap_err();
        assert!(matches!(err, ActionDispatchError::MalformedReference { .. }));
    }

    #[test]
    fn failure_chain_resolves_failure_refs_only() {
        // A ref using the successActions path should NOT match the
        // failure resolver — they live in disjoint maps.
        let chain = vec![FailureActionOrRef::Reference(ActionRef {
            reference: "$components.successActions.foo".to_string(),
        })];
        let mut comps = Components::default();
        comps.success_actions.insert(
            "foo".to_string(),
            unconditional_end_action(),
        );
        let err = resolve_failure_chain(&chain, Some(&comps)).unwrap_err();
        assert!(matches!(err, ActionDispatchError::MalformedReference { .. }));
    }

    #[test]
    fn criterion_failure_skips_action() {
        // Action with a criterion that evaluates to false → action does NOT fire.
        let action = Action {
            name: "guarded".into(),
            kind: "end".into(),
            workflow_id: None,
            step_id: None,
            criteria: vec![Criterion {
                condition: "1 == 2".into(),
                context: None,
                kind: None,
            }],
        };
        let flow = dispatch_success_chain(&[action], &empty_ctx()).unwrap();
        assert!(matches!(flow, StepFlow::Continue));
    }

}
