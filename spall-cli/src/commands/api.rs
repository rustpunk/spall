#![allow(dead_code, unused_variables, unused_imports)]

//! `spall api` management subcommands.

use clap::ArgMatches;
use miette::Result;

/// Handle `spall api add|list|remove|refresh`.
pub fn handle_api_management(matches: &ArgMatches) -> Result<()> {
    match matches.subcommand() {
        Some(("add", sub)) => handle_add(sub),
        Some(("list", sub)) => handle_list(sub),
        Some(("remove", sub)) => handle_remove(sub),
        Some(("refresh", sub)) => handle_refresh(sub),
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

fn handle_refresh(matches: &ArgMatches) -> Result<()> {
    let all = matches.get_flag("all");
    let name = matches.get_one::<String>("name");
    // TODO(Wave 1.5): re-fetch remote specs and invalidate caches.
    if all {
        eprintln!("Refreshing all APIs...");
    } else if let Some(n) = name {
        eprintln!("Refreshing API '{}'...", n);
    }
    Ok(())
}
