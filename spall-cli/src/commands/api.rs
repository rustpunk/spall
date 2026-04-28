#![allow(unused_imports)]

//! `spall api` management subcommands.

use clap::ArgMatches;
use miette::Result;

/// Handle `spall api add|list|remove|refresh|discover`.
pub async fn handle_api_management(
    matches: &ArgMatches,
    cache_dir: &std::path::Path,
) -> Result<()> {
    match matches.subcommand() {
        Some(("add", sub)) => handle_add(sub),
        Some(("list", sub)) => handle_list(sub),
        Some(("remove", sub)) => handle_remove(sub),
        Some(("refresh", sub)) => handle_refresh(sub, cache_dir).await,
        Some(("discover", sub)) => handle_discover(sub).await,
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
                let proxy = crate::http::resolve_proxy(
                    entry,
                    &registry.defaults,
                    &clap::ArgMatches::default(),
                    &clap::ArgMatches::default(),
                );
                match crate::fetch::refresh(&entry.source, cache_dir, proxy.as_deref()).await {
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
            let proxy = crate::http::resolve_proxy(
                entry,
                &registry.defaults,
                &clap::ArgMatches::default(),
                &clap::ArgMatches::default(),
            );
            crate::fetch::refresh(&entry.source, cache_dir, proxy.as_deref())
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

async fn handle_discover(matches: &ArgMatches) -> Result<()> {
    let url = matches
        .get_one::<String>("url")
        .ok_or_else(|| crate::SpallCliError::Usage("URL required".to_string()))?;

    eprintln!("Probing {} for OpenAPI spec...", url);

    let discovered = crate::discover::probe(url).await?;

    // Check for name collision.
    let registry = spall_config::registry::ApiRegistry::load()
        .map_err(crate::SpallCliError::Config)?;
    if registry.find(&discovered.name).is_some() {
        return Err(crate::SpallCliError::Usage(format!(
            "API name '{}' already exists. Remove it first or register manually with `spall api add`.",
            discovered.name
        )).into());
    }

    spall_config::registry::ApiRegistry::add_api(
        &discovered.name,
        &discovered.spec_url,
    )
    .map_err(crate::SpallCliError::Config)?;

    eprintln!(
        "Discovered and registered '{}' ({}) from {}",
        discovered.name, discovered.title, discovered.spec_url
    );
    Ok(())
}
