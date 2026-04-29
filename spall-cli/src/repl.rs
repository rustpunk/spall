//! Interactive REPL shell for spall.

use std::path::Path;
use rustyline::{DefaultEditor, error::ReadlineError};

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
