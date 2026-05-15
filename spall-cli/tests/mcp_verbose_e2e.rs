//! Issue #17 e2e: `--spall-verbose` for `spall mcp` (stdio + http
//! transports). Spawns the binary, captures stderr, asserts the
//! `[spall-mcp]` sentinel line shapes and the no-leak guarantee.
//!
//! The string literals (`"[spall-mcp]"`, `"Bearer [REDACTED]"`) are
//! the wire contract of the verbose log — keeping them as inline
//! literals (rather than imports of `pub(crate) const`s) is
//! intentional: the test IS the contract.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{MockServer, ResponseTemplate};

fn bin_path() -> String {
    std::env::var("CARGO_BIN_EXE_spall").unwrap_or_else(|_| String::from("target/debug/spall"))
}

/// Pet-store spec with a single GET /pets/{petId} → matches the
/// existing `mcp_e2e.rs` shape so test setup is familiar.
fn pet_spec(port: u16) -> String {
    format!(
        r#"{{
  "openapi": "3.0.0",
  "info": {{ "title": "PetStore", "version": "1.0.0" }},
  "servers": [{{ "url": "http://localhost:{port}" }}],
  "paths": {{
    "/pets/{{petId}}": {{
      "get": {{
        "operationId": "getPetById",
        "tags": ["pets"],
        "parameters": [{{
          "name": "petId",
          "in": "path",
          "required": true,
          "schema": {{ "type": "integer" }}
        }}],
        "responses": {{ "200": {{ "description": "OK" }} }}
      }}
    }}
  }}
}}"#
    )
}

fn setup_api(temp: &TempDir, api_name: &str, spec_path: &str) {
    let apis_dir = temp.path().join("spall").join("apis");
    std::fs::create_dir_all(&apis_dir).unwrap();
    let toml = format!("source = \"{}\"\n", spec_path);
    std::fs::write(apis_dir.join(format!("{}.toml", api_name)), toml).unwrap();
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Server {
    fn send(&mut self, msg: &Value) {
        let mut buf = serde_json::to_vec(msg).expect("serialize");
        buf.push(b'\n');
        self.stdin.write_all(&buf).expect("write");
        self.stdin.flush().expect("flush");
    }

    fn recv(&mut self) -> Value {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).expect("read");
        assert!(n > 0, "EOF on stdout before reply");
        assert!(line.starts_with('{'), "stdout must be JSON: {:?}", line);
        serde_json::from_str(&line).expect("parse")
    }
}

fn spawn(temp: &TempDir, api: &str, extra_args: &[&str]) -> Server {
    let cache_dir = temp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let mut cmd = Command::new(bin_path());
    cmd.env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", &cache_dir)
        .arg("mcp")
        .arg(api)
        .args(extra_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn spall mcp");
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    Server { child, stdin, stdout }
}

fn shutdown(s: Server) -> String {
    drop(s.stdin);
    drop(s.stdout);
    let out = s.child.wait_with_output().expect("wait");
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[tokio::test]
async fn verbose_flag_off_emits_no_kind_sentinel_lines() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let mut server = spawn(&temp, "petstore", &[]);
    server.send(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
    }));
    let _ = server.recv();
    let stderr = shutdown(server);

    // Without --spall-verbose, no `[spall-mcp] kind=` lines should be
    // present. (The pre-existing `[spall-mcp] listening on` line is HTTP-
    // only and absent here; we still grep specifically for `kind=` to
    // distinguish the new verbose lines from the existing transport
    // sentinel.)
    assert!(
        !stderr.contains("[spall-mcp] kind="),
        "verbose-off must not emit kind= lines; stderr:\n{}",
        stderr,
    );
}

#[tokio::test]
async fn verbose_flag_on_emits_startup_line_with_api_and_transport() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let mut server = spawn(&temp, "petstore", &["--spall-verbose"]);
    server.send(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
    }));
    let _ = server.recv();
    let stderr = shutdown(server);

    assert!(
        stderr.contains("[spall-mcp] kind=startup api=petstore transport=stdio"),
        "missing startup sentinel; stderr:\n{}",
        stderr,
    );
    // No profiles configured → `<none>` placeholder.
    assert!(
        stderr.contains("profiles=<none>"),
        "expected profiles=<none>; stderr:\n{}",
        stderr,
    );
}

#[tokio::test]
async fn verbose_emits_tools_call_line_with_default_profile() {
    let mock = MockServer::start().await;
    wiremock::Mock::given(method("GET"))
        .and(path("/pets/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 1})))
        .mount(&mock)
        .await;

    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let mut server = spawn(&temp, "petstore", &["--spall-verbose"]);
    server.send(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
    }));
    let _ = server.recv();
    server.send(&json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "getpetbyid", "arguments": { "petId": 1 } }
    }));
    let resp = server.recv();
    assert_eq!(resp["result"]["isError"], false, "{:?}", resp);
    let stderr = shutdown(server);

    // Per-call line: profile=<default>, method=GET, url uses the
    // operation's path_template (literal `{petId}`, not the rendered
    // `1` — see verbose.rs "What is NOT redacted in v1").
    assert!(
        stderr.contains("[spall-mcp] kind=tools/call tool=getpetbyid profile=<default> method=GET url=/pets/{petId}"),
        "tools/call line missing or wrong shape; stderr:\n{}",
        stderr,
    );
}

