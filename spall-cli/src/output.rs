//! Response formatting, TTY detection, and output modes.

use is_terminal::IsTerminal;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;
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
    /// CSV output (Wave 2).
    Csv,
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
            "csv" => Some(OutputMode::Csv),
            _ => None,
        }
    }
}

/// Write response body to stdout or a file.
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

    // If mode needs structured JSON, try parsing once and delegate.
    match mode {
        OutputMode::Yaml | OutputMode::Table | OutputMode::Csv => {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
                return emit_json_value(&value, mode, save_path);
            }
            // Fall back: if not valid JSON, warn and stream raw.
            eprintln!("Warning: expected JSON for {} output, got non-JSON body. Falling back to raw.",
                mode_name(mode));
            io::stdout().write_all(body)?;
            return Ok(());
        }
        _ => {}
    }

    match mode {
        OutputMode::Pretty => {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
                if let Ok(pretty) = serde_json::to_string_pretty(&value) {
                    let highlighted = highlight_json(&pretty);
                    io::stdout().write_all(highlighted.as_bytes())?;
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
            // Unreachable: handled above.
            io::stdout().write_all(body)?;
        }
        OutputMode::Table => {
            io::stdout().write_all(body)?;
        }
        OutputMode::Csv => {
            io::stdout().write_all(body)?;
        }
    }

    Ok(())
}

/// Emit an already-parsed JSON value directly.
///
/// Used by pagination to avoid serialising→re-parsing.
pub fn emit_json_value(
    value: &serde_json::Value,
    mode: OutputMode,
    save_path: Option<&str>,
) -> io::Result<()> {
    if let Some(path) = save_path {
        let bytes = serde_json::to_vec(value).unwrap_or_default();
        std::fs::write(path, bytes)?;
        eprintln!("Response saved to {}", path);
        return Ok(());
    }

    match mode {
        OutputMode::Pretty => {
            let pretty = serde_json::to_string_pretty(value).unwrap_or_default();
            let highlighted = highlight_json(&pretty);
            io::stdout().write_all(highlighted.as_bytes())?;
            io::stdout().write_all(b"\n")?;
        }
        OutputMode::Raw => {
            let raw = serde_json::to_vec(value).unwrap_or_default();
            io::stdout().write_all(&raw)?;
        }
        OutputMode::Yaml => {
            match spall_core::yaml::to_string(value) {
                Ok(yaml) => {
                    io::stdout().write_all(yaml.as_bytes())?;
                }
                Err(e) => {
                    eprintln!("Warning: YAML serialization failed ({}). Falling back to pretty JSON.", e);
                    let pretty = serde_json::to_string_pretty(value).unwrap_or_default();
                    io::stdout().write_all(pretty.as_bytes())?;
                    io::stdout().write_all(b"\n")?;
                }
            }
        }
        OutputMode::Table => {
            print_table(value)?;
        }
        OutputMode::Csv => {
            print_csv(value)?;
        }
    }

    Ok(())
}

/// Highlight a JSON string with syntect using the base16-ocean.dark theme.
/// Falls back to the plain string if highlighting fails.
fn highlight_json(json_str: &str) -> String {
    let ss = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let syntax = match ss.find_syntax_by_extension("json") {
        Some(s) => s,
        None => return json_str.to_string(),
    };
    let theme = match ts.themes.get("base16-ocean.dark") {
        Some(t) => t,
        None => return json_str.to_string(),
    };
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut output = String::new();
    for line in json_str.lines() {
        let regions = match highlighter.highlight_line(line, &ss) {
            Ok(r) => r,
            Err(_) => {
                output.push_str(line);
                output.push('\n');
                continue;
            }
        };
        output.push_str(&as_24_bit_terminal_escaped(&regions[..], false));
        output.push('\n');
    }
    // Remove trailing newline if input didn't end with one, to preserve exact formatting.
    if !json_str.ends_with('\n') {
        output.pop();
    }
    output
}

fn mode_name(mode: OutputMode) -> &'static str {
    match mode {
        OutputMode::Pretty => "pretty",
        OutputMode::Raw => "raw",
        OutputMode::Yaml => "yaml",
        OutputMode::Table => "table",
        OutputMode::Csv => "csv",
    }
}

