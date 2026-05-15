//! `spall arazzo run|validate` subcommand handlers.

use clap::{Arg, ArgAction, ArgMatches, Command};
use miette::Result;
use spall_config::registry::ApiRegistry;
use std::path::Path;

use crate::arazzo_runner::{
    load_doc, outcome_to_json, parse_inputs, prepare_sources, run_workflow, validate_doc,
    ArazzoRunError, RunOptions, Severity,
};
use crate::SpallCliError;

/// Build the `arazzo` subcommand tree.
pub fn arazzo_cmd() -> Command {
    Command::new("arazzo")
        .about("Run or validate an Arazzo 1.0.1 workflow document")
        .subcommand(
            Command::new("run")
                .about("Execute a workflow against its source descriptions")
                .arg(
                    Arg::new("file")
                        .required(true)
                        .help("Path to the .arazzo.yaml document"),
                )
                .arg(
                    Arg::new("input")
                        .long("input")
                        .action(ArgAction::Append)
                        .help("Workflow input as key=value (repeatable; populates $inputs.<key>)"),
                )
                .arg(
                    Arg::new("workflow")
                        .long("workflow")
                        .help("Workflow ID to run when the document has multiple workflows"),
                )
                .arg(
                    Arg::new("dry-run")
                        .long("dry-run")
                        .action(ArgAction::SetTrue)
                        .help("Parse + plan the run; print each step's resolved request, send nothing"),
                )
                .arg(
                    Arg::new("output")
                        .long("output")
                        .value_parser(["json", "yaml"])
                        .default_value("json")
                        .help("Format for the final workflow outputs on stdout"),
                )
                .arg(
                    Arg::new("verbose")
                        .long("verbose")
                        .action(ArgAction::SetTrue)
                        .help("Emit a workflow-start banner showing source bindings"),
                )
                .arg(
                    Arg::new("max-steps")
                        .long("spall-max-steps")
                        .value_parser(clap::value_parser!(usize))
                        .help("Hard cap on per-workflow step executions (default 10000). Bounds infinite-goto loops in malformed workflows; counts retries and goto-revisits."),
                ),
        )
        .subcommand(
            Command::new("validate")
                .about("Parse a document and report any v1-unsupported constructs")
                .arg(
                    Arg::new("file")
                        .required(true)
                        .help("Path to the .arazzo.yaml document"),
                ),
        )
}

/// Dispatcher for the `arazzo` subcommand.
pub async fn handle_arazzo(
    matches: &ArgMatches,
    registry: &ApiRegistry,
    cache_dir: &Path,
) -> Result<()> {
    match matches.subcommand() {
        Some(("run", sub)) => run(sub, registry, cache_dir).await,
        Some(("validate", sub)) => validate(sub),
        _ => Err(SpallCliError::Usage(
            "Usage: spall arazzo <run|validate> <file>".to_string(),
        )
        .into()),
    }
}

fn validate(matches: &ArgMatches) -> Result<()> {
    let file = matches.get_one::<String>("file").expect("required by clap");
    let path = Path::new(file);
    let doc = load_doc(path).map_err(into_cli_err)?;
    let diags = validate_doc(&doc);

    let mut had_error = false;
    if diags.is_empty() {
        eprintln!(
            "ok: '{}' parses cleanly; all v1 features supported ({} workflow{}, {} source{}).",
            path.display(),
            doc.workflows.len(),
            if doc.workflows.len() == 1 { "" } else { "s" },
            doc.source_descriptions.len(),
            if doc.source_descriptions.len() == 1 { "" } else { "s" },
        );
        return Ok(());
    }
    for d in &diags {
        let prefix = match d.severity {
            Severity::Error => {
                had_error = true;
                "error"
            }
            Severity::Warning => "warning",
        };
        eprintln!("{}: {}", prefix, d.message);
    }
    if had_error {
        return Err(SpallCliError::ValidationFailed.into());
    }
    Ok(())
}

async fn run(matches: &ArgMatches, registry: &ApiRegistry, cache_dir: &Path) -> Result<()> {
    let file = matches.get_one::<String>("file").expect("required by clap");
    let path = Path::new(file);
    let verbose = matches.get_flag("verbose");
    let dry_run = matches.get_flag("dry-run");
    let workflow_id = matches.get_one::<String>("workflow").cloned();
    let raw_inputs: Vec<String> = matches
        .get_many::<String>("input")
        .map(|vals| vals.cloned().collect())
        .unwrap_or_default();
    let output_fmt = matches
        .get_one::<String>("output")
        .map(|s| s.as_str())
        .unwrap_or("json");

    let doc = load_doc(path).map_err(into_cli_err)?;

    let inputs = parse_inputs(&raw_inputs).map_err(into_cli_err)?;

    // For source loading we accept the system-default proxy and a
    // default-shaped HttpConfig. Per-source proxy overrides + global
    // --spall-* flags can be wired through in a follow-up.
    let proxy = crate::http::resolve_env_proxy();
    let sources =
        prepare_sources(&doc, path, registry, cache_dir, proxy.as_deref(), verbose)
            .await
            .map_err(into_cli_err)?;

    let max_steps = matches
        .get_one::<usize>("max-steps")
        .copied()
        .unwrap_or(crate::arazzo_runner::DEFAULT_MAX_STEPS);

    let opts = RunOptions {
        workflow_id,
        inputs,
        dry_run,
        verbose,
        max_steps,
    };

    let http_config = crate::http::HttpConfig::default();
    let outcome = run_workflow(&doc, &sources, opts, http_config)
        .await
        .map_err(into_cli_err)?;

    let value = outcome_to_json(&outcome);
    match output_fmt {
        "yaml" => {
            let s = spall_core::yaml::to_string(&value).map_err(SpallCliError::Cache)?;
            println!("{}", s);
        }
        _ => {
            let s = serde_json::to_string_pretty(&value)
                .map_err(|e| SpallCliError::Usage(e.to_string()))?;
            println!("{}", s);
        }
    }
    Ok(())
}

fn into_cli_err(e: ArazzoRunError) -> SpallCliError {
    match e {
        ArazzoRunError::Transport(msg) => SpallCliError::Network(msg),
        ArazzoRunError::StepHttpError { status, .. } if (400..500).contains(&status) => {
            SpallCliError::Http4xx(status)
        }
        ArazzoRunError::StepHttpError { status, .. } if (500..600).contains(&status) => {
            SpallCliError::Http5xx(status)
        }
        other => SpallCliError::Usage(other.to_string()),
    }
}
