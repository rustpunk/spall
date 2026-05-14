//! Model Context Protocol server over stdio.
//!
//! `spall mcp <api>` exposes every (filtered) operation in a registered
//! API as an MCP tool. The wire protocol is line-delimited JSON-RPC 2.0;
//! three RPC methods are handled (`initialize`, `tools/list`,
//! `tools/call`) plus the `notifications/initialized` and `ping`
//! protocol-level messages. Anything else returns method-not-found.
//!
//! Stdout discipline is critical: every byte we write to stdout must
//! parse as a JSON-RPC message. Diagnostics go to stderr via
//! `eprintln!`. No `tracing` dep — see plan doc for the rationale.

pub mod schema;

use indexmap::IndexMap;
use serde_json::{json, Value};
use spall_config::registry::ApiEntry;
use spall_core::ir::{ParameterLocation, ResolvedOperation, ResolvedSpec};
use std::collections::BTreeMap;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::execute::{execute_operation_programmatic, ProgrammaticArgs};

/// Advertised by the server during `initialize`. The MCP spec allows
/// server and client to negotiate; if the client sends an older string
/// we still respond with this one and the client can decide whether to
/// proceed.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// JSON-RPC error code per spec §3 (method-not-found).
const ERR_METHOD_NOT_FOUND: i32 = -32601;
const ERR_INVALID_PARAMS: i32 = -32602;
const ERR_PARSE: i32 = -32700;

/// One tool in the dispatch registry. Built once at startup and never
/// mutated thereafter; the operation index is what dispatch uses to
/// look up the `ResolvedOperation` on `tools/call`.
struct ToolEntry {
    op_index: usize,
    description: String,
    input_schema: Value,
}

/// Build the tool registry from `spec` applying the include/exclude tag
/// filter. Returns tools in spec order (`IndexMap` preserves insertion).
fn build_registry(
    spec: &ResolvedSpec,
    include: &[String],
    exclude: &[String],
) -> IndexMap<String, ToolEntry> {
    let mut out: IndexMap<String, ToolEntry> = IndexMap::new();
    for (idx, op) in spec.operations.iter().enumerate() {
        if !tag_filter_admits(op, include, exclude) {
            continue;
        }
        let raw = sanitize_tool_name(&op.operation_id);
        let name = unique_name(&raw, &out);
        let description = build_description(op);
        let input_schema = schema::operation_input_schema(op);
        out.insert(
            name,
            ToolEntry {
                op_index: idx,
                description,
                input_schema,
            },
        );
    }
    out
}

fn tag_filter_admits(op: &ResolvedOperation, include: &[String], exclude: &[String]) -> bool {
    let synthetic_default = op.tags.is_empty();
    let tag_in = |list: &[String]| -> bool {
        if synthetic_default {
            list.iter().any(|t| t == "default")
        } else {
            op.tags.iter().any(|t| list.iter().any(|w| w == t))
        }
    };
    if !include.is_empty() && !tag_in(include) {
        return false;
    }
    if !exclude.is_empty() && tag_in(exclude) {
        return false;
    }
    true
}