fn print_table(value: &serde_json::Value) -> io::Result<()> {
    let rows = match value.as_array() {
        Some(arr) => arr,
        None => {
            eprintln!("Warning: table mode requires a JSON array. Falling back to pretty JSON.");
            let pretty = serde_json::to_string_pretty(value).unwrap_or_default();
            io::stdout().write_all(pretty.as_bytes())?;
            io::stdout().write_all(b"\n")?;
            return Ok(());
        }
    };

    if rows.is_empty() {
        println!("(empty result set)");
        return Ok(());
    }

    // Collect all unique top-level keys in order of appearance.
    let mut headers: Vec<String> = Vec::new();
    let mut seen_headers: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in rows {
        if let Some(obj) = row.as_object() {
            for key in obj.keys() {
                if seen_headers.insert(key.clone()) {
                    headers.push(key.clone());
                }
            }
        }
    }

    // Build table rows.
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    for row in rows {
        let mut table_row = Vec::with_capacity(headers.len());
        if let Some(obj) = row.as_object() {
            for key in &headers {
                let cell = match obj.get(key) {
                    Some(serde_json::Value::String(s)) => s.clone(),
                    Some(v) => v.to_string(),
                    None => String::new(),
                };
                table_row.push(cell);
            }
        } else {
            eprintln!("Warning: table mode requires an array of objects. Row is not an object; falling back to pretty JSON.");
            let pretty = serde_json::to_string_pretty(value).unwrap_or_default();
            io::stdout().write_all(pretty.as_bytes())?;
            io::stdout().write_all(b"\n")?;
            return Ok(());
        }
        table_rows.push(table_row);
    }

    let mut builder = tabled::builder::Builder::default();
    builder.push_record(&headers);
    for row in &table_rows {
        builder.push_record(row);
    }
    let mut table = builder.build();
    table.with(tabled::settings::Style::modern());
    println!("{}", table);
    Ok(())
}

fn print_csv(value: &serde_json::Value) -> io::Result<()> {
    let rows = match value.as_array() {
        Some(arr) => arr,
        None => {
            eprintln!("Warning: csv mode requires a JSON array. Falling back to raw.");
            let raw = serde_json::to_vec(value).unwrap_or_default();
            io::stdout().write_all(&raw)?;
            return Ok(());
        }
    };

    if rows.is_empty() {
        return Ok(());
    }

    let mut writer = csv::Writer::from_writer(io::stdout());

    // Collect headers.
    let mut headers: Vec<String> = Vec::new();
    let mut seen_headers: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in rows {
        if let Some(obj) = row.as_object() {
            for key in obj.keys() {
                if seen_headers.insert(key.clone()) {
                    headers.push(key.clone());
                }
            }
        }
    }

    writer.write_record(&headers)?;

    for row in rows {
        if let Some(obj) = row.as_object() {
            let record: Vec<String> = headers
                .iter()
                .map(|key| {
                    match obj.get(key) {
                        Some(serde_json::Value::String(s)) => s.clone(),
                        Some(v) => v.to_string(),
                        None => String::new(),
                    }
                })
                .collect();
            writer.write_record(&record)?;
        } else {
            eprintln!("Warning: csv mode requires an array of objects. Row is not an object; falling back to raw.");
            drop(writer);
            let raw = serde_json::to_vec(value).unwrap_or_default();
            io::stdout().write_all(&raw)?;
            return Ok(());
        }
    }

    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_roundtrip_from_json() -> Result<(), String> {
        let value = serde_json::json!({"name": "test", "count": 42});
        let yaml = spall_core::yaml::to_string(&value)?;
        assert!(yaml.contains("name: test"));
        assert!(yaml.contains("count: 42"));
        Ok(())
    }

    #[test]
    fn table_mode_non_array_fallback() -> io::Result<()> {
        let value = serde_json::json!("hello");
        // Should not panic; falls back internally.
        print_table(&value)?;
        Ok(())
    }

    #[test]
    fn csv_mode_non_array_fallback() -> io::Result<()> {
        let value = serde_json::json!("hello");
        print_csv(&value)?;
        Ok(())
    }
}
