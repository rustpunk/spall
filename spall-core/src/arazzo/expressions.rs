//! Arazzo expression dialect + `successCriteria` runtime conditions.
//!
//! Two grammars live here:
//!
//! 1. **Expression** — the `$`-prefixed path-style dialect used in
//!    parameter values, request-body payloads, step outputs, and inside
//!    `successCriteria` operands. Examples: `$inputs.email`,
//!    `$steps.create-user.outputs.token`,
//!    `$steps.s1.response.body#/data/id`,
//!    `$response.header.X-Request-Id`, `$response.statusCode`.
//!
//! 2. **Condition** — the runtime mini-language used by
//!    `SuccessCriterion::condition`. v1 supports:
//!    - a bare expression (truthy test), and
//!    - a three-token binary comparison `<lhs> <op> <rhs>` where each
//!      operand is either an expression or a literal (number, bool,
//!      `null`, or quoted string) and `<op>` is `==` / `!=` / `<` / `<=`
//!      / `>` / `>=`.

use std::collections::BTreeMap;
use thiserror::Error;

/// Failures during expression/condition parsing or evaluation.
#[derive(Debug, Error)]
pub enum ExprError {
    #[error("unknown expression namespace: {0}")]
    UnknownNamespace(String),
    #[error("malformed step expression: {0}")]
    BadStepExpression(String),
    #[error("malformed response expression: {0}")]
    BadResponseExpression(String),
    #[error("malformed condition: {0}")]
    BadCondition(String),
    #[error("unknown workflow input: {0}")]
    UnknownInput(String),
    #[error("unknown step id: {0}")]
    UnknownStep(String),
    #[error("step '{step}' has no output named '{name}'")]
    UnknownOutput { step: String, name: String },
    #[error("header '{0}' not present in response")]
    UnknownHeader(String),
    #[error("JSON Pointer '{0}' did not resolve")]
    PointerNotFound(String),
    #[error("expression used outside a step body: {0}")]
    NoCurrentResponse(String),
    #[error("operand cannot be coerced to a number: {0}")]
    NotANumber(String),
}

/// Parsed Arazzo expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A literal value (used when the source string isn't an expression).
    Literal(serde_json::Value),
    /// `$inputs.<name>` or `$workflow.inputs.<name>`.
    Inputs(String),
    /// `$steps.<step>.outputs.<name>`.
    StepOutput { step: String, name: String },
    /// `$steps.<step>.response.body#/<pointer>`.
    StepResponseBody { step: String, pointer: String },
    /// `$steps.<step>.response.header.<name>`.
    StepResponseHeader { step: String, name: String },
    /// `$steps.<step>.response.statusCode`.
    StepResponseStatus { step: String },
    /// `$response.body#/<pointer>` (only valid inside outputs/criteria).
    ResponseBody { pointer: String },
    /// `$response.header.<name>` (only valid inside outputs/criteria).
    ResponseHeader { name: String },
    /// `$response.statusCode` (only valid inside outputs/criteria).
    ResponseStatus,
}

/// Snapshot of an HTTP response, in the form the evaluator needs.
#[derive(Debug, Clone, Default)]
pub struct ResponseSnapshot {
    pub status: u16,
    /// Lowercased header name → value (latest wins on duplicates).
    pub headers: BTreeMap<String, String>,
    pub body: serde_json::Value,
}

/// One completed step's stored state.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub response: ResponseSnapshot,
    pub outputs: BTreeMap<String, serde_json::Value>,
}

/// Evaluation context. The runner mutates this as it walks steps.
#[derive(Debug, Default)]
pub struct Context {
    pub inputs: BTreeMap<String, serde_json::Value>,
    pub steps: BTreeMap<String, StepResult>,
    /// The response of the step *currently* under evaluation — set
    /// before `successCriteria` and `outputs` evaluation, cleared after.
    pub current_response: Option<ResponseSnapshot>,
}

