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

pub mod http;
pub mod schema;

use indexmap::IndexMap;
use serde_json::{json, Value};
use spall_config::registry::{ApiEntry, ApiRegistry};
use spall_core::ir::{HttpMethod, ParameterLocation, ResolvedOperation, ResolvedParameter, ResolvedSpec};
use spall_core::value::SpallValue;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
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

/// Claude Desktop silently truncates `tools/list` past ~100 entries
/// (modelcontextprotocol/discussions/537). When the filtered registry
/// exceeds this hint, `run()` emits a startup warning naming the most
/// populated tags so the user can craft a `--spall-include` filter.
const MAX_TOOL_COUNT_HINT: usize = 100;

/// How many tag buckets to surface in the warning text.
const WARNING_TAG_HISTOGRAM_TOP_N: usize = 5;

/// One tool in the dispatch registry. Built once at startup and never
/// mutated thereafter.
///
/// `op_index` indexes into `spec.operations` by position. This is safe
/// because the registry and the `spec` passed to `handle_tools_call`
/// are the same instance for the server's lifetime — the spec is loaded
/// once at startup and never reloaded. If hot-reload is ever added,
/// store the operation by id instead.
///
/// `annotations` is the per-MCP-spec hint block surfaced on
/// `tools/list`. Derived from the HTTP method at build time and merged
/// with the operation's `x-mcp-annotations` extension when present.
///
/// `auth_profile` is `Some(name)` when the operation should dispatch
/// against a profile-overlaid `ApiEntry` rather than the default. The
/// name resolves to a pre-cached entry in [`AuthProfiles`].
pub(crate) struct ToolEntry {
    op_index: usize,
    description: String,
    input_schema: Value,
    annotations: Value,
    auth_profile: Option<String>,
}

/// `ApiEntry`s for MCP tool dispatch — default + lazily-resolved overlays.
///
/// `default` is the no-overlay entry (used by tools without a profile
/// override) and is materialized at startup.
///
/// `validated` is the set of profile names referenced by
/// `--spall-auth-tool` or `x-mcp-auth-profile` and confirmed to exist
/// in the API's `[profile.*]` block — typos surface at startup, not at
/// first dispatch.
///
/// `cache` is populated lazily: the first `tools/call` against a tool
/// with `auth_profile = Some(name)` calls `registry.resolve_profile`
/// once and inserts the overlaid entry; subsequent calls reuse it.
/// Profiles in `validated` but never invoked stay un-resolved for the
/// server's lifetime — see issue #19 for the OAuth-refresh
/// blast-radius motivation.
pub struct AuthProfiles {
    pub default: ApiEntry,
    pub validated: HashSet<String>,
    /// Cache holds the overlaid `ApiEntry`; per-call token freshness
    /// is handled downstream in `crate::auth::resolve`.
    cache: tokio::sync::RwLock<HashMap<String, ApiEntry>>,
    registry: Arc<ApiRegistry>,
    api_name: String,
}

/// Failure modes for [`AuthProfiles::resolve`]. `NotValidated` is an
/// internal-bug sentinel — the dispatcher asked for a profile startup
/// never approved. `RegistryMiss` means the registry rejected the
/// resolution at first-call time (would require a registry mutation
/// in flight; no current code path does this).
#[derive(Debug)]
pub(crate) enum ResolveErr {
    NotValidated { name: String },
    RegistryMiss { name: String },
}

impl std::fmt::Display for ResolveErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveErr::NotValidated { name } => write!(
                f,
                "internal: auth profile '{}' not validated at startup",
                name
            ),
            ResolveErr::RegistryMiss { name } => write!(
                f,
                "auth profile '{}' resolution failed: not found in registry",
                name
            ),
        }
    }
}

impl AuthProfiles {
    /// Build from the validated set + the registry/api_name needed for
    /// lazy resolution. The cache starts empty.
    pub fn new(
        default: ApiEntry,
        validated: HashSet<String>,
        registry: Arc<ApiRegistry>,
        api_name: String,
    ) -> Self {
        Self {
            default,
            validated,
            cache: tokio::sync::RwLock::new(HashMap::new()),
            registry,
            api_name,
        }
    }

    /// Resolve the dispatch target for a tool call. `None` returns a
    /// borrow of the default entry; `Some(name)` consults the lazy
    /// cache and on miss calls `registry.resolve_profile` exactly once
    /// (double-checked lock — two concurrent first-callers race past
    /// the read lock; the loser sees the winner's insert under the
    /// write lock). The owned variant carries a clone out of the cache
    /// so the read lock can be released before the dispatcher runs.
    pub(crate) async fn resolve(
        &self,
        profile: Option<&str>,
    ) -> Result<Cow<'_, ApiEntry>, ResolveErr> {
        let Some(name) = profile else {
            return Ok(Cow::Borrowed(&self.default));
        };
        if !self.validated.contains(name) {
            return Err(ResolveErr::NotValidated {
                name: name.to_string(),
            });
        }
        {
            let r = self.cache.read().await;
            if let Some(entry) = r.get(name) {
                return Ok(Cow::Owned(entry.clone()));
            }
        }
        let mut w = self.cache.write().await;
        if let Some(entry) = w.get(name) {
            return Ok(Cow::Owned(entry.clone()));
        }
        let entry = self
            .registry
            .resolve_profile(&self.api_name, Some(name))
            .ok_or_else(|| ResolveErr::RegistryMiss {
                name: name.to_string(),
            })?;
        w.insert(name.to_string(), entry.clone());
        Ok(Cow::Owned(entry))
    }
}

