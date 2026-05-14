//! `spall mcp <api>` subcommand: serve a registered API over Model
//! Context Protocol on stdio.

use clap::{Arg, ArgAction, ArgMatches, Command};
use miette::Result;
use spall_config::registry::{ApiEntry, ApiRegistry};
use spall_core::value::SpallValue;
use std::collections::{HashMap, HashSet};
use std::path::Path;

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
                .value_parser(["stdio"])
                .default_value("stdio")
                .help("Transport. Only 'stdio' is supported in v1; Streamable HTTP is a followup."),
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

    crate::mcp::run(api_name, spec, profiles, include, exclude, max_tools, auth_tool)
        .await
        .map_err(Into::into)
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

/// Pre-resolve every profile referenced by `--spall-auth-tool` or by
/// an `x-mcp-auth-profile` extension into an `ApiEntry`. Profiles that
/// don't exist in the API's `[profiles.*]` block error out at startup
/// so the per-call dispatch path stays infallible.
fn resolve_auth_profiles(
    api_name: &str,
    registry: &ApiRegistry,
    spec: &spall_core::ir::ResolvedSpec,
    default_entry: &ApiEntry,
    auth_tool: &HashMap<String, String>,
) -> Result<crate::mcp::AuthProfiles> {
    let mut needed: HashSet<String> = HashSet::new();
    needed.extend(auth_tool.values().cloned());
    for op in &spec.operations {
        if let Some(SpallValue::Str(p)) = op.extensions.get("x-mcp-auth-profile") {
            needed.insert(p.clone());
        }
    }

    let mut by_profile: HashMap<String, ApiEntry> = HashMap::new();
    for profile in needed {
        if !default_entry.profiles.contains_key(&profile) {
            let configured: Vec<&String> = default_entry.profiles.keys().collect();
            return Err(SpallCliError::Usage(format!(
                "auth profile '{}' is not configured for api '{}'; configured profiles: {:?}",
                profile, api_name, configured,
            ))
            .into());
        }
        let entry = registry
            .resolve_profile(api_name, Some(&profile))
            .ok_or_else(|| {
                SpallCliError::Usage(format!(
                    "internal: api '{}' vanished while resolving profile '{}'",
                    api_name, profile
                ))
            })?;
        by_profile.insert(profile, entry);
    }

    Ok(crate::mcp::AuthProfiles {
        default: default_entry.clone(),
        by_profile,
    })
}