/// Parse an Arazzo expression string into an [`Expr`].
///
/// If `raw` does not start with `$`, it is returned as
/// `Expr::Literal(Value::String(raw))`. Callers that already know they
/// have a JSON literal (number, object, array) should construct
/// `Expr::Literal` directly without going through this function.
#[must_use = "the parsed Expr is the only output"]
pub fn parse_expression(raw: &str) -> Result<Expr, ExprError> {
    if !raw.starts_with('$') {
        return Ok(Expr::Literal(serde_json::Value::String(raw.to_string())));
    }
    if let Some(rest) = raw.strip_prefix("$inputs.") {
        if rest.is_empty() {
            return Err(ExprError::BadStepExpression(raw.to_string()));
        }
        return Ok(Expr::Inputs(rest.to_string()));
    }
    if let Some(rest) = raw.strip_prefix("$workflow.inputs.") {
        if rest.is_empty() {
            return Err(ExprError::BadStepExpression(raw.to_string()));
        }
        return Ok(Expr::Inputs(rest.to_string()));
    }
    if let Some(rest) = raw.strip_prefix("$steps.") {
        return parse_steps_expr(rest);
    }
    if let Some(rest) = raw.strip_prefix("$response") {
        return parse_response_expr(rest, None);
    }
    Err(ExprError::UnknownNamespace(raw.to_string()))
}

fn parse_steps_expr(rest: &str) -> Result<Expr, ExprError> {
    let (step_id, tail) = rest
        .split_once('.')
        .ok_or_else(|| ExprError::BadStepExpression(rest.to_string()))?;
    if step_id.is_empty() {
        return Err(ExprError::BadStepExpression(rest.to_string()));
    }
    if let Some(name) = tail.strip_prefix("outputs.") {
        if name.is_empty() {
            return Err(ExprError::BadStepExpression(rest.to_string()));
        }
        return Ok(Expr::StepOutput {
            step: step_id.to_string(),
            name: name.to_string(),
        });
    }
    if let Some(resp_tail) = tail.strip_prefix("response") {
        return parse_response_expr(resp_tail, Some(step_id));
    }
    Err(ExprError::BadStepExpression(rest.to_string()))
}

/// Shared parser for the `response`-anchored tail. `step` is `None` when
/// the expression starts with `$response.*` and `Some(id)` for
/// `$steps.<id>.response.*`.
fn parse_response_expr(tail: &str, step: Option<&str>) -> Result<Expr, ExprError> {
    if tail == ".statusCode" {
        return Ok(match step {
            Some(s) => Expr::StepResponseStatus {
                step: s.to_string(),
            },
            None => Expr::ResponseStatus,
        });
    }
    if let Some(after_body) = tail.strip_prefix(".body") {
        if let Some(ptr) = after_body.strip_prefix('#') {
            return Ok(match step {
                Some(s) => Expr::StepResponseBody {
                    step: s.to_string(),
                    pointer: ptr.to_string(),
                },
                None => Expr::ResponseBody {
                    pointer: ptr.to_string(),
                },
            });
        }
    }
    if let Some(name) = tail.strip_prefix(".header.") {
        if name.is_empty() {
            return Err(ExprError::BadResponseExpression(tail.to_string()));
        }
        return Ok(match step {
            Some(s) => Expr::StepResponseHeader {
                step: s.to_string(),
                name: name.to_string(),
            },
            None => Expr::ResponseHeader {
                name: name.to_string(),
            },
        });
    }
    Err(ExprError::BadResponseExpression(tail.to_string()))
}

