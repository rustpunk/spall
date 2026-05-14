//! `spall mcp <api>` subcommand: serve a registered API over Model
//! Context Protocol on stdio.

use clap::{Arg, ArgAction, ArgMatches, Command};
use miette::Result;
use spall_config::registry::ApiRegistry;
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

    let entry = registry
        .resolve_profile(&api_name, None)
        .ok_or_else(|| SpallCliError::Usage(format!("Unknown API: {}", api_name)))?;

    let proxy = crate::http::resolve_env_proxy();
    let raw = crate::fetch::load_raw(&entry.source, cache_dir, proxy.as_deref())
        .await
        .map_err(|e| SpallCliError::SpecLoadFailed {
            api: api_name.clone(),
            source: entry.source.clone(),
            cause: spall_core::error::SpallCoreError::InvalidSource(e.to_string()),
        })?;

    let spec = spall_core::cache::load_or_resolve(&entry.source, &raw, cache_dir).map_err(|e| {
        SpallCliError::SpecLoadFailed {
            api: api_name.clone(),
            source: entry.source.clone(),
            cause: e,
        }
    })?;

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

    crate::mcp::run(api_name, spec, entry, include, exclude, max_tools)
        .await
        .map_err(Into::into)
}
