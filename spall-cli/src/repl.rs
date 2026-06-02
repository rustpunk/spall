//! Interactive REPL shell for spall.

use rustyline::{error::ReadlineError, DefaultEditor};
use std::path::Path;

/// Per-stage REPL pipe failure with actionable diagnostics.
#[derive(Debug, thiserror::Error)]
enum PipeError {
    #[error("Pipe failed at stage {stage}: parse error\n  Expression: {expr}\n  Reason:     {reason}\n  To debug:   spall chain validate '{expr}'")]
    Parse {
        stage: usize,
        expr: String,
        reason: String,
    },
    #[error("Pipe failed at stage {stage}: jmespath error\n  Expression: {expr}\n  Reason:     {reason}\n  To debug:   echo '{{}}' | spall jmespath '{expr}'")]
    JmesPath {
        stage: usize,
        expr: String,
        reason: String,
    },
    #[error("Pipe failed at stage {stage}: no response from previous stage to feed into next stage\n  To debug:   ensure stage {} succeeded and returned JSON", stage - 1)]
    NoPreviousResponse { stage: usize },
    #[error("Pipe failed at stage {stage}: chain target '{target}' is not an operation of API '{api}'\n  Reason:     chaining stays within a single API; to chain across APIs run them as separate pipes\n  To debug:   spall {api} --help")]
    UnknownTarget {
        stage: usize,
        api: String,
        target: String,
    },
    #[error("Pipe failed at stage {stage}: could not load the spec for API '{api}'\n  Reason:     {reason}")]
    SpecLoad {
        stage: usize,
        api: String,
        reason: String,
    },
    #[error("Pipe failed at stage {stage}: request error\n  Reason:     {reason}")]
    Http { stage: usize, reason: String },
}