/// Evaluate a parsed [`Expr`] against a [`Context`].
#[must_use = "ignoring the evaluated value defeats the purpose"]
pub fn eval(expr: &Expr, ctx: &Context) -> Result<serde_json::Value, ExprError> {
    match expr {
        Expr::Literal(v) => Ok(v.clone()),
        Expr::Inputs(name) => ctx
            .inputs
            .get(name)
            .cloned()
            .ok_or_else(|| ExprError::UnknownInput(name.clone())),
        Expr::StepOutput { step, name } => {
            let s = ctx
                .steps
                .get(step)
                .ok_or_else(|| ExprError::UnknownStep(step.clone()))?;
            s.outputs
                .get(name)
                .cloned()
                .ok_or_else(|| ExprError::UnknownOutput {
                    step: step.clone(),
                    name: name.clone(),
                })
        }
        Expr::StepResponseBody { step, pointer } => {
            let s = ctx
                .steps
                .get(step)
                .ok_or_else(|| ExprError::UnknownStep(step.clone()))?;
            json_pointer(&s.response.body, pointer)
        }
        Expr::StepResponseHeader { step, name } => {
            let s = ctx
                .steps
                .get(step)
                .ok_or_else(|| ExprError::UnknownStep(step.clone()))?;
            lookup_header(&s.response.headers, name)
        }
        Expr::StepResponseStatus { step } => {
            let s = ctx
                .steps
                .get(step)
                .ok_or_else(|| ExprError::UnknownStep(step.clone()))?;
            Ok(serde_json::Value::Number(s.response.status.into()))
        }
        Expr::ResponseBody { pointer } => {
            let r = ctx
                .current_response
                .as_ref()
                .ok_or_else(|| ExprError::NoCurrentResponse("$response.body".into()))?;
            json_pointer(&r.body, pointer)
        }
        Expr::ResponseHeader { name } => {
            let r = ctx
                .current_response
                .as_ref()
                .ok_or_else(|| ExprError::NoCurrentResponse("$response.header".into()))?;
            lookup_header(&r.headers, name)
        }
        Expr::ResponseStatus => {
            let r = ctx
                .current_response
                .as_ref()
                .ok_or_else(|| ExprError::NoCurrentResponse("$response.statusCode".into()))?;
            Ok(serde_json::Value::Number(r.status.into()))
        }
    }
}

fn json_pointer(v: &serde_json::Value, ptr: &str) -> Result<serde_json::Value, ExprError> {
    // serde_json::Value::pointer wants the leading "/" and treats "" as the
    // whole document. Both forms are RFC 6901 compliant.
    v.pointer(ptr)
        .cloned()
        .ok_or_else(|| ExprError::PointerNotFound(ptr.to_string()))
}

fn lookup_header(
    headers: &BTreeMap<String, String>,
    name: &str,
) -> Result<serde_json::Value, ExprError> {
    let lower = name.to_ascii_lowercase();
    headers
        .get(&lower)
        .map(|v| serde_json::Value::String(v.clone()))
        .ok_or_else(|| ExprError::UnknownHeader(name.to_string()))
}

// ============================================================================
// Runtime conditions (the `successCriteria` mini-language)
// ============================================================================

/// One operand of a binary comparison: an expression to evaluate, or a
/// literal value parsed from the criterion string itself.
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Expr(Expr),
    Literal(serde_json::Value),
}

/// Comparison operators supported in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Parsed runtime condition.
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// A bare expression — the criterion passes when the value is truthy.
    Truthy(Expr),
    /// A three-token binary comparison.
    BinOp {
        lhs: Operand,
        op: CompareOp,
        rhs: Operand,
    },
}

/// Parse a runtime-condition string into a [`Condition`].
#[must_use = "the parsed Condition is the only output"]
pub fn parse_condition(raw: &str) -> Result<Condition, ExprError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ExprError::BadCondition(raw.to_string()));
    }
    let tokens = tokenize(trimmed)?;
    if tokens.len() == 3 {
        if let Some(op) = parse_compare_op(&tokens[1]) {
            let lhs = parse_operand(&tokens[0])?;
            let rhs = parse_operand(&tokens[2])?;
            return Ok(Condition::BinOp { lhs, op, rhs });
        }
    }
    // Fall back to single-expression truthiness against the *original*
    // input — preserves any internal whitespace inside e.g. JSON Pointers.
    let expr = parse_expression(trimmed)?;
    Ok(Condition::Truthy(expr))
}

fn parse_compare_op(s: &str) -> Option<CompareOp> {
    Some(match s {
        "==" => CompareOp::Eq,
        "!=" => CompareOp::Ne,
        "<" => CompareOp::Lt,
        "<=" => CompareOp::Le,
        ">" => CompareOp::Gt,
        ">=" => CompareOp::Ge,
        _ => return None,
    })
}

