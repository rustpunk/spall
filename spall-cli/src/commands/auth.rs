//! `spall auth` subcommands.

use clap::ArgMatches;
use miette::Result;

/// Handle `spall auth status|login`.
pub async fn handle_auth(matches: &ArgMatches) -> Result<()> {
    match matches.subcommand() {
        Some(("status", sub)) => handle_status(sub).await,
        Some(("login", sub)) => handle_login(sub).await,
        _ => Ok(()),
    }
}

async fn handle_status(matches: &ArgMatches) -> Result<()> {
    let api_name = matches
        .get_one::<String>("api")
        .ok_or_else(|| crate::SpallCliError::Usage("API name required".to_string()))?;

    let registry = spall_config::registry::ApiRegistry::load()
        .map_err(crate::SpallCliError::Config)?;
    let entry = registry
        .find(api_name)
        .ok_or_else(|| crate::SpallCliError::Usage(format!("Unknown API: {}", api_name)))?;

    let resolved = crate::auth::resolve(api_name, entry.auth.as_ref(), None);

    if let Some(auth) = resolved {
        eprintln!("Auth for '{}': kind = {}", api_name, auth.kind_label());
    } else {
        eprintln!("Auth for '{}': not configured", api_name);
    }

    Ok(())
}

async fn handle_login(matches: &ArgMatches) -> Result<()> {
    let api_name = matches
        .get_one::<String>("api")
        .ok_or_else(|| crate::SpallCliError::Usage("API name required".to_string()))?;

    let registry = spall_config::registry::ApiRegistry::load()
        .map_err(crate::SpallCliError::Config)?;
    let entry = registry
        .find(api_name)
        .ok_or_else(|| crate::SpallCliError::Usage(format!("Unknown API: {}", api_name)))?;

    let auth_config = match &entry.auth {
        Some(a) => a,
        None => {
            return Err(crate::SpallCliError::Usage(format!(
                "No auth configuration found for API '{}'",
                api_name
            )).into());
        }
    };

    match auth_config.kind.unwrap_or(spall_config::auth::AuthKind::Bearer) {
        spall_config::auth::AuthKind::OAuth2 => {
            eprintln!("OAuth2 login stub for '{}'", api_name);
            eprintln!("In Wave 3 Independent, obtain a token manually and pass it via --spall-auth or SPALL_{}_TOKEN.",
                api_name.to_uppercase().replace('-', "_"));
            // TODO(Wave 3+): implement full PKCE browser flow.
        }
        spall_config::auth::AuthKind::Basic => {
            eprintln!("Basic auth for '{}' does not require login.", api_name);
            eprintln!("Set the password via auth.password_env or interactive prompt.");
        }
        _ => {
            eprintln!("Login not required for '{}' auth kind.", api_name);
        }
    }

    Ok(())
}