/// Run the spall interactive REPL.
pub async fn run(
    cache_dir: &Path,
    registry: &spall_config::registry::ApiRegistry,
) -> Result<(), crate::SpallCliError> {
    let mut rl = DefaultEditor::new()
        .map_err(|e| crate::SpallCliError::Usage(format!("REPL init failed: {}", e)))?;

    let history_path = cache_dir.join("repl_history");
    let _ = rl.load_history(&history_path);

    println!("spall REPL — type 'help' for commands, 'quit' or 'exit' to leave.");

    loop {
        match rl.readline("spall> ") {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Err(e) = rl.add_history_entry(trimmed) {
                    eprintln!("Warning: failed to add history entry: {}", e);
                }

                if trimmed.contains('|') {
                    let stages: Vec<&str> = trimmed.split('|').map(|s| s.trim()).collect();
                    if let Err(e) = run_piped(&stages, registry, cache_dir).await {
                        eprintln!("{}", e);
                    }
                    continue;
                }

                match trimmed {
                    "quit" | "exit" => break,
                    "help" => {
                        println!("Special commands:");
                        println!("  help      Show this help message");
                        println!("  history   List last 20 requests from history");
                        println!("  quit      Exit the REPL");
                        println!("  exit      Exit the REPL");
                        println!("Any other input is parsed as a spall command, e.g.:");
                        println!("  api list");
                        println!("  <api> <operation> [args...]");
                    }
                    "history" => {
                        if let Err(e) = show_history(cache_dir) {
                            eprintln!("History error: {}", e);
                        }
                    }
                    _ => {
                        let Some(args) = shlex::split(trimmed) else {
                            eprintln!("Error: unmatched quote in input");
                            continue;
                        };
                        if args.is_empty() {
                            continue;
                        }
                        if args[0] == "repl" {
                            println!("Already in REPL.");
                            continue;
                        }
                        // Prepend binary name so run_with_args sees correct argv[0].
                        let mut full_args = vec!["spall".to_string()];
                        full_args.extend(args);
                        // Single-command dispatch has no downstream consumer.
                        let dispatch = async move {
                            let mut sink = crate::execute::ResponseContext::new();
                            crate::run_with_args(&full_args, registry, cache_dir, &mut sink).await
                        };
                        if let Err(e) = Box::pin(dispatch).await {
                            let code = match e.downcast_ref::<crate::SpallCliError>() {
                                Some(err) => err.exit_code(),
                                None => crate::EXIT_USAGE,
                            };
                            eprintln!("Error (exit {}): {:?}", code, e);
                        }
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("Interrupted. Use 'quit' to exit.");
            }
            Err(ReadlineError::Eof) => {
                println!("EOF");
                break;
            }
            Err(e) => {
                eprintln!("Readline error: {}", e);
                break;
            }
        }
    }

    if let Err(e) = rl.save_history(&history_path) {
        eprintln!("Warning: failed to save REPL history: {}", e);
    }

    Ok(())
}

fn show_history(cache_dir: &Path) -> Result<(), crate::SpallCliError> {
    let history = crate::history::History::open(cache_dir)
        .map_err(|e| crate::SpallCliError::Usage(format!("History DB error: {}", e)))?;
    let rows = history
        .list(20)
        .map_err(|e| crate::SpallCliError::Usage(format!("History DB error: {}", e)))?;

    if rows.is_empty() {
        println!("No history recorded yet.");
        return Ok(());
    }

    for r in rows {
        let ts = chrono::DateTime::from_timestamp(r.timestamp as i64, 0)
            .unwrap_or(chrono::DateTime::UNIX_EPOCH)
            .format("%Y-%m-%d %H:%M:%S UTC");
        println!(
            "#{} {} | {} {} | {} {} | {}ms",
            r.id, ts, r.api, r.operation, r.method, r.url, r.duration_ms
        );
    }
    Ok(())
}

/// Load the resolved spec for a registered API, used to resolve and validate
/// chain targets in a pipe (the stage-0 API governs every later stage).
async fn load_api_spec(
    api_name: &str,
    registry: &spall_config::registry::ApiRegistry,
    cache_dir: &Path,
) -> Result<spall_core::ir::ResolvedSpec, String> {
    let entry = registry
        .resolve_profile(api_name, None)
        .ok_or_else(|| format!("unknown API '{}'", api_name))?;
    let proxy = crate::http::resolve_env_proxy();
    let raw = crate::fetch::load_raw(&entry.source, cache_dir, proxy.as_deref())
        .await
        .map_err(|e| e.to_string())?;
    spall_core::cache::load_or_resolve(&entry.source, &raw, cache_dir).map_err(|e| e.to_string())
}

async fn run_piped(
    stages: &[&str],
    registry: &spall_config::registry::ApiRegistry,
    cache_dir: &std::path::Path,
) -> Result<(), PipeError> {
    if stages.len() < 2 {
        return Err(PipeError::Parse {
            stage: 0,
            expr: stages.first().unwrap_or(&"").to_string(),
            reason: "pipe requires at least two stages".to_string(),
        });
    }

    // Stage 0: raw execution.
    let stage0_args = shlex::split(stages[0]).ok_or_else(|| PipeError::Parse {
        stage: 0,
        expr: stages[0].to_string(),
        reason: "unmatched quote in pipe stage 0".to_string(),
    })?;
    if stage0_args.is_empty() {
        return Err(PipeError::Parse {
            stage: 0,
            expr: stages[0].to_string(),
            reason: "empty pipe stage 0".to_string(),
        });
    }

    let api_name = &stage0_args[0];
    let mut full_args = vec!["spall".to_string()];
    full_args.extend(stage0_args.clone());

    // Explicit per-pipe response context (replaces the former global static).
    // Each stage writes into it only on a successful operation; a stage that
    // never reaches `execute_operation` (e.g. `history list`) leaves it empty,
    // so the next stage sees `NoPreviousResponse` instead of stale data.
    let mut sink = crate::execute::ResponseContext::new();

    match Box::pin(crate::run_with_args(
        &full_args,
        registry,
        cache_dir,
        &mut sink,
    ))
    .await
    {
        Ok(()) => {}
        Err(e) => {
            return Err(PipeError::Http {
                stage: 0,
                reason: e.to_string(),
            });
        }
    }

    // Stages 1+ are chain expressions against the previous response. They all
    // dispatch against the stage-0 API; its spec resolves and validates chain
    // targets (#34, #40). It is loaded lazily on first need — only once a stage
    // actually has a response to chain — so a non-operation stage-0 (e.g.
    // `history list`) still surfaces as `NoPreviousResponse` (#37) rather than a
    // spec-load failure for the non-API stage-0 token.
    let mut spec: Option<spall_core::ir::ResolvedSpec> = None;

    for (i, stage) in stages.iter().skip(1).enumerate() {
        let stage_num = i + 1;
        let response = sink
            .take()
            .ok_or(PipeError::NoPreviousResponse { stage: stage_num })?;

        let chain = crate::chain::ChainExpr::parse(stage).map_err(|e| PipeError::Parse {
            stage: stage_num,
            expr: stage.to_string(),
            reason: e.to_string(),
        })?;

        // Load (and cache) the stage-0 API spec the first time a real response
        // reaches a chain stage.
        if spec.is_none() {
            spec = Some(load_api_spec(api_name, registry, cache_dir).await.map_err(
                |reason| PipeError::SpecLoad {
                    stage: stage_num,
                    api: api_name.to_string(),
                    reason,
                },
            )?);
        }
        let spec_ref = spec
            .as_ref()
            .ok_or(PipeError::NoPreviousResponse { stage: stage_num })?;

        // #40: the chain target must be an operation of the stage-0 API.
        // Resolving it here yields an actionable error instead of dispatching
        // against the wrong API or a confusing "Unknown operation".
        let target_op = spec_ref
            .operations
            .iter()
            .find(|o| o.operation_id == chain.target_op_id)
            .ok_or_else(|| PipeError::UnknownTarget {
                stage: stage_num,
                api: api_name.to_string(),
                target: chain.target_op_id.clone(),
            })?;

        // #34: resolve bindings against the target op's parameter metadata.
        let next_args =
            chain
                .resolve(&response, target_op)
                .map_err(|e| PipeError::JmesPath {
                    stage: stage_num,
                    expr: stage.to_string(),
                    reason: e.to_string(),
                })?;

        let mut full_args = vec!["spall".to_string(), api_name.to_string()];
        full_args.extend(next_args);

        match Box::pin(crate::run_with_args(
            &full_args,
            registry,
            cache_dir,
            &mut sink,
        ))
        .await
        {
            Ok(()) => {}
            Err(e) => {
                return Err(PipeError::Http {
                    stage: stage_num,
                    reason: e.to_string(),
                });
            }
        }
    }

    Ok(())
}