/// Build the tool registry from `spec` applying the include/exclude tag
/// filter. Returns tools in spec order (`IndexMap` preserves insertion).
///
/// When `max_tools` is `Some(N)` and the filtered count exceeds `N`,
/// the registry is deterministically truncated to `N` entries before
/// insertion — order documented in [`truncate_deterministically`].
///
/// `auth_tool` maps a finalized tool name to a profile name the
/// dispatcher should use in place of the default `ApiEntry`. Entries in
/// the map whose key doesn't match any final tool name are surfaced to
/// the caller as `unmatched_auth_tool_keys` — `run()` warns about them
/// on stderr.
fn build_registry(
    spec: &ResolvedSpec,
    include: &[String],
    exclude: &[String],
    max_tools: Option<usize>,
    auth_tool: &HashMap<String, String>,
) -> (IndexMap<String, ToolEntry>, Vec<String>) {
    let filtered: Vec<(usize, &ResolvedOperation)> = spec
        .operations
        .iter()
        .enumerate()
        .filter(|(_, op)| tag_filter_admits(op, include, exclude))
        .collect();

    let ordered = match max_tools {
        Some(cap) if filtered.len() > cap => truncate_deterministically(filtered, cap),
        _ => filtered,
    };

    let mut out: IndexMap<String, ToolEntry> = IndexMap::new();
    let mut matched_keys: std::collections::HashSet<&str> =
        std::collections::HashSet::new();
    for (idx, op) in ordered {
        let raw = sanitize_tool_name(&op.operation_id);
        let name = unique_name(&raw, &out);
        let description = build_description(op);
        let input_schema = schema::operation_input_schema(op);
        let annotations = derive_annotations(op);
        let auth_profile = resolve_auth_profile(op, &name, auth_tool, &mut matched_keys);
        out.insert(
            name,
            ToolEntry {
                op_index: idx,
                description,
                input_schema,
                annotations,
                auth_profile,
            },
        );
    }

    let unmatched: Vec<String> = auth_tool
        .keys()
        .filter(|k| !matched_keys.contains(k.as_str()))
        .cloned()
        .collect();
    (out, unmatched)
}

/// MCP `annotations` block for one tool, per spec 2025-06-18 §tools.
/// Defaults are derived from HTTP method; `x-mcp-annotations` on the
/// operation overrides each hint field-by-field. Returns `{}` when no
/// hints apply (POST without an override).
fn derive_annotations(op: &ResolvedOperation) -> Value {
    let mut map = serde_json::Map::new();
    let (read_only, destructive, idempotent) = match op.method {
        HttpMethod::Get | HttpMethod::Head | HttpMethod::Options | HttpMethod::Trace => {
            (Some(true), Some(false), Some(true))
        }
        HttpMethod::Put | HttpMethod::Delete => (Some(false), Some(true), Some(true)),
        HttpMethod::Patch => (Some(false), Some(true), Some(false)),
        HttpMethod::Post => (None, None, None),
    };
    if let Some(b) = read_only {
        map.insert("readOnlyHint".to_string(), Value::Bool(b));
    }
    if let Some(b) = destructive {
        map.insert("destructiveHint".to_string(), Value::Bool(b));
    }
    if let Some(b) = idempotent {
        map.insert("idempotentHint".to_string(), Value::Bool(b));
    }

    // Auto-derive a human-readable `title` from `op.summary` so MCP
    // clients (Claude Desktop, Cursor, ChatGPT Apps) get a friendly
    // display string in tool pickers instead of the sanitized
    // operationId. `entry().or_insert` is defensive — the override
    // loop below uses unconditional `map.insert`, so an explicit
    // `x-mcp-annotations.title` will still win even if this fires
    // first. If `op.summary` is absent and no override is supplied,
    // the field is omitted entirely (cleaner than synthesizing a
    // title from operationId; clients fall back to the tool name).
    if let Some(summary) = &op.summary {
        map.entry("title".to_string())
            .or_insert_with(|| Value::String(summary.clone()));
    }

    // Field-by-field override from x-mcp-annotations. Spec authors can
    // flip any hint, set `openWorldHint` (which spall doesn't derive),
    // or supply `title`. Unknown keys pass through so future MCP spec
    // additions don't need a spall release.
    if let Some(SpallValue::Object(over)) = op.extensions.get("x-mcp-annotations") {
        for (k, v) in over {
            map.insert(k.clone(), Value::from(v));
        }
    }

    Value::Object(map)
}