/// Lower-kebab-case sanitizer that matches the SEP-986 tool-name
/// character class (`[A-Za-z0-9_./-]`, 1..64). Whitespace and any
/// disallowed character collapses to `-`. Runs of `-` are collapsed.
fn sanitize_tool_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for ch in raw.chars() {
        let allowed = ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/');
        if allowed {
            // Lower-case ASCII to match spall's existing CLI naming.
            for c in ch.to_lowercase() {
                out.push(c);
            }
            prev_dash = ch == '-';
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.starts_with('-') {
        out.remove(0);
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("op");
    }
    // Defensive truncation. SEP-986 caps at 64; we cap at 64 to match.
    if out.len() > 64 {
        out.truncate(64);
        while out.ends_with('-') {
            out.pop();
        }
    }
    out
}

fn unique_name(raw: &str, existing: &IndexMap<String, ToolEntry>) -> String {
    if !existing.contains_key(raw) {
        return raw.to_string();
    }
    let mut n: usize = 2;
    loop {
        let candidate = format!("{}-{}", raw, n);
        if !existing.contains_key(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn build_description(op: &ResolvedOperation) -> String {
    let head = op
        .summary
        .clone()
        .or_else(|| op.description.clone())
        .unwrap_or_else(|| format!("{} {}", op.method, op.path_template));
    let mut buf = head;
    if !op.tags.is_empty() {
        buf.push_str(" (tags: ");
        buf.push_str(&op.tags.join(", "));
        buf.push(')');
    }
    buf
}

/// Server entry point. Builds the tool registry then serves stdio
/// JSON-RPC until EOF on stdin.
#[must_use = "ignoring this Result swallows server-side errors"]
pub async fn run(
    api_name: String,
    spec: ResolvedSpec,
    entry: ApiEntry,
    include: Vec<String>,
    exclude: Vec<String>,
) -> Result<(), crate::SpallCliError> {
    let registry = build_registry(&spec, &include, &exclude);
    eprintln!(
        "spall mcp: serving '{}' over stdio ({} tool{})",
        api_name,
        registry.len(),
        if registry.len() == 1 { "" } else { "s" }
    );

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::stdout();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_line(&line, &spec, &entry, &registry).await;
        if let Some(resp) = response {
            let mut bytes = match serde_json::to_vec(&resp) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("spall mcp: failed to serialize response: {}", e);
                    continue;
                }
            };
            bytes.push(b'\n');
            if let Err(e) = stdout.write_all(&bytes).await {
                eprintln!("spall mcp: stdout write failed: {}", e);
                break;
            }
            if let Err(e) = stdout.flush().await {
                eprintln!("spall mcp: stdout flush failed: {}", e);
                break;
            }
        }
    }
    Ok(())
}

/// Parse one JSON-RPC frame and dispatch. Returns `None` for
/// notifications (no `id`) and for parse errors that aren't recoverable
/// into a JSON-RPC error envelope (notifications can't error per spec).
async fn handle_line(
    line: &str,
    spec: &ResolvedSpec,
    entry: &ApiEntry,
    registry: &IndexMap<String, ToolEntry>,
) -> Option<Value> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("spall mcp: invalid JSON-RPC line: {}", e);
            // Parse errors don't have an id; spec §3 says reply with null id.
            return Some(rpc_error(Value::Null, ERR_PARSE, "Parse error"));
        }
    };

    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let id = msg.get("id").cloned();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    match method {
        "initialize" => Some(rpc_result(id, handle_initialize())),
        "notifications/initialized" | "notifications/cancelled" => None,
        "ping" => Some(rpc_result(id, json!({}))),
        "tools/list" => Some(rpc_result(id, handle_tools_list(registry))),
        "tools/call" => {
            let result = handle_tools_call(&params, spec, entry, registry).await;
            Some(rpc_result(id, result))
        }
        "" => {
            // No method field at all — malformed but recoverable.
            Some(rpc_error(
                id.unwrap_or(Value::Null),
                ERR_INVALID_PARAMS,
                "missing method",
            ))
        }
        other => {
            // Unknown method. If the original was a notification (no
            // `id`), the spec forbids a reply.
            id.map(|id| {
                rpc_error(
                    id,
                    ERR_METHOD_NOT_FOUND,
                    &format!("unknown method: {}", other),
                )
            })
        }
    }
}

fn handle_initialize() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {
            "tools": { "listChanged": false }
        },
        "serverInfo": {
            "name": "spall",
            "version": env!("CARGO_PKG_VERSION"),
        }
    })
}

fn handle_tools_list(registry: &IndexMap<String, ToolEntry>) -> Value {
    let tools: Vec<Value> = registry
        .iter()
        .map(|(name, entry)| {
            json!({
                "name": name,
                "description": entry.description,
                "inputSchema": entry.input_schema,
            })
        })
        .collect();
    json!({ "tools": tools })
}

