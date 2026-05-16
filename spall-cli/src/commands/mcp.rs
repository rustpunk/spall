//! `spall mcp <api>` subcommand: serve a registered API over Model
//! Context Protocol on stdio.

use clap::{Arg, ArgAction, ArgMatches, Command};
use miette::Result;
use spall_config::registry::{ApiEntry, ApiRegistry};
use spall_core::value::SpallValue;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::SpallCliError;

/// Build the `mcp` subcommand. Mirrors the shape of `commands::arazzo`.
pub fn mcp_cmd() -> Command {
    Command::new("mcp")
        .about("Serve a registered API over Model Context Protocol (stdio)")
        .arg(
            Arg::new("api")
                .required(true)
                .help("Registered API name (must already be added via `spall api add`)"),
        )
        .arg(
            Arg::new("transport")
                .long("spall-transport")
                .value_parser(["stdio", "http"])
                .default_value("stdio")
                .help("Transport: 'stdio' (default, for Claude Desktop / config-launched servers) or 'http' (Streamable HTTP per MCP spec 2025-06-18, for reverse-proxy / hosted servers)."),
        )
        .arg(
            Arg::new("port")
                .long("spall-port")
                .value_parser(clap::value_parser!(u16))
                .help("Listen port when --spall-transport=http (default 8765). Pass 0 to let the kernel pick a free port; the bound port is printed to stderr."),
        )
        .arg(
            Arg::new("bind")
                .long("spall-bind")
                .help("Bind interface when --spall-transport=http (default 127.0.0.1). The MCP spec recommends localhost-only by default; opt into broader binds explicitly."),
        )
        .arg(
            Arg::new("allowed_origin")
                .long("spall-allowed-origin")
                .action(ArgAction::Append)
                .help("Allowlist of Origin header values when --spall-transport=http (repeatable). When non-empty, requests with a non-matching Origin get HTTP 403 — mitigates DNS rebinding."),
        )
        .arg(
            Arg::new("include")
                .long("spall-include")
                .action(ArgAction::Append)
                .help("Only expose operations carrying this OpenAPI tag (repeatable). Untagged operations belong to the synthetic tag 'default'."),
        )
        .arg(
            Arg::new("exclude")
                .long("spall-exclude")
                .action(ArgAction::Append)
                .help("Hide operations carrying this OpenAPI tag (repeatable)."),
        )
        .arg(
            Arg::new("max_tools")
                .long("spall-max-tools")
                .value_parser(clap::value_parser!(usize))
                .help("Deterministically truncate the filtered registry to N tools. Order: alphabetical by first tag, then spec order within tag."),
        )
        .arg(
            Arg::new("list_tags")
                .long("spall-list-tags")
                .action(ArgAction::SetTrue)
                .help("Load the spec, print 'tag\\tcount\\tsample-op-id' TSV to stdout, and exit without starting the server. Honors --spall-include / --spall-exclude."),
        )
        .arg(
            Arg::new("auth_tool")
                .long("spall-auth-tool")
                .action(ArgAction::Append)
                .help("Per-tool auth profile override in the form <tool>=<profile> (repeatable). <tool> matches either the tool name from tools/list or the raw operationId; <profile> must exist in the API's [profiles.*] block."),
        )
}

/// Dispatcher for the `mcp` subcommand.
#[must_use = "dropping the Result swallows server startup and runtime errors"]
pub async fn handle_mcp(
    matches: &ArgMatches,
    registry: &ApiRegistry,
    cache_dir: &Path,
) -> Result<()> {
    let api_name = matches
        .get_one::<String>("api")
        .ok_or_else(|| SpallCliError::Usage("API name required".to_string()))?
        .clone();

    let default_entry = registry
        .resolve_profile(&api_name, None)
        .ok_or_else(|| SpallCliError::Usage(format!("Unknown API: {}", api_name)))?;

    let proxy = crate::http::resolve_env_proxy();
    let raw = crate::fetch::load_raw(&default_entry.source, cache_dir, proxy.as_deref())
        .await
        .map_err(|e| SpallCliError::SpecLoadFailed {
            api: api_name.clone(),
            source: default_entry.source.clone(),
            cause: spall_core::error::SpallCoreError::InvalidSource(e.to_string()),
        })?;

    let spec = spall_core::cache::load_or_resolve(&default_entry.source, &raw, cache_dir).map_err(
        |e| SpallCliError::SpecLoadFailed {
            api: api_name.clone(),
            source: default_entry.source.clone(),
            cause: e,
        },
    )?;

    let include: Vec<String> = matches
        .get_many::<String>("include")
        .map(|vals| vals.cloned().collect())
        .unwrap_or_default();
    let exclude: Vec<String> = matches
        .get_many::<String>("exclude")
        .map(|vals| vals.cloned().collect())
        .unwrap_or_default();

    if matches.get_flag("list_tags") {
        crate::mcp::list_tags(&spec, &include, &exclude);
        return Ok(());
    }

    let max_tools = matches.get_one::<usize>("max_tools").copied();

    let auth_tool = parse_auth_tool_flags(matches.get_many::<String>("auth_tool"))?;
    let profiles = resolve_auth_profiles(&api_name, registry, &spec, &default_entry, &auth_tool)?;

    // Reuses the workspace-global `--spall-verbose` flag declared in
    // `main.rs::cli()` (also used by the request-execution path for
    // header logging). For the MCP subcommand, this means: emit
    // redacted per-call diagnostics to stderr per docs/operations/
    // mcp.md#debugging. Stdout JSON-RPC discipline stays intact.
    let verbose = matches.get_flag("spall-verbose");

    let transport = matches
        .get_one::<String>("transport")
        .map(String::as_str)
        .unwrap_or("stdio");
    match transport {
        "stdio" => crate::mcp::run(
            api_name, spec, profiles, include, exclude, max_tools, auth_tool, verbose,
        )
        .await
        .map_err(Into::into),
        "http" => {
            let port = matches
                .get_one::<u16>("port")
                .copied()
                .unwrap_or(crate::mcp::http::DEFAULT_PORT);
            let bind = matches
                .get_one::<String>("bind")
                .map(String::as_str)
                .unwrap_or(crate::mcp::http::DEFAULT_BIND);
            let listen_addr: std::net::SocketAddr = format!("{}:{}", bind, port).parse().map_err(
                |e| SpallCliError::Usage(format!("invalid --spall-bind '{}:{}': {}", bind, port, e)),
            )?;
            let allowed_origins: Vec<String> = matches
                .get_many::<String>("allowed_origin")
                .map(|vals| vals.cloned().collect())
                .unwrap_or_default();
            crate::mcp::http::run_http(
                api_name,
                spec,
                profiles,
                include,
                exclude,
                max_tools,
                auth_tool,
                listen_addr,
                allowed_origins,
                verbose,
            )
            .await
            .map_err(Into::into)
        }
        other => Err(SpallCliError::Usage(format!("unknown transport '{}'", other)).into()),
    }
}