fn parse_operand(token: &str) -> Result<Operand, ExprError> {
    if token.is_empty() {
        return Err(ExprError::BadCondition("empty operand".to_string()));
    }
    let first = token.as_bytes()[0];
    if first == b'"' || first == b'\'' {
        let last = token.as_bytes()[token.len() - 1];
        if last != first || token.len() < 2 {
            return Err(ExprError::BadCondition(format!(
                "unterminated quoted operand: {}",
                token
            )));
        }
        let inner = &token[1..token.len() - 1];
        return Ok(Operand::Literal(serde_json::Value::String(
            inner.to_string(),
        )));
    }
    match token {
        "true" => return Ok(Operand::Literal(serde_json::Value::Bool(true))),
        "false" => return Ok(Operand::Literal(serde_json::Value::Bool(false))),
        "null" => return Ok(Operand::Literal(serde_json::Value::Null)),
        _ => {}
    }
    if let Ok(n) = token.parse::<f64>() {
        if let Some(num) = serde_json::Number::from_f64(n) {
            return Ok(Operand::Literal(serde_json::Value::Number(num)));
        }
    }
    Ok(Operand::Expr(parse_expression(token)?))
}

/// Whitespace tokenizer that keeps quoted strings intact (both `"` and `'`).
fn tokenize(s: &str) -> Result<Vec<String>, ExprError> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_quote: Option<char> = None;
    for c in s.chars() {
        if let Some(q) = in_quote {
            cur.push(c);
            if c == q {
                in_quote = None;
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if c == '"' || c == '\'' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            in_quote = Some(c);
            cur.push(c);
            continue;
        }
        if c.is_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        cur.push(c);
    }
    if in_quote.is_some() {
        return Err(ExprError::BadCondition(format!(
            "unterminated quoted string in: {}",
            s
        )));
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

/// Evaluate a parsed condition against a context, returning the pass/fail.
#[must_use = "ignoring the result defeats successCriteria"]
pub fn eval_condition(c: &Condition, ctx: &Context) -> Result<bool, ExprError> {
    match c {
        Condition::Truthy(e) => Ok(is_truthy(&eval(e, ctx)?)),
        Condition::BinOp { lhs, op, rhs } => {
            let l = eval_operand(lhs, ctx)?;
            let r = eval_operand(rhs, ctx)?;
            apply_op(*op, &l, &r)
        }
    }
}

fn eval_operand(op: &Operand, ctx: &Context) -> Result<serde_json::Value, ExprError> {
    match op {
        Operand::Literal(v) => Ok(v.clone()),
        Operand::Expr(e) => eval(e, ctx),
    }
}

fn is_truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(o) => !o.is_empty(),
    }
}

fn apply_op(
    op: CompareOp,
    lhs: &serde_json::Value,
    rhs: &serde_json::Value,
) -> Result<bool, ExprError> {
    match op {
        CompareOp::Eq => Ok(values_equal(lhs, rhs)),
        CompareOp::Ne => Ok(!values_equal(lhs, rhs)),
        CompareOp::Lt | CompareOp::Le | CompareOp::Gt | CompareOp::Ge => {
            let lf = to_f64(lhs)?;
            let rf = to_f64(rhs)?;
            Ok(match op {
                CompareOp::Lt => lf < rf,
                CompareOp::Le => lf <= rf,
                CompareOp::Gt => lf > rf,
                CompareOp::Ge => lf >= rf,
                _ => unreachable!(),
            })
        }
    }
}

fn values_equal(l: &serde_json::Value, r: &serde_json::Value) -> bool {
    match (l, r) {
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
            a.as_f64() == b.as_f64()
        }
        _ => l == r,
    }
}

