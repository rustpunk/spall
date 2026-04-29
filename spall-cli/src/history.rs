//! SQLite-backed request/response history.

use rusqlite::{params, Connection};
use std::path::Path;

/// An in-memory representation of a recorded request.
#[derive(Debug, Clone)]
pub struct RequestRecord {
    pub timestamp: u64,
    pub api: String,
    pub operation: String,
    pub method: String,
    pub url: String,
    pub status_code: u16,
    pub duration_ms: u64,
    pub request_headers: Vec<(String, String)>,
    pub response_headers: Vec<(String, String)>,
}

/// History database handle.
pub struct History {
    conn: Connection,
}

impl History {
    /// Open or create the history database in `cache_dir/history.db`.
    pub fn open(cache_dir: &Path) -> Result<Self, rusqlite::Error> {
        let db_path = cache_dir.join("history.db");
        let conn = Connection::open(&db_path)?;
        let version: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version == 0 {
            conn.execute_batch(SCHEMA_V1)?;
            conn.execute("PRAGMA user_version = 1", [])?;
        }
        Ok(Self { conn })
    }

    /// Record a request/response pair.
    pub fn record(&self, record: &RequestRecord) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO requests
             (timestamp, api, operation, method, url, status_code, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.timestamp as i64,
                record.api,
                record.operation,
                record.method,
                record.url,
                record.status_code as i32,
                record.duration_ms as i64,
            ],
        )?;
        let id = self.conn.last_insert_rowid();

        for (k, v) in &record.request_headers {
            self.conn.execute(
                "INSERT INTO request_headers (request_id, key, value) VALUES (?1, ?2, ?3)",
                params![id, k, v],
            )?;
        }

        for (k, v) in &record.response_headers {
            self.conn.execute(
                "INSERT INTO response_headers (request_id, key, value) VALUES (?1, ?2, ?3)",
                params![id, k, v],
            )?;
        }

        Ok(())
    }

    /// List the most recent requests (newest first).
    pub fn list(&self, limit: usize) -> Result<Vec<HistoryRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, timestamp, api, operation, method, url, status_code, duration_ms
             FROM requests
             ORDER BY timestamp DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(HistoryRow {
                id: row.get(0)?,
                timestamp: row.get::<_, i64>(1)? as u64,
                api: row.get(2)?,
                operation: row.get(3)?,
                method: row.get(4)?,
                url: row.get(5)?,
                status_code: row.get::<_, i32>(6)? as u16,
                duration_ms: row.get::<_, i64>(7)? as u64,
            })
        })?;
        rows.collect()
    }

    /// Get full details for a single request, including headers.
    pub fn get(&self, id: i64) -> Result<Option<FullRequest>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, timestamp, api, operation, method, url, status_code, duration_ms
             FROM requests WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(HistoryRow {
                id: row.get(0)?,
                timestamp: row.get::<_, i64>(1)? as u64,
                api: row.get(2)?,
                operation: row.get(3)?,
                method: row.get(4)?,
                url: row.get(5)?,
                status_code: row.get::<_, i32>(6)? as u16,
                duration_ms: row.get::<_, i64>(7)? as u64,
            })
        })?;

        let row = match rows.next().transpose()? {
            Some(r) => r,
            None => return Ok(None),
        };

        let req_headers: Vec<(String, String)> = self
            .conn
            .prepare("SELECT key, value FROM request_headers WHERE request_id = ?1")?
            .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;

        let resp_headers: Vec<(String, String)> = self
            .conn
            .prepare("SELECT key, value FROM response_headers WHERE request_id = ?1")?
            .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;

        Ok(Some(FullRequest {
            row,
            request_headers: req_headers,
            response_headers: resp_headers,
        }))
    }

    /// Clear all recorded history and vacuum the database.
    pub fn clear(&self) -> Result<(), rusqlite::Error> {
        self.conn.execute_batch(
            "DELETE FROM request_headers;
             DELETE FROM response_headers;
             DELETE FROM requests;
             VACUUM;",
        )?;
        Ok(())
    }
}

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS requests (
    id INTEGER PRIMARY KEY,
    timestamp INTEGER NOT NULL,
    api TEXT NOT NULL,
    operation TEXT NOT NULL,
    method TEXT NOT NULL,
    url TEXT NOT NULL,
    status_code INTEGER,
    duration_ms INTEGER
);

CREATE TABLE IF NOT EXISTS request_headers (
    request_id INTEGER NOT NULL,
    key TEXT NOT NULL,
    value TEXT
);

CREATE TABLE IF NOT EXISTS response_headers (
    request_id INTEGER NOT NULL,
    key TEXT NOT NULL,
    value TEXT
);

CREATE INDEX IF NOT EXISTS idx_requests_timestamp ON requests(timestamp);
"#;

/// A row from the `requests` table.
#[derive(Debug, Clone)]
pub struct HistoryRow {
    pub id: i64,
    pub timestamp: u64,
    pub api: String,
    pub operation: String,
    pub method: String,
    pub url: String,
    pub status_code: u16,
    pub duration_ms: u64,
}

/// Full request details including headers.
#[derive(Debug, Clone)]
pub struct FullRequest {
    pub row: HistoryRow,
    pub request_headers: Vec<(String, String)>,
    pub response_headers: Vec<(String, String)>,
}

/// Check whether a header name is sensitive and should be redacted in history.
pub fn is_sensitive_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("auth")
        || lower.contains("cookie")
        || lower.contains("token")
        || lower.contains("key")
}