/// Pick the auth profile a tool should dispatch against. CLI flag takes
/// precedence over the `x-mcp-auth-profile` extension; the flag's key
/// is matched against the finalized tool name AND the raw
/// `operationId` so users can write either form. The matched key (if
/// any) is recorded so `build_registry` can surface unmatched CLI keys.
fn resolve_auth_profile<'a>(
    op: &'a ResolvedOperation,
    name: &str,
    auth_tool: &'a HashMap<String, String>,
    matched: &mut std::collections::HashSet<&'a str>,
) -> Option<String> {
    // Try the sanitized tool name first, fall back to the raw
    // operationId. Dedupe so a tool whose sanitized form equals its
    // operationId doesn't get probed twice. The two-form match exists
    // because users may have only seen the post-sanitize name in
    // tools/list output OR may write against the raw operationId from
    // the spec.
    let mut probes: Vec<&str> = vec![name];
    if op.operation_id.as_str() != name {
        probes.push(op.operation_id.as_str());
    }
    for key in probes {
        if let Some((k, p)) = auth_tool.get_key_value(key) {
            matched.insert(k.as_str());
            return Some(p.clone());
        }
    }
    if let Some(SpallValue::Str(s)) = op.extensions.get("x-mcp-auth-profile") {
        return Some(s.clone());
    }
    None
}

/// Returns the first OpenAPI tag for an operation, or the synthetic
/// `"default"` bucket when it has none. Used as the primary sort key
/// for deterministic truncation and for tag-histogram bookkeeping.
fn first_tag(op: &ResolvedOperation) -> &str {
    op.tags.first().map(String::as_str).unwrap_or("default")
}

/// Pick the first `cap` operations under a deterministic ordering so
/// truncation is stable across invocations on the same spec.
///
/// Order: alphabetical by first tag (or the synthetic `"default"`),
/// then by operation index within that tag (spec order). After
/// truncation the selected ops are re-sorted by index so the resulting
/// `tools/list` still reads in spec order within the chosen subset.
fn truncate_deterministically(
    filtered: Vec<(usize, &ResolvedOperation)>,
    cap: usize,
) -> Vec<(usize, &ResolvedOperation)> {
    let mut entries = filtered;
    entries.sort_by(|a, b| first_tag(a.1).cmp(first_tag(b.1)).then_with(|| a.0.cmp(&b.0)));
    entries.truncate(cap);
    entries.sort_by_key(|a| a.0);
    entries
}

/// Build `(tag, count)` pairs for the >100-tools warning. Each op
/// contributes once to every tag it carries; untagged ops fall into
/// `"default"`. Sorted by count desc then tag asc, capped at `top_n`.
fn tag_histogram(
    registry: &IndexMap<String, ToolEntry>,
    spec: &ResolvedSpec,
    top_n: usize,
) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for entry in registry.values() {
        let op = match spec.operations.get(entry.op_index) {
            Some(op) => op,
            None => continue,
        };
        if op.tags.is_empty() {
            *counts.entry("default".to_string()).or_insert(0) += 1;
        } else {
            for tag in &op.tags {
                *counts.entry(tag.clone()).or_insert(0) += 1;
            }
        }
    }
    let mut v: Vec<(String, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(top_n);
    v
}