async fn handle_tools_call(
    params: &Value,
    spec: &ResolvedSpec,
    entry: &ApiEntry,
    registry: &IndexMap<String, ToolEntry>,
) -> Value {
    let name = match params.get("name").and_then(Value::as_str) {
        Some(s) => s,
        None => return tool_error("missing 'name' in tools/call params"),
    };
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);

    let tool = match registry.get(name) {
        Some(t) => t,
        None => return tool_error(&format!("unknown tool: {}", name)),
    };
    let op = match spec.operations.get(tool.op_index) {
        Some(op) => op,
        None => return tool_error("internal: operation index out of range"),
    };

    let prog_args = match build_programmatic_args(op, &arguments) {
        Ok(a) => a,
        Err(e) => return tool_error(&e),
    };

    match execute_operation_programmatic(op, spec, entry, &prog_args).await {
        Ok(res) => {
            let is_error = res.status.as_u16() >= 400;
            let body_text = match serde_json::to_string_pretty(&res.value) {
                Ok(s) => s,
                Err(_) => res.value.to_string(),
            };
            json!({
                "content": [{ "type": "text", "text": body_text }],
                "isError": is_error,
            })
        }
        Err(e) => tool_error(&format!("request failed: {}", e)),
    }
}

fn tool_error(message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true,
    })
}

/// Walk the MCP-provided arguments object, route each key to the
/// matching `ResolvedParameter::location` slot in `ProgrammaticArgs`, and
/// handle the `body` reserved key when the operation has a request body.
/// Non-string scalars are coerced to strings for non-body slots (path,
/// query, header, cookie all want `String`).
fn build_programmatic_args(
    op: &ResolvedOperation,
    arguments: &Value,
) -> Result<ProgrammaticArgs, String> {
    let mut prog = ProgrammaticArgs::default();
    let obj = match arguments {
        Value::Object(map) => map,
        Value::Null => return Ok(prog),
        _ => return Err("'arguments' must be a JSON object".to_string()),
    };

    let mut by_name: BTreeMap<&str, ParameterLocation> = BTreeMap::new();
    for p in &op.parameters {
        by_name.insert(p.name.as_str(), p.location);
    }

    for (key, value) in obj {
        if key == "body" && op.request_body.is_some() {
            prog.body = Some(value.clone());
            continue;
        }
        let loc = match by_name.get(key.as_str()) {
            Some(l) => *l,
            None => return Err(format!("unknown argument '{}'", key)),
        };
        let stringified = coerce_to_string(value);
        match loc {
            ParameterLocation::Path => {
                prog.path.insert(key.clone(), stringified);
            }
            ParameterLocation::Query => {
                prog.query.insert(key.clone(), stringified);
            }
            ParameterLocation::Header => {
                prog.header.insert(key.clone(), stringified);
            }
            ParameterLocation::Cookie => {
                prog.cookie.insert(key.clone(), stringified);
            }
        }
    }
    Ok(prog)
}

fn coerce_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => String::new(),
        Value::Array(_) | Value::Object(_) => v.to_string(),
    }
}