fn to_f64(v: &serde_json::Value) -> Result<f64, ExprError> {
    match v {
        serde_json::Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| ExprError::NotANumber(v.to_string())),
        serde_json::Value::String(s) => s
            .parse::<f64>()
            .map_err(|_| ExprError::NotANumber(v.to_string())),
        _ => Err(ExprError::NotANumber(v.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_ctx() -> Context {
        let mut ctx = Context::default();
        ctx.inputs
            .insert("email".to_string(), json!("alice@example.com"));
        ctx.inputs.insert("region".to_string(), json!("eu-west"));
        ctx.inputs.insert("flag_true".to_string(), json!(true));
        ctx.inputs.insert("flag_false".to_string(), json!(false));
        ctx.inputs.insert("empty".to_string(), json!(""));
        ctx.inputs.insert("count".to_string(), json!(5));

        let mut s1_headers = BTreeMap::new();
        s1_headers.insert("x-request-id".to_string(), "req-1".to_string());
        s1_headers.insert("content-type".to_string(), "application/json".to_string());
        let s1 = StepResult {
            response: ResponseSnapshot {
                status: 201,
                headers: s1_headers,
                body: json!({"data": {"id": "user-42"}, "status": "ready", "token": "abc"}),
            },
            outputs: {
                let mut m = BTreeMap::new();
                m.insert("token".to_string(), json!("abc"));
                m.insert("count".to_string(), json!(3));
                m
            },
        };
        ctx.steps.insert("s1".to_string(), s1);
        ctx
    }

    #[test]
    fn literal_passthrough() {
        let e = parse_expression("hello").unwrap();
        assert_eq!(e, Expr::Literal(json!("hello")));
        assert_eq!(eval(&e, &make_ctx()).unwrap(), json!("hello"));
    }

    #[test]
    fn inputs_short_form() {
        let e = parse_expression("$inputs.email").unwrap();
        assert_eq!(e, Expr::Inputs("email".to_string()));
        assert_eq!(eval(&e, &make_ctx()).unwrap(), json!("alice@example.com"));
    }

    #[test]
    fn inputs_workflow_alias() {
        let e = parse_expression("$workflow.inputs.region").unwrap();
        assert_eq!(e, Expr::Inputs("region".to_string()));
        assert_eq!(eval(&e, &make_ctx()).unwrap(), json!("eu-west"));
    }

    #[test]
    fn inputs_miss_is_error() {
        let e = parse_expression("$inputs.missing").unwrap();
        let err = eval(&e, &make_ctx()).unwrap_err();
        assert!(matches!(err, ExprError::UnknownInput(ref n) if n == "missing"));
    }

    #[test]
    fn step_output() {
        let e = parse_expression("$steps.s1.outputs.token").unwrap();
        assert_eq!(
            e,
            Expr::StepOutput {
                step: "s1".to_string(),
                name: "token".to_string()
            }
        );
        assert_eq!(eval(&e, &make_ctx()).unwrap(), json!("abc"));
    }

    #[test]
    fn step_response_body_pointer() {
        let e = parse_expression("$steps.s1.response.body#/data/id").unwrap();
        assert_eq!(
            e,
            Expr::StepResponseBody {
                step: "s1".to_string(),
                pointer: "/data/id".to_string(),
            }
        );
        assert_eq!(eval(&e, &make_ctx()).unwrap(), json!("user-42"));
    }

    #[test]
    fn step_response_header_case_insensitive() {
        let e = parse_expression("$steps.s1.response.header.X-Request-Id").unwrap();
        assert_eq!(eval(&e, &make_ctx()).unwrap(), json!("req-1"));
    }

    #[test]
    fn step_response_status() {
        let e = parse_expression("$steps.s1.response.statusCode").unwrap();
        assert_eq!(eval(&e, &make_ctx()).unwrap(), json!(201));
    }

    #[test]
    fn response_expressions_use_current_response() {
        let mut ctx = make_ctx();
        ctx.current_response = Some(ResponseSnapshot {
            status: 200,
            headers: {
                let mut m = BTreeMap::new();
                m.insert("location".to_string(), "/things/9".to_string());
                m
            },
            body: json!({"foo": [10, 20, 30]}),
        });
        let body_e = parse_expression("$response.body#/foo/1").unwrap();
        assert_eq!(eval(&body_e, &ctx).unwrap(), json!(20));
        let hdr_e = parse_expression("$response.header.Location").unwrap();
        assert_eq!(eval(&hdr_e, &ctx).unwrap(), json!("/things/9"));
        let sc = parse_expression("$response.statusCode").unwrap();
        assert_eq!(eval(&sc, &ctx).unwrap(), json!(200));
    }

    #[test]
    fn response_expression_without_current_errors() {
        let ctx = make_ctx();
        let e = parse_expression("$response.statusCode").unwrap();
        assert!(matches!(
            eval(&e, &ctx).unwrap_err(),
            ExprError::NoCurrentResponse(_)
        ));
    }

    #[test]
    fn unknown_namespace_is_error() {
        assert!(matches!(
            parse_expression("$bogus.foo").unwrap_err(),
            ExprError::UnknownNamespace(_)
        ));
    }

    #[test]
    fn empty_input_name_rejected_at_parse_time() {
        // Symmetric with $steps..outputs.x — both should fail at parse,
        // not silently parse and then fail at eval with UnknownInput("").
        assert!(matches!(
            parse_expression("$inputs.").unwrap_err(),
            ExprError::BadStepExpression(_)
        ));
        assert!(matches!(
            parse_expression("$workflow.inputs.").unwrap_err(),
            ExprError::BadStepExpression(_)
        ));
    }

    #[test]
    fn condition_truthy_bare_expression() {
        let ctx = make_ctx();
        let c = parse_condition("$inputs.flag_true").unwrap();
        assert!(eval_condition(&c, &ctx).unwrap());
        let c = parse_condition("$inputs.flag_false").unwrap();
        assert!(!eval_condition(&c, &ctx).unwrap());
        let c = parse_condition("$inputs.empty").unwrap();
        assert!(!eval_condition(&c, &ctx).unwrap());
    }

    #[test]
    fn condition_eq_expr_to_literal_number() {
        let mut ctx = make_ctx();
        ctx.current_response = Some(ResponseSnapshot {
            status: 200,
            headers: BTreeMap::new(),
            body: json!(null),
        });
        let c = parse_condition("$response.statusCode == 200").unwrap();
        assert!(eval_condition(&c, &ctx).unwrap());
        let c = parse_condition("$response.statusCode != 404").unwrap();
        assert!(eval_condition(&c, &ctx).unwrap());
        let c = parse_condition("$response.statusCode == 404").unwrap();
        assert!(!eval_condition(&c, &ctx).unwrap());
    }

    #[test]
    fn condition_eq_string_to_quoted_literal() {
        let mut ctx = make_ctx();
        ctx.current_response = Some(ResponseSnapshot {
            status: 200,
            headers: BTreeMap::new(),
            body: json!({"status": "ready"}),
        });
        let c = parse_condition(r#"$response.body#/status == "ready""#).unwrap();
        assert!(eval_condition(&c, &ctx).unwrap());
        let c = parse_condition(r#"$response.body#/status == "broken""#).unwrap();
        assert!(!eval_condition(&c, &ctx).unwrap());
    }

    #[test]
    fn condition_numeric_comparisons() {
        let ctx = make_ctx();
        let c = parse_condition("$steps.s1.outputs.count >= 3").unwrap();
        assert!(eval_condition(&c, &ctx).unwrap());
        let c = parse_condition("$steps.s1.outputs.count > 5").unwrap();
        assert!(!eval_condition(&c, &ctx).unwrap());
        let c = parse_condition("$steps.s1.outputs.count <= 3").unwrap();
        assert!(eval_condition(&c, &ctx).unwrap());
        let c = parse_condition("$steps.s1.outputs.count < 4").unwrap();
        assert!(eval_condition(&c, &ctx).unwrap());
    }

    #[test]
    fn condition_with_bool_and_null_literals() {
        let ctx = make_ctx();
        let c = parse_condition("$inputs.flag_true == true").unwrap();
        assert!(eval_condition(&c, &ctx).unwrap());
        let c = parse_condition("$inputs.flag_false == false").unwrap();
        assert!(eval_condition(&c, &ctx).unwrap());
        let c = parse_condition("$inputs.flag_true == null").unwrap();
        assert!(!eval_condition(&c, &ctx).unwrap());
    }

    #[test]
    fn condition_numeric_compare_string_operand_errors() {
        let ctx = make_ctx();
        // $inputs.email is "alice@example.com" — not coercible to a number.
        let c = parse_condition("$inputs.email > 5").unwrap();
        assert!(matches!(
            eval_condition(&c, &ctx).unwrap_err(),
            ExprError::NotANumber(_)
        ));
    }

    #[test]
    fn tokenize_keeps_quotes_intact() {
        let toks = tokenize(r#"$x == "a b c""#).unwrap();
        assert_eq!(toks, vec!["$x", "==", "\"a b c\""]);
    }

    #[test]
    fn tokenize_rejects_unterminated_quote() {
        let err = tokenize(r#"$x == "abc"#).unwrap_err();
        assert!(matches!(err, ExprError::BadCondition(_)));
    }
}