/// Print `tag\tcount\tsample-op-id` TSV for every tag in the filtered
/// spec, then return. Used by the `--spall-list-tags` early-exit path;
/// honors the same include/exclude filters the server would apply.
///
/// Untagged operations belong to the synthetic tag `"default"`. An op
/// carrying multiple tags contributes once to each.
pub fn list_tags(spec: &ResolvedSpec, include: &[String], exclude: &[String]) {
    let mut buckets: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for op in spec
        .operations
        .iter()
        .filter(|op| tag_filter_admits(op, include, exclude))
    {
        if op.tags.is_empty() {
            let e = buckets
                .entry("default".to_string())
                .or_insert_with(|| (0, op.operation_id.clone()));
            e.0 += 1;
        } else {
            for tag in &op.tags {
                let e = buckets
                    .entry(tag.clone())
                    .or_insert_with(|| (0, op.operation_id.clone()));
                e.0 += 1;
            }
        }
    }
    println!("tag\tcount\tsample-op-id");
    for (tag, (count, sample)) in buckets {
        println!("{}\t{}\t{}", tag, count, sample);
    }
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

/// Build the tool registry and emit the startup banner + any tool-count
/// warnings to stderr. Shared by the stdio (`run`) and HTTP
/// (`http::run_http`) entry points so banner / warning copy stays
/// identical across transports.
pub(crate) fn prepare_server(
    api_name: &str,
    transport_label: &str,
    spec: &ResolvedSpec,
    include: &[String],
    exclude: &[String],
    max_tools: Option<usize>,
    auth_tool: &HashMap<String, String>,
) -> IndexMap<String, ToolEntry> {
    let (registry, unmatched_auth) = build_registry(spec, include, exclude, max_tools, auth_tool);
    eprintln!(
        "spall mcp: serving '{}' over {} ({} tool{})",
        api_name,
        transport_label,
        registry.len(),
        if registry.len() == 1 { "" } else { "s" }
    );
    for key in &unmatched_auth {
        eprintln!(
            "spall mcp: --spall-auth-tool key '{}' did not match any registered tool (filtered out, or unknown operationId)",
            key,
        );
    }
    if registry.len() > MAX_TOOL_COUNT_HINT {
        let hist = tag_histogram(&registry, spec, WARNING_TAG_HISTOGRAM_TOP_N)
            .into_iter()
            .map(|(t, c)| format!("{}={}", t, c))
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!(
            "spall mcp: WARNING {} tools exceeds the ~{}-tool cap most MCP clients (incl. Claude Desktop) silently truncate at; pass --spall-include <tag> or --spall-max-tools <N> to trim.",
            registry.len(),
            MAX_TOOL_COUNT_HINT,
        );
        eprintln!("spall mcp: top tags by population: {}", hist);
    }
    registry
}

/// Server entry point. Builds the tool registry then serves stdio
/// JSON-RPC until EOF on stdin.
#[must_use = "ignoring this Result swallows server-side errors"]
pub async fn run(
    api_name: String,
    spec: ResolvedSpec,
    profiles: AuthProfiles,
    include: Vec<String>,
    exclude: Vec<String>,
    max_tools: Option<usize>,
    auth_tool: HashMap<String, String>,
) -> Result<(), crate::SpallCliError> {
    let registry = prepare_server(
        &api_name, "stdio", &spec, &include, &exclude, max_tools, &auth_tool,
    );

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::stdout();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_line(&line, &spec, &profiles, &registry).await;
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
pub(crate) async fn handle_line(
    line: &str,
    spec: &ResolvedSpec,
    profiles: &AuthProfiles,
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
        "tools/list" => Some(rpc_result(id, handle_tools_list(spec, registry))),
        "tools/call" => {
            let result = handle_tools_call(&params, spec, profiles, registry).await;
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

fn handle_tools_list(spec: &ResolvedSpec, registry: &IndexMap<String, ToolEntry>) -> Value {
    let tools: Vec<Value> = registry
        .iter()
        .map(|(name, entry)| {
            let tags: Vec<Value> = spec
                .operations
                .get(entry.op_index)
                .map(|op| op.tags.iter().cloned().map(Value::String).collect())
                .unwrap_or_default();
            json!({
                "name": name,
                "description": entry.description,
                "inputSchema": entry.input_schema,
                "annotations": entry.annotations,
                "_meta": { "spall.tags": tags },
            })
        })
        .collect();
    json!({ "tools": tools })
}

async fn handle_tools_call(
    params: &Value,
    spec: &ResolvedSpec,
    profiles: &AuthProfiles,
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

    let resolved = match profiles.resolve(tool.auth_profile.as_deref()).await {
        Ok(r) => r,
        Err(e) => return tool_error(&format!("{}", e)),
    };
    let entry: &ApiEntry = &resolved;

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
/// matching `ResolvedParameter` slot in `ProgrammaticArgs`, and handle
/// the `body` reserved key when the operation has a request body.
///
/// Array arguments are expanded per the parameter's OpenAPI `style` and
/// `explode` flags: query params with the default `form` + `explode:
/// true` produce `?ids=1&ids=2&ids=3` via `query_extras`; non-exploded
/// forms and `simple`-style path/header/cookie params produce
/// comma-joined values.
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

    let by_name: BTreeMap<&str, &ResolvedParameter> =
        op.parameters.iter().map(|p| (p.name.as_str(), p)).collect();

    for (key, value) in obj {
        if key == "body" && op.request_body.is_some() {
            prog.body = Some(value.clone());
            continue;
        }
        let param = match by_name.get(key.as_str()) {
            Some(p) => *p,
            None => return Err(format!("unknown argument '{}'", key)),
        };
        place_argument(&mut prog, param, key, value);
    }
    Ok(prog)
}

/// Place one argument into the right `ProgrammaticArgs` slot, expanding
/// arrays per the parameter's OpenAPI `style` / `explode` flags.
fn place_argument(
    prog: &mut ProgrammaticArgs,
    param: &ResolvedParameter,
    name: &str,
    value: &Value,
) {
    match (param.location, value) {
        (ParameterLocation::Query, Value::Array(items)) => {
            // OpenAPI defaults for query: style = "form", explode = true.
            // Form + explode → ?ids=1&ids=2; form + no-explode → ?ids=1,2.
            let explode = explode_default_for(&param.style, param.explode, true);
            if explode {
                for item in items {
                    prog.query_extras
                        .push((name.to_string(), scalar_to_string(item)));
                }
            } else {
                let joined = items
                    .iter()
                    .map(scalar_to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                prog.query.insert(name.to_string(), joined);
            }
        }
        (ParameterLocation::Path, Value::Array(items)) => {
            // OpenAPI default for path: style = "simple". `simple` and
            // `simple + explode` both comma-join scalars; matrix/label
            // styles produce different separators and are uncommon
            // enough to defer.
            let joined = items
                .iter()
                .map(scalar_to_string)
                .collect::<Vec<_>>()
                .join(",");
            prog.path.insert(name.to_string(), joined);
        }
        (ParameterLocation::Header, Value::Array(items)) => {
            // RFC 9110 §5.3 — multi-value headers are comma-joined.
            let joined = items
                .iter()
                .map(scalar_to_string)
                .collect::<Vec<_>>()
                .join(",");
            prog.header.insert(name.to_string(), joined);
        }
        (ParameterLocation::Cookie, Value::Array(items)) => {
            let joined = items
                .iter()
                .map(scalar_to_string)
                .collect::<Vec<_>>()
                .join(",");
            prog.cookie.insert(name.to_string(), joined);
        }
        (ParameterLocation::Path, v) => {
            prog.path.insert(name.to_string(), scalar_to_string(v));
        }
        (ParameterLocation::Query, v) => {
            prog.query.insert(name.to_string(), scalar_to_string(v));
        }
        (ParameterLocation::Header, v) => {
            prog.header.insert(name.to_string(), scalar_to_string(v));
        }
        (ParameterLocation::Cookie, v) => {
            prog.cookie.insert(name.to_string(), scalar_to_string(v));
        }
    }
}

/// OpenAPI 3.0/3.1 explode defaults: `true` only when style is `form`,
/// `false` otherwise. If the spec authors explicitly set `explode`, the
/// resolver carries their choice forward; only treat it as a default
/// when the parser left it at the all-zeroes fallback.
fn explode_default_for(style: &str, explode: bool, fallback_for_form: bool) -> bool {
    if explode {
        return true;
    }
    if style == "form" {
        // The IR populates `explode` from the spec; we can't distinguish
        // "spec said false" from "spec didn't say". Default form params
        // to exploded per OpenAPI's `form` default — matches what curl /
        // Postman would do for a missing `explode` field.
        return fallback_for_form;
    }
    false
}

/// Coerce a JSON scalar to its string form. Objects fall back to JSON
/// text (the param is type: object, not array — OpenAPI's `deepObject`
/// style for query params is rare and out of v1 scope).
fn scalar_to_string(v: &Value) -> String {
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
                annotations: json!({}),
                auth_profile: None,
            },
        );
        assert_eq!(unique_name("foo", &existing), "foo-2");
        existing.insert(
            "foo-2".to_string(),
            ToolEntry {
                op_index: 1,
                description: String::new(),
                input_schema: json!({}),
                annotations: json!({}),
                auth_profile: None,
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
        let reg = build_registry(&spec, &["taga".into()], &[], None, &HashMap::new()).0;
        let names: Vec<&str> = reg.keys().map(String::as_str).collect();
        assert_eq!(names, vec!["first", "third"]);
    }

    #[test]
    fn truncate_deterministic_picks_same_subset_across_runs() {
        // Two tags, both populated, with operations interleaved in spec
        // order. The truncation sort key (tag asc, then op_index asc)
        // must produce a stable subset regardless of insertion timing.
        let mut ops = Vec::new();
        for i in 0..50 {
            ops.push(op(&format!("alpha-{}", i), &["alpha"]));
            ops.push(op(&format!("beta-{}", i), &["beta"]));
        }
        let spec = spec_of(ops);
        let first = build_registry(&spec, &[], &[], Some(30), &HashMap::new()).0;
        let second = build_registry(&spec, &[], &[], Some(30), &HashMap::new()).0;
        let names_a: Vec<&str> = first.keys().map(String::as_str).collect();
        let names_b: Vec<&str> = second.keys().map(String::as_str).collect();
        assert_eq!(names_a.len(), 30);
        assert_eq!(names_a, names_b, "truncation must be deterministic");
        // 30 ops, alpha-bucket first (alphabetical). Alpha had 50 ops at
        // even indices 0,2,...,98 → first 30 are alpha-0..alpha-29.
        for name in &names_a {
            assert!(
                name.starts_with("alpha-"),
                "expected alpha bucket to fill first, got {}",
                name
            );
        }
    }

    #[test]
    fn truncate_below_cap_keeps_all_ops_in_spec_order() {
        let spec = spec_of(vec![
            op("a", &["t1"]),
            op("b", &["t2"]),
            op("c", &["t1"]),
        ]);
        let reg = build_registry(&spec, &[], &[], Some(10), &HashMap::new()).0;
        let names: Vec<&str> = reg.keys().map(String::as_str).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn tag_histogram_sorts_by_count_desc_then_tag_asc() {
        let spec = spec_of(vec![
            op("a", &["users"]),
            op("b", &["users"]),
            op("c", &["users"]),
            op("d", &["orgs"]),
            op("e", &["orgs"]),
            op("f", &["billing"]),
            op("g", &[]),
        ]);
        let reg = build_registry(&spec, &[], &[], None, &HashMap::new()).0;
        let hist = tag_histogram(&reg, &spec, 10);
        assert_eq!(
            hist,
            vec![
                ("users".to_string(), 3),
                ("orgs".to_string(), 2),
                ("billing".to_string(), 1),
                ("default".to_string(), 1),
            ],
        );
    }

    #[test]
    fn tag_histogram_caps_to_top_n() {
        let mut ops = Vec::new();
        for tag in ["t1", "t2", "t3", "t4", "t5", "t6", "t7"] {
            ops.push(op(&format!("op-{}", tag), &[tag]));
        }
        let spec = spec_of(ops);
        let reg = build_registry(&spec, &[], &[], None, &HashMap::new()).0;
        let hist = tag_histogram(&reg, &spec, 3);
        assert_eq!(hist.len(), 3);
    }

    #[test]
    fn multi_tag_op_contributes_to_every_tag_bucket() {
        let spec = spec_of(vec![op("a", &["users", "admin"])]);
        let reg = build_registry(&spec, &[], &[], None, &HashMap::new()).0;
        let hist = tag_histogram(&reg, &spec, 10);
        let map: BTreeMap<String, usize> = hist.into_iter().collect();
        assert_eq!(map.get("users"), Some(&1));
        assert_eq!(map.get("admin"), Some(&1));
    }

    fn op_with_method(id: &str, method: HttpMethod) -> ResolvedOperation {
        let mut o = op(id, &[]);
        o.method = method;
        o
    }

    #[test]
    fn annotations_derive_from_http_method() {
        // GET / HEAD / OPTIONS / TRACE → readOnly + idempotent.
        for m in [
            HttpMethod::Get,
            HttpMethod::Head,
            HttpMethod::Options,
            HttpMethod::Trace,
        ] {
            let ann = derive_annotations(&op_with_method("x", m));
            assert_eq!(ann["readOnlyHint"], json!(true), "{:?}", m);
            assert_eq!(ann["destructiveHint"], json!(false), "{:?}", m);
            assert_eq!(ann["idempotentHint"], json!(true), "{:?}", m);
        }
        // PUT / DELETE → destructive + idempotent.
        for m in [HttpMethod::Put, HttpMethod::Delete] {
            let ann = derive_annotations(&op_with_method("x", m));
            assert_eq!(ann["readOnlyHint"], json!(false), "{:?}", m);
            assert_eq!(ann["destructiveHint"], json!(true), "{:?}", m);
            assert_eq!(ann["idempotentHint"], json!(true), "{:?}", m);
        }
        // PATCH → destructive, NOT idempotent.
        let ann = derive_annotations(&op_with_method("x", HttpMethod::Patch));
        assert_eq!(ann["readOnlyHint"], json!(false));
        assert_eq!(ann["destructiveHint"], json!(true));
        assert_eq!(ann["idempotentHint"], json!(false));
        // POST → no hints (server unknown intent).
        let ann = derive_annotations(&op_with_method("x", HttpMethod::Post));
        assert!(ann.as_object().unwrap().is_empty());
    }

    #[test]
    fn x_mcp_annotations_extension_overrides_field_by_field() {
        let mut o = op_with_method("x", HttpMethod::Get);
        // Flip readOnlyHint and add openWorldHint (which spall doesn't
        // derive). idempotentHint should retain the GET-derived true.
        let mut override_map: IndexMap<String, SpallValue> = IndexMap::new();
        override_map.insert("readOnlyHint".into(), SpallValue::Bool(false));
        override_map.insert("openWorldHint".into(), SpallValue::Bool(true));
        o.extensions
            .insert("x-mcp-annotations".into(), SpallValue::Object(override_map));
        let ann = derive_annotations(&o);
        assert_eq!(ann["readOnlyHint"], json!(false));
        assert_eq!(ann["openWorldHint"], json!(true));
        assert_eq!(ann["idempotentHint"], json!(true));
    }

    #[test]
    fn op_summary_auto_derives_tool_title() {
        let mut o = op_with_method("listPets", HttpMethod::Get);
        o.summary = Some("List pets owned by the caller".to_string());
        let ann = derive_annotations(&o);
        assert_eq!(
            ann["title"],
            json!("List pets owned by the caller"),
            "op.summary should auto-derive `title`",
        );
    }

    #[test]
    fn x_mcp_annotations_title_overrides_summary_derived() {
        let mut o = op_with_method("listPets", HttpMethod::Get);
        o.summary = Some("List pets owned by the caller".to_string());
        let mut override_map: IndexMap<String, SpallValue> = IndexMap::new();
        override_map.insert(
            "title".into(),
            SpallValue::Str("Show My Pets".to_string()),
        );
        o.extensions
            .insert("x-mcp-annotations".into(), SpallValue::Object(override_map));
        let ann = derive_annotations(&o);
        assert_eq!(
            ann["title"],
            json!("Show My Pets"),
            "explicit x-mcp-annotations.title must win over op.summary",
        );
    }

    #[test]
    fn absent_summary_and_no_override_omits_title_field() {
        let o = op_with_method("listPets", HttpMethod::Get);
        // No summary, no x-mcp-annotations override.
        let ann = derive_annotations(&o);
        assert!(
            ann.get("title").is_none(),
            "title must be omitted when no source is available; got {:?}",
            ann,
        );
    }

    #[test]
    fn x_mcp_auth_profile_extension_picks_up_at_build_time() {
        let mut o = op("authed", &[]);
        o.extensions
            .insert("x-mcp-auth-profile".into(), SpallValue::Str("admin".into()));
        let spec = spec_of(vec![o]);
        let reg = build_registry(&spec, &[], &[], None, &HashMap::new()).0;
        let (_name, entry) = reg.iter().next().unwrap();
        assert_eq!(entry.auth_profile.as_deref(), Some("admin"));
    }

    #[test]
    fn cli_auth_tool_flag_overrides_extension() {
        let mut o = op("authed", &[]);
        o.extensions
            .insert("x-mcp-auth-profile".into(), SpallValue::Str("admin".into()));
        let spec = spec_of(vec![o]);
        let mut flags = HashMap::new();
        flags.insert("authed".to_string(), "readonly".to_string());
        let (reg, unmatched) = build_registry(&spec, &[], &[], None, &flags);
        let (_name, entry) = reg.iter().next().unwrap();
        assert_eq!(entry.auth_profile.as_deref(), Some("readonly"));
        assert!(unmatched.is_empty());
    }

    #[test]
    fn unmatched_auth_tool_key_is_reported() {
        let spec = spec_of(vec![op("a", &[])]);
        let mut flags = HashMap::new();
        flags.insert("ghost-tool".to_string(), "admin".to_string());
        let (_, unmatched) = build_registry(&spec, &[], &[], None, &flags);
        assert_eq!(unmatched, vec!["ghost-tool".to_string()]);
    }

    #[test]
    fn cli_auth_tool_matches_raw_operation_id_or_sanitized_name() {
        // sanitize lowercases "Foo" → "foo", but the user may have
        // written the raw operationId on the CLI. Either form should
        // land on the same tool.
        let spec = spec_of(vec![op("GetThing", &[])]);
        let mut by_raw = HashMap::new();
        by_raw.insert("GetThing".to_string(), "p".to_string());
        let entry_a = build_registry(&spec, &[], &[], None, &by_raw)
            .0
            .iter()
            .next()
            .map(|(_, e)| e.auth_profile.clone())
            .unwrap();
        let mut by_sanitized = HashMap::new();
        by_sanitized.insert("getthing".to_string(), "p".to_string());
        let entry_b = build_registry(&spec, &[], &[], None, &by_sanitized)
            .0
            .iter()
            .next()
            .map(|(_, e)| e.auth_profile.clone())
            .unwrap();
        assert_eq!(entry_a, Some("p".to_string()));
        assert_eq!(entry_b, Some("p".to_string()));
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

    fn make_param(name: &str, loc: ParameterLocation, style: &str, explode: bool) -> ResolvedParameter {
        ResolvedParameter {
            name: name.to_string(),
            location: loc,
            required: false,
            deprecated: false,
            style: style.to_string(),
            explode,
            schema: bare_schema(),
            description: None,
            extensions: IndexMap::new(),
        }
    }

    fn op_with_params(params: Vec<ResolvedParameter>) -> ResolvedOperation {
        ResolvedOperation {
            operation_id: "x".into(),
            method: HttpMethod::Get,
            path_template: "/x".into(),
            summary: None,
            description: None,
            deprecated: false,
            parameters: params,
            request_body: None,
            responses: IndexMap::new(),
            security: Vec::new(),
            tags: Vec::new(),
            extensions: IndexMap::new(),
            servers: Vec::new(),
        }
    }

    #[test]
    fn array_query_with_form_explode_expands_to_repeated_pairs() {
        // OpenAPI default for query params: style=form, explode=true.
        let op = op_with_params(vec![make_param(
            "ids",
            ParameterLocation::Query,
            "form",
            true,
        )]);
        let prog = build_programmatic_args(&op, &json!({"ids": [1, 2, 3]})).unwrap();
        assert!(prog.query.is_empty(), "explode=true bypasses the single-value map");
        assert_eq!(
            prog.query_extras,
            vec![
                ("ids".to_string(), "1".to_string()),
                ("ids".to_string(), "2".to_string()),
                ("ids".to_string(), "3".to_string()),
            ]
        );
    }

    #[test]
    fn array_query_with_form_style_still_explodes_when_spec_says_false() {
        // Limitation: the IR's `explode: bool` collapses
        // "spec set false" with "spec omitted explode entirely" into
        // the same value. To match OpenAPI's documented `form` default
        // (explode=true), spall errs on the side of repetition. Specs
        // that explicitly need comma-join under `form` are uncommon
        // and would have to set `style: pipeDelimited` or
        // `spaceDelimited` to get unambiguous comma-join.
        let op = op_with_params(vec![make_param(
            "ids",
            ParameterLocation::Query,
            "form",
            false,
        )]);
        let prog = build_programmatic_args(&op, &json!({"ids": ["a", "b"]})).unwrap();
        assert_eq!(prog.query_extras.len(), 2);
    }

    #[test]
    fn array_query_with_pipe_delimited_style_comma_joins() {
        // Non-form, non-exploded styles collapse to comma-join in v1.
        let op = op_with_params(vec![make_param(
            "ids",
            ParameterLocation::Query,
            "pipeDelimited",
            false,
        )]);
        let prog = build_programmatic_args(&op, &json!({"ids": [1, 2, 3]})).unwrap();
        assert!(prog.query_extras.is_empty());
        assert_eq!(prog.query.get("ids").map(String::as_str), Some("1,2,3"));
    }

    #[test]
    fn array_path_param_comma_joins() {
        let op = op_with_params(vec![make_param(
            "ids",
            ParameterLocation::Path,
            "simple",
            false,
        )]);
        let prog = build_programmatic_args(&op, &json!({"ids": [1, 2, 3]})).unwrap();
        assert_eq!(prog.path.get("ids").map(String::as_str), Some("1,2,3"));
    }

    #[test]
    fn array_header_comma_joins_per_rfc_9110() {
        let op = op_with_params(vec![make_param(
            "X-Tags",
            ParameterLocation::Header,
            "simple",
            false,
        )]);
        let prog = build_programmatic_args(&op, &json!({"X-Tags": ["a", "b", "c"]})).unwrap();
        assert_eq!(prog.header.get("X-Tags").map(String::as_str), Some("a,b,c"));
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

    fn auth_profile_entry(name: &str, profiles: &[&str]) -> ApiEntry {
        use spall_config::registry::ProfileConfig;
        let mut map = std::collections::HashMap::new();
        for p in profiles {
            map.insert(
                (*p).to_string(),
                ProfileConfig {
                    base_url: Some(format!("https://{}-{}.example", name, p)),
                    headers: Vec::new(),
                    auth: None,
                    proxy: None,
                },
            );
        }
        ApiEntry {
            name: name.to_string(),
            source: format!("/tmp/{}.json", name),
            config_path: None,
            base_url: Some("https://default.example".to_string()),
            default_headers: Vec::new(),
            auth: None,
            proxy: None,
            profiles: map,
        }
    }

    #[tokio::test]
    async fn resolve_default_returns_borrow_of_default_entry() {
        let entry = auth_profile_entry("svc", &[]);
        let registry = Arc::new(ApiRegistry::from_entries(
            vec![entry.clone()],
            spall_config::sources::GlobalDefaults::default(),
        ));
        let profiles = AuthProfiles::new(entry, HashSet::new(), registry, "svc".to_string());
        let cow = profiles.resolve(None).await.expect("default resolves");
        assert!(matches!(cow, Cow::Borrowed(_)));
        assert_eq!(cow.name, "svc");
        assert_eq!(cow.base_url.as_deref(), Some("https://default.example"));
    }

    #[tokio::test]
    async fn resolve_concurrent_two_spawns_yield_same_profile_entry() {
        let entry = auth_profile_entry("svc", &["admin"]);
        let registry = Arc::new(ApiRegistry::from_entries(
            vec![entry.clone()],
            spall_config::sources::GlobalDefaults::default(),
        ));
        let mut validated = HashSet::new();
        validated.insert("admin".to_string());
        let profiles = Arc::new(AuthProfiles::new(
            entry,
            validated,
            registry,
            "svc".to_string(),
        ));

        let p1 = Arc::clone(&profiles);
        let p2 = Arc::clone(&profiles);
        let h1 = tokio::spawn(async move {
            p1.resolve(Some("admin")).await.map(Cow::into_owned)
        });
        let h2 = tokio::spawn(async move {
            p2.resolve(Some("admin")).await.map(Cow::into_owned)
        });
        let r1 = h1.await.expect("task 1").expect("resolve 1");
        let r2 = h2.await.expect("task 2").expect("resolve 2");
        assert_eq!(r1.base_url, r2.base_url);
        assert_eq!(r1.base_url.as_deref(), Some("https://svc-admin.example"));
    }

    #[tokio::test]
    async fn resolve_not_validated_surfaces_internal_attribution() {
        let entry = auth_profile_entry("svc", &["admin"]);
        let registry = Arc::new(ApiRegistry::from_entries(
            vec![entry.clone()],
            spall_config::sources::GlobalDefaults::default(),
        ));
        let profiles = AuthProfiles::new(entry, HashSet::new(), registry, "svc".to_string());
        let err = profiles
            .resolve(Some("ghost"))
            .await
            .expect_err("ghost is not validated");
        assert_eq!(
            format!("{}", err),
            "internal: auth profile 'ghost' not validated at startup"
        );
    }

    #[tokio::test]
    async fn resolve_registry_miss_surfaces_not_found_attribution() {
        let entry = auth_profile_entry("svc", &["admin"]);
        let empty_registry = Arc::new(ApiRegistry::from_entries(
            Vec::new(),
            spall_config::sources::GlobalDefaults::default(),
        ));
        let mut validated = HashSet::new();
        validated.insert("admin".to_string());
        let profiles =
            AuthProfiles::new(entry, validated, empty_registry, "svc".to_string());
        let err = profiles
            .resolve(Some("admin"))
            .await
            .expect_err("registry has no api");
        assert_eq!(
            format!("{}", err),
            "auth profile 'admin' resolution failed: not found in registry"
        );
    }
}
