#![allow(dead_code, unused_variables, unused_imports)]

//! Response formatting, TTY detection, and output modes.

use is_terminal::IsTerminal;
use std::io::{self, Write};

/// Output mode for formatting responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Pretty-printed JSON (default when TTY).
    Pretty,
    /// Raw JSON / bytes (default when piped).
    Raw,
    /// YAML output.
    Yaml,
    /// Table output (Wave 2).
    Table,
}

impl Default for OutputMode {
    fn default() -> Self {
        if io::stdout().is_terminal() {
            OutputMode::Pretty
        } else {
            OutputMode::Raw
        }
    }
}

impl OutputMode {
    /// Parse from a string override.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "json" | "pretty" => Some(OutputMode::Pretty),
            "raw" => Some(OutputMode::Raw),
            "yaml" => Some(OutputMode::Yaml),
            "table" => Some(OutputMode::Table),
            _ => None,
        }
    }
}

/// Write response body to stdout or a file.
///
/// Wave 1: basic stdout with mode selection.
/// Wave 1.5+: file output via `@path` or `--spall-download`.
pub fn emit_response(
    body: &[u8],
    mode: OutputMode,
    save_path: Option<&str>,
) -> io::Result<()> {
    if let Some(path) = save_path {
        std::fs::write(path, body)?;
        eprintln!("Response saved to {}", path);
        return Ok(());
    }

    match mode {
        OutputMode::Pretty => {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
                if let Ok(pretty) = serde_json::to_string_pretty(&value) {
                    io::stdout().write_all(pretty.as_bytes())?;
                    io::stdout().write_all(b"\n")?;
                } else {
                    io::stdout().write_all(body)?;
                }
            } else {
                io::stdout().write_all(body)?;
            }
        }
        OutputMode::Raw => {
            io::stdout().write_all(body)?;
        }
        OutputMode::Yaml => {
            // TODO(Wave 2): YAML serialization.
            io::stdout().write_all(body)?;
        }
        OutputMode::Table => {
            // TODO(Wave 2): table formatting.
            io::stdout().write_all(body)?;
        }
    }

    Ok(())
}
