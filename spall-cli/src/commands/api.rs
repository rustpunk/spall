#![allow(dead_code, unused_variables, unused_imports)]

//! `spall api` management subcommands.

use clap::ArgMatches;
use miette::Result;

/// Handle `spall api add|list|remove|refresh`.
pub async fn handle_api_management(
    matches: &ArgMatches,
    cache_dir: &std::path::Path,
) -> Result<()> {
    match matches.subcommand() {
        Some(("add", sub)) => handle_add(sub),
        Some(("list", sub)) => handle_list(sub),
        Some(("remove", sub)) => handle_remove(sub),
        Some(("refresh", sub)) => handle_refresh(sub, cache_dir).await,
        _ => {
            // Should not happen if clap parsing is correct.
            Ok(())
        }
    }
}

fn handle_add(matches: &ArgMatches) -> Result<()> {
    let name = matches.get_one::<String>("name").unwrap();
    let source = matches.get_one::<String>("source").unwrap();
    spall_config::registry::ApiRegistry::add_api(name, source).map_err(|e| {
        crate::SpallCliError::Config(e)
    })?;
    eprintln!("Registered API '{}' from {}", name, source);
    Ok(())
}

fn handle_list(_matches: &ArgMatches) -> Result<()> {
    let registry = spall_config::registry::ApiRegistry::load().map_err(|e| {
        crate::SpallCliError::Config(e)
    })?;
    if registry.apis.is_empty() {
        eprintln!("Registered APIs:\n  (no APIs registered yet)");
    } else {
        eprintln!("Registered APIs:");
        for entry in &registry.apis {
            eprintln!("  {:<20} {}", entry.name, entry.source);
        }
    }
    Ok(())
}

fn handle_remove(matches: &ArgMatches) -> Result<()> {
    let name = matches.get_one::<String>("name").unwrap();
    spall_config::registry::ApiRegistry::remove_api(name).map_err(|e| {
        crate::SpallCliError::Config(e)
    })?;
    eprintln!("Removed API '{}'", name);
    Ok(())
}

async fn handle_refresh(
    matches: &ArgMatches,
    cache_dir: &std::path::Path,
) -> Result<()> {
    let all = matches.get_flag("all");
    let name = matches.get_one::<String>("name");

    let registry = spall_config::registry::ApiRegistry::load().map_err(|e| {
        crate::SpallCliError::Config(e)
    })?;

    if all {
        for entry in &registry.apis {
            if entry.source.starts_with("http://") || entry.source.starts_with("https://") {
                match crate::fetch::refresh(&entry.source, cache_dir).await {
                    Ok(_) => eprintln!("Refreshed API '{}'", entry.name),
                    Err(e) => eprintln!("Failed to refresh API '{}': {}", entry.name, e),
                }
            }
        }
    } else if let Some(n) = name {
        let entry = registry
            .find(n)
            .ok_or_else(|| crate::SpallCliError::Usage(format!("Unknown API: {}", n)))?;
        if entry.source.starts_with("http://") || entry.source.starts_with("https://") {
            crate::fetch::refresh(&entry.source, cache_dir)
                .await
                .map_err(|e| crate::SpallCliError::Network(e.to_string()))?;
            eprintln!("Refreshed API '{}'", n);
        } else {
            eprintln!("Warning: refresh only applies to remote specs. '{}' is a file source.", n);
        }
    } else {
        return Err(crate::SpallCliError::Usage(
            "Provide an API name or use --all".to_string(),
        )
        .into());
    }

    Ok(())
}
