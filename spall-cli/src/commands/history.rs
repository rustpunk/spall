//! `spall history` subcommands.

use clap::ArgMatches;
use miette::Result;

/// Handle `spall history list|show|clear`.
pub fn handle_history(matches: &ArgMatches, cache_dir: &std::path::Path) -> Result<()> {
    match matches.subcommand() {
        Some(("list" | "", _)) => handle_list(cache_dir),
        Some(("show", sub)) => handle_show(sub, cache_dir),
        Some(("clear", _)) => handle_clear(cache_dir),
        _ => handle_list(cache_dir),
    }
}

fn handle_list(cache_dir: &std::path::Path) -> Result<()> {
    let history = crate::history::History::open(cache_dir)
        .map_err(|e| crate::SpallCliError::Usage(format!("History DB error: {}", e)))?;
    let rows = history
        .list(20)
        .map_err(|e| crate::SpallCliError::Usage(format!("History DB error: {}", e)))?;

    if rows.is_empty() {
        println!("No history recorded yet.");
        return Ok(());
    }

    let records: Vec<HistoryRecord> = rows
        .into_iter()
        .map(|r| HistoryRecord {
            id: r.id,
            timestamp: format_timestamp(r.timestamp),
            api: r.api,
            operation: r.operation,
            method: r.method,
            url: r.url,
            status: r.status_code.to_string(),
            duration: format!("{}ms", r.duration_ms),
        })
        .collect();

    let mut table = tabled::Table::new(&records);
    table.with(tabled::settings::Style::modern());
    println!("{}", table);
    Ok(())
}

fn handle_show(matches: &ArgMatches, cache_dir: &std::path::Path) -> Result<()> {
    let id: i64 = matches
        .get_one::<String>("id")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| crate::SpallCliError::Usage("Invalid history ID".to_string()))?;

    let history = crate::history::History::open(cache_dir)
        .map_err(|e| crate::SpallCliError::Usage(format!("History DB error: {}", e)))?;
    let full = history
        .get(id)
        .map_err(|e| crate::SpallCliError::Usage(format!("History DB error: {}", e)))?;

    match full {
        Some(r) => {
            println!("Request #{}", r.row.id);
            println!("  Timestamp: {}", format_timestamp(r.row.timestamp));
            println!("  API:       {}", r.row.api);
            println!("  Operation: {}", r.row.operation);
            println!("  Method:    {}", r.row.method);
            println!("  URL:       {}", r.row.url);
            println!("  Status:    {}", r.row.status_code);
            println!("  Duration:  {}ms", r.row.duration_ms);
            println!("  Request Headers:");
            for (k, v) in &r.request_headers {
                if crate::history::is_sensitive_header(k) {
                    println!("    {}: [REDACTED]", k);
                } else {
                    println!("    {}: {}", k, v);
                }
            }
            println!("  Response Headers:");
            for (k, v) in &r.response_headers {
                if crate::history::is_sensitive_header(k) {
                    println!("    {}: [REDACTED]", k);
                } else {
                    println!("    {}: {}", k, v);
                }
            }
        }
        None => {
            eprintln!("No history entry with ID {}", id);
        }
    }
    Ok(())
}

fn handle_clear(cache_dir: &std::path::Path) -> Result<()> {
    let history = crate::history::History::open(cache_dir)
        .map_err(|e| crate::SpallCliError::Usage(format!("History DB error: {}", e)))?;
    history
        .clear()
        .map_err(|e| crate::SpallCliError::Usage(format!("History DB error: {}", e)))?;
    println!("History cleared.");
    Ok(())
}

fn format_timestamp(ts: u64) -> String {
    let dt = chrono::DateTime::from_timestamp(ts as i64, 0).unwrap_or(chrono::DateTime::UNIX_EPOCH);
    dt.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

/// Helper struct for table display.
#[derive(tabled::Tabled)]
struct HistoryRecord {
    #[tabled(rename = "ID")]
    id: i64,
    #[tabled(rename = "Timestamp")]
    timestamp: String,
    #[tabled(rename = "API")]
    api: String,
    #[tabled(rename = "Operation")]
    operation: String,
    #[tabled(rename = "Method")]
    method: String,
    #[tabled(rename = "URL")]
    url: String,
    #[tabled(rename = "Status")]
    status: String,
    #[tabled(rename = "Duration")]
    duration: String,
}