fn rpc_result(id: Option<Value>, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "result": result,
    })
}

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use spall_core::ir::{
        HttpMethod, ResolvedOperation, ResolvedParameter, ResolvedSchema, ResolvedSpec,
    };

    fn op(id: &str, tags: &[&str]) -> ResolvedOperation {
        ResolvedOperation {
            operation_id: id.to_string(),
            method: HttpMethod::Get,
            path_template: format!("/{}", id),
            summary: None,
            description: None,
            deprecated: false,
            parameters: Vec::new(),
            request_body: None,
            responses: IndexMap::new(),
            security: Vec::new(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            extensions: IndexMap::new(),
            servers: Vec::new(),
        }
    }

    fn spec_of(ops: Vec<ResolvedOperation>) -> ResolvedSpec {
        ResolvedSpec {
            title: "T".into(),
            version: "0".into(),
            base_url: "http://x".into(),
            operations: ops,
            servers: Vec::new(),
        }
    }

    #[test]
    fn sanitize_replaces_special_chars() {
        assert_eq!(sanitize_tool_name("getPetById"), "getpetbyid");
        assert_eq!(sanitize_tool_name("create user"), "create-user");
        assert_eq!(sanitize_tool_name("Foo::Bar"), "foo-bar");
        assert_eq!(sanitize_tool_name("---trim---"), "trim");
        assert_eq!(sanitize_tool_name(""), "op");
        assert_eq!(sanitize_tool_name("ok-name_v2.1"), "ok-name_v2.1");
    }

    #[test]
    fn sanitize_truncates_to_sixty_four() {
        let raw = "a".repeat(120);
        let san = sanitize_tool_name(&raw);
        assert!(san.len() <= 64);
    }

    #[test]
    fn unique_appends_suffix_on_collision() {
        let mut existing: IndexMap<String, ToolEntry> = IndexMap::new();
        existing.insert(
            "foo".to_string(),
            ToolEntry {
                op_index: 0,
                description: String::new(),
                input_schema: json!({}),
            },
        );
        assert_eq!(unique_name("foo", &existing), "foo-2");
        existing.insert(
            "foo-2".to_string(),
            ToolEntry {
                op_index: 1,
                description: String::new(),
                input_schema: json!({}),
            },
        );
        assert_eq!(unique_name("foo", &existing), "foo-3");
    }

    #[test]
    fn tag_filter_include_admits_only_matching() {
        let op = op("list", &["taga"]);
        assert!(tag_filter_admits(&op, &["taga".into()], &[]));
        assert!(!tag_filter_admits(&op, &["tagb".into()], &[]));
    }

    #[test]
    fn tag_filter_exclude_rejects_matching() {
        let op = op("list", &["taga"]);
        assert!(!tag_filter_admits(&op, &[], &["taga".into()]));
        assert!(tag_filter_admits(&op, &[], &["tagb".into()]));
    }

    #[test]
    fn untagged_op_belongs_to_synthetic_default() {
        let op = op("list", &[]);
        assert!(tag_filter_admits(&op, &["default".into()], &[]));
        assert!(!tag_filter_admits(&op, &["taga".into()], &[]));
        assert!(!tag_filter_admits(&op, &[], &["default".into()]));
    }

    #[test]
    fn registry_preserves_spec_order_after_filter() {
        let spec = spec_of(vec![
            op("first", &["taga"]),
            op("second", &["tagb"]),
            op("third", &["taga"]),
        ]);
        let reg = build_registry(&spec, &["taga".into()], &[]);
        let names: Vec<&str> = reg.keys().map(String::as_str).collect();
        assert_eq!(names, vec!["first", "third"]);
    }

    #[test]
    fn build_args_routes_by_parameter_location() {
        let p = |name: &str, loc: ParameterLocation| ResolvedParameter {
            name: name.to_string(),
            location: loc,
            required: false,
            deprecated: false,
            style: "form".to_string(),
            explode: false,
            schema: bare_schema(),
            description: None,
            extensions: IndexMap::new(),
        };
        let op = ResolvedOperation {
            operation_id: "x".into(),
            method: HttpMethod::Get,
            path_template: "/{a}".into(),
            summary: None,
            description: None,
            deprecated: false,
            parameters: vec![
                p("a", ParameterLocation::Path),
                p("b", ParameterLocation::Query),
                p("c", ParameterLocation::Header),
                p("d", ParameterLocation::Cookie),
            ],
            request_body: None,
            responses: IndexMap::new(),
            security: Vec::new(),
            tags: Vec::new(),
            extensions: IndexMap::new(),
            servers: Vec::new(),
        };

        let args = json!({"a": 1, "b": "two", "c": true, "d": "cookie"});
        let prog = build_programmatic_args(&op, &args).expect("ok");
        assert_eq!(prog.path.get("a").map(String::as_str), Some("1"));
        assert_eq!(prog.query.get("b").map(String::as_str), Some("two"));
        assert_eq!(prog.header.get("c").map(String::as_str), Some("true"));
        assert_eq!(prog.cookie.get("d").map(String::as_str), Some("cookie"));
    }

    #[test]
    fn build_args_rejects_unknown_argument() {
        let op = op("x", &[]);
        let args = json!({"zzz": 1});
        let err = build_programmatic_args(&op, &args).unwrap_err();
        assert!(err.contains("unknown argument 'zzz'"));
    }

    fn bare_schema() -> ResolvedSchema {
        ResolvedSchema {
            type_name: None,
            format: None,
            description: None,
            default: None,
            enum_values: Vec::new(),
            nullable: false,
            read_only: false,
            write_only: false,
            is_recursive: false,
            pattern: None,
            min_length: None,
            max_length: None,
            minimum: None,
            maximum: None,
            multiple_of: None,
            exclusive_minimum: false,
            exclusive_maximum: false,
            min_items: None,
            max_items: None,
            unique_items: false,
            additional_properties: true,
            properties: IndexMap::new(),
            items: None,
        }
    }
}