#[tokio::test]
async fn verbose_redacts_bearer_token_and_does_not_leak_plaintext() {
    let mock = MockServer::start().await;
    wiremock::Mock::given(method("GET"))
        .and(path("/admin"))
        .and(header("Authorization", "Bearer supersecret-do-not-leak"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": "admin"})))
        .expect(1)
        .mount(&mock)
        .await;

    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("authed.json");
    let spec = format!(
        r#"{{
  "openapi": "3.0.0",
  "info": {{ "title": "Authed", "version": "1.0.0" }},
  "servers": [{{ "url": "http://localhost:{port}" }}],
  "paths": {{
    "/admin": {{ "get": {{ "operationId": "admin-op", "responses": {{ "200": {{ "description": "OK" }} }} }} }}
  }}
}}"#,
        port = mock.address().port()
    );
    std::fs::write(&spec_path, spec).unwrap();

    let apis_dir = temp.path().join("spall").join("apis");
    std::fs::create_dir_all(&apis_dir).unwrap();
    let api_toml = format!(
        r#"source = "{}"

[profile.admin]
[profile.admin.auth]
kind = "bearer"
token = "supersecret-do-not-leak"
"#,
        spec_path.to_str().unwrap()
    );
    std::fs::write(apis_dir.join("authed.toml"), api_toml).unwrap();

    let mut server = spawn(
        &temp,
        "authed",
        &["--spall-auth-tool", "admin-op=admin", "--spall-verbose"],
    );
    server.send(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
    }));
    let _ = server.recv();
    server.send(&json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "admin-op", "arguments": {} }
    }));
    let resp = server.recv();
    assert_eq!(resp["result"]["isError"], false, "{:?}", resp);
    let stderr = shutdown(server);

    // The bearer token must reach the wire (wiremock's `.expect(1)`
    // already enforces that). Stderr must NEVER contain it.
    assert!(
        !stderr.contains("supersecret-do-not-leak"),
        "PLAINTEXT TOKEN LEAKED into stderr; full stderr:\n{}",
        stderr,
    );
    // Startup line should mention the profile name (no credential).
    assert!(
        stderr.contains("profiles=admin"),
        "startup must list profile name; stderr:\n{}",
        stderr,
    );
    // tools/call line should attribute the dispatch to the admin profile.
    assert!(
        stderr.contains("kind=tools/call tool=admin-op profile=admin"),
        "tools/call attribution missing; stderr:\n{}",
        stderr,
    );
}

/// Spawn `spall mcp <api> --spall-transport http --spall-port 0` and
/// parse the OS-assigned URL from the existing `[spall-mcp] listening
/// on` sentinel.
fn spawn_http(temp: &TempDir, api: &str, extra_args: &[&str]) -> (Child, String) {
    let cache_dir = temp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let mut cmd = Command::new(bin_path());
    cmd.env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", &cache_dir)
        .arg("mcp")
        .arg(api)
        .args(["--spall-transport", "http", "--spall-port", "0"])
        .args(extra_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn spall mcp http");
    let stderr = child.stderr.take().unwrap();
    let mut reader = BufReader::new(stderr);
    let mut url: Option<String> = None;
    for _ in 0..50 {
        let mut line = String::new();
        let n = reader.read_line(&mut line).expect("read stderr");
        if n == 0 {
            break;
        }
        if let Some(rest) = line.strip_prefix("[spall-mcp] listening on ") {
            url = Some(rest.trim().trim_end_matches('/').to_string());
            break;
        }
    }
    let url = url.expect("HTTP transport must print listening sentinel");
    child.stderr = Some(reader.into_inner());
    (child, url)
}

#[tokio::test]
async fn verbose_http_transport_logs_origin_and_redacts_authorization() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let (mut child, url) = spawn_http(&temp, "petstore", &["--spall-verbose"]);

    // Initialize first to grab a session-id.
    let client = reqwest::Client::new();
    let init = client
        .post(&url)
        .header("content-type", "application/json")
        .body(
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
            })
            .to_string(),
        )
        .send()
        .await
        .expect("send init");
    let session_id = init
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("init must return session id")
        .to_string();
    let _ = init.text().await;

    // Now POST a tools/list with a bogus Authorization header to
    // exercise the redactor on inbound headers.
    let _resp = client
        .post(&url)
        .header("content-type", "application/json")
        .header("mcp-session-id", &session_id)
        .header("authorization", "Bearer leak-this-if-broken")
        .body(
            json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}
            })
            .to_string(),
        )
        .send()
        .await
        .expect("send tools/list");
    let _ = _resp.text().await;

    // Drain stderr from the child, then kill it.
    child.kill().expect("kill child");
    let mut leftover = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        // Read until pipe closes; bounded by child exit.
        let _ = stderr.read_to_string(&mut leftover);
    }
    let _ = child.wait_with_output();
    // Brief tolerance for stderr buffering on busy test hosts.
    std::thread::sleep(Duration::from_millis(50));

    assert!(
        leftover.contains("[spall-mcp] kind=http-request"),
        "http-request sentinel missing; stderr:\n{}",
        leftover,
    );
    assert!(
        leftover.contains("Bearer [REDACTED]"),
        "Authorization header must redact to Bearer [REDACTED]; stderr:\n{}",
        leftover,
    );
    assert!(
        !leftover.contains("leak-this-if-broken"),
        "PLAINTEXT BEARER LEAKED into stderr; full stderr:\n{}",
        leftover,
    );
}