/// Parse `--spall-auth-tool <tool>=<profile>` instances into a map.
/// Each value must contain exactly one `=` separator and non-empty
/// halves; malformed entries fail loudly so a typo doesn't silently
/// fall through to the default profile at dispatch time.
fn parse_auth_tool_flags(
    values: Option<clap::parser::ValuesRef<String>>,
) -> Result<HashMap<String, String>> {
    let mut out: HashMap<String, String> = HashMap::new();
    let Some(values) = values else {
        return Ok(out);
    };
    for raw in values {
        let Some((tool, profile)) = raw.split_once('=') else {
            return Err(SpallCliError::Usage(format!(
                "--spall-auth-tool value '{}' must be <tool>=<profile>",
                raw
            ))
            .into());
        };
        let tool = tool.trim();
        let profile = profile.trim();
        if tool.is_empty() || profile.is_empty() {
            return Err(SpallCliError::Usage(format!(
                "--spall-auth-tool value '{}' has empty <tool> or <profile>",
                raw
            ))
            .into());
        }
        out.insert(tool.to_string(), profile.to_string());
    }
    Ok(out)
}

/// Validate every profile referenced by `--spall-auth-tool` or by an
/// `x-mcp-auth-profile` extension. Profiles that don't exist in the
/// API's `[profile.*]` block surface ALL unknown names at once with
/// their call-site attribution (which flag / extension on which
/// operation introduced them), sorted for deterministic output. The
/// `[profile.*]` membership is the canonical validation here.
///
/// Per #19: profile resolution is deferred to first `tools/call`
/// dispatch via [`crate::mcp::AuthProfiles::resolve`]. This function
/// only builds the validated-name set + the registry/api_name needed
/// for the lazy resolver; no `ApiEntry` overlay is materialized here.
fn resolve_auth_profiles(
    api_name: &str,
    registry: &ApiRegistry,
    spec: &spall_core::ir::ResolvedSpec,
    default_entry: &ApiEntry,
    auth_tool: &HashMap<String, String>,
) -> Result<crate::mcp::AuthProfiles> {
    // Track each referenced profile with where it was referenced from.
    // BTreeMap (not HashSet) so error output is alphabetically stable.
    let mut needed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut auth_tool_sorted: Vec<(&String, &String)> = auth_tool.iter().collect();
    auth_tool_sorted.sort_by_key(|(k, _)| k.as_str());
    for (tool_key, profile) in auth_tool_sorted {
        needed
            .entry(profile.clone())
            .or_default()
            .push(format!("--spall-auth-tool {}={}", tool_key, profile));
    }
    for op in &spec.operations {
        if let Some(SpallValue::Str(p)) = op.extensions.get("x-mcp-auth-profile") {
            needed
                .entry(p.clone())
                .or_default()
                .push(format!("x-mcp-auth-profile on operation '{}'", op.operation_id));
        }
    }

    let mut unknown: Vec<(String, Vec<String>)> = Vec::new();
    let mut validated: HashSet<String> = HashSet::new();
    for (profile, sources) in needed {
        if !default_entry.profiles.contains_key(&profile) {
            unknown.push((profile, sources));
            continue;
        }
        validated.insert(profile);
    }

    if !unknown.is_empty() {
        let mut configured: Vec<&String> = default_entry.profiles.keys().collect();
        configured.sort();
        let report = unknown
            .iter()
            .map(|(p, srcs)| format!("  '{}' (referenced from: {})", p, srcs.join("; ")))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(SpallCliError::Usage(format!(
            "auth profile(s) not configured for api '{}':\n{}\nconfigured profiles in this api: {:?}",
            api_name, report, configured,
        ))
        .into());
    }

    Ok(crate::mcp::AuthProfiles::new(
        default_entry.clone(),
        validated,
        Arc::new(registry.clone()),
        api_name.to_string(),
    ))
}
