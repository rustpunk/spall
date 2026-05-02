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
                        // Dispatch in a boxed future to avoid async recursion.
                        let dispatch = async move {
                            crate::run_with_args(&full_args, registry, cache_dir).await
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

    match Box::pin(crate::run_with_args(&full_args, registry, cache_dir)).await {
        Ok(()) => {}
        Err(e) => {
            return Err(PipeError::Http {
                stage: 0,
                reason: e.to_string(),
            });
        }
    }

    // Stages 1+ are chain expressions against the previous response.
    for (i, stage) in stages.iter().skip(1).enumerate() {
        let stage_num = i + 1;
        let response = crate::execute::take_last_response()
            .ok_or(PipeError::NoPreviousResponse { stage: stage_num })?;

        let chain = crate::chain::ChainExpr::parse(stage).map_err(|e| PipeError::Parse {
            stage: stage_num,
            expr: stage.to_string(),
            reason: e.to_string(),
        })?;
        let next_args = chain.resolve(&response).map_err(|e| PipeError::JmesPath {
            stage: stage_num,
            expr: stage.to_string(),
            reason: e.to_string(),
        })?;

        let mut full_args = vec!["spall".to_string(), api_name.to_string()];
        full_args.extend(next_args);

        match Box::pin(crate::run_with_args(&full_args, registry, cache_dir)).await {
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
