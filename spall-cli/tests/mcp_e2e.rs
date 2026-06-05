//! End-to-end tests for `spall mcp <api>` over stdio.
//!
//! Spawn the binary as a subprocess with stdin/stdout piped; speak
//! line-delimited JSON-RPC 2.0; assert the wire shape and that the
//! backend wiremock server received the dispatched HTTP call.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use tempfile::TempDir;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{MockServer, ResponseTemplate};

fn bin_path() -> String {
    std::env::var("CARGO_BIN_EXE_spall").unwrap_or_else(|_| String::from("target/debug/spall"))
}

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
    }},
    "/health": {{
      "get": {{
        "operationId": "healthCheck",
        "tags": ["ops"],
        "responses": {{ "200": {{ "description": "OK" }} }}
      }}
    }}
  }}
}}"#
    )
}

fn setup_api(temp: &TempDir, api_name: &str, spec_path: &str) {
    let config_dir = temp.path().join("spall");
    let apis_dir = config_dir.join("apis");
    std::fs::create_dir_all(&apis_dir).unwrap();
    let toml = format!("source = \"{}\"\n", spec_path);
    std::fs::write(apis_dir.join(format!("{}.toml", api_name)), toml).unwrap();
}

/// Spawned server + framed I/O helpers.
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
        let n = self
            .stdout
            .read_line(&mut line)
            .expect("read JSON-RPC line");
        assert!(n > 0, "EOF on stdout before reply");
        assert!(
            line.starts_with('{'),
            "stdout discipline: every line must be a JSON object, got: {:?}",
            line
        );
        serde_json::from_str(&line).expect("parse JSON-RPC line")
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
    Server {
        child,
        stdin,
        stdout,
    }
}

fn shutdown(s: Server) -> String {
    drop(s.stdin);
    drop(s.stdout);
    let out = s.child.wait_with_output().expect("wait");
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[tokio::test]
async fn initialize_then_tools_list_then_tools_call() {
    let mock = MockServer::start().await;
    wiremock::Mock::given(method("GET"))
        .and(path("/pets/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 42, "name": "Fluffy"})))
        .expect(1)
        .mount(&mock)
        .await;

    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let mut server = spawn(&temp, "petstore", &[]);

    // initialize
    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0" }
        }
    }));
    let resp = server.recv();
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(resp["result"]["serverInfo"]["name"], "spall");

    // notifications/initialized — no reply expected
    server.send(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    }));

    // tools/list
    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    }));
    let resp = server.recv();
    assert_eq!(resp["id"], 2);
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 2, "expected 2 tools, got {:?}", tools);
    let pet_tool = tools
        .iter()
        .find(|t| t["name"] == "getpetbyid")
        .expect("getpetbyid tool present");
    assert_eq!(
        pet_tool["inputSchema"]["properties"]["petId"]["type"],
        "integer"
    );
    let required: Vec<&str> = pet_tool["inputSchema"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(required.contains(&"petId"));

    // tools/call
    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "getpetbyid",
            "arguments": { "petId": 42 }
        }
    }));
    let resp = server.recv();
    assert_eq!(resp["id"], 3);
    assert_eq!(resp["result"]["isError"], false);
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    let body: Value = serde_json::from_str(text).expect("parse echoed body");
    assert_eq!(body["id"], 42);
    assert_eq!(body["name"], "Fluffy");

    let stderr = shutdown(server);
    // Server should announce itself on stderr (banner-only, never stdout).
    assert!(
        stderr.contains("serving 'petstore'"),
        "expected stderr banner, got: {}",
        stderr
    );
}

#[tokio::test]
async fn include_filter_limits_tools_by_tag() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let mut server = spawn(&temp, "petstore", &["--spall-include", "ops"]);

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
    }));
    let _ = server.recv();

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    }));
    let resp = server.recv();
    let tools = resp["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1, "include filter must keep only 'ops' tag");
    assert_eq!(tools[0]["name"], "healthcheck");

    let _ = shutdown(server);
}

#[tokio::test]
async fn http_404_surfaces_as_tool_is_error() {
    let mock = MockServer::start().await;
    wiremock::Mock::given(method("GET"))
        .and(path("/pets/999"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "not found"})))
        .mount(&mock)
        .await;

    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let mut server = spawn(&temp, "petstore", &[]);

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
    }));
    let _ = server.recv();

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "getpetbyid", "arguments": { "petId": 999 } }
    }));
    let resp = server.recv();
    assert_eq!(resp["result"]["isError"], true);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    let body: Value = serde_json::from_str(text).expect("parse body");
    assert_eq!(body["error"], "not found");

    let _ = shutdown(server);
}

#[tokio::test]
async fn array_query_param_explodes_into_repeated_pairs() {
    // Regression guard for issue #10 (audit smell #1): when an MCP tool
    // is called with an array argument that maps to a query parameter,
    // the dispatcher must honor OpenAPI's form+explode default and send
    // `?ids=1&ids=2&ids=3`, not `?ids=%5B1%2C2%2C3%5D`.
    let mock = MockServer::start().await;
    wiremock::Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("ids", "1"))
        // wiremock's query_param matches any occurrence; combined with
        // expect(1) below we assert the request fires exactly once
        // with all the right values via expect_count + a second
        // matcher block.
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"hits": 3})))
        .expect(1)
        .mount(&mock)
        .await;

    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("search.json");
    let spec = format!(
        r#"{{
  "openapi": "3.0.0",
  "info": {{ "title": "Search", "version": "1.0.0" }},
  "servers": [{{ "url": "http://localhost:{port}" }}],
  "paths": {{
    "/search": {{
      "get": {{
        "operationId": "search",
        "parameters": [{{
          "name": "ids",
          "in": "query",
          "schema": {{ "type": "array", "items": {{ "type": "integer" }} }},
          "style": "form",
          "explode": true
        }}],
        "responses": {{ "200": {{ "description": "OK" }} }}
      }}
    }}
  }}
}}"#,
        port = mock.address().port()
    );
    std::fs::write(&spec_path, spec).unwrap();
    setup_api(&temp, "search", spec_path.to_str().unwrap());

    let mut server = spawn(&temp, "search", &[]);
    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
    }));
    let _ = server.recv();

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "search", "arguments": { "ids": [1, 2, 3] } }
    }));
    let resp = server.recv();
    assert_eq!(resp["result"]["isError"], false);

    // Inspect what wiremock actually saw — the strongest assertion:
    // all three values arrived as repeated `ids=N` pairs, not as a
    // single literal-JSON value.
    let received = mock.received_requests().await.expect("requests");
    assert_eq!(received.len(), 1);
    let url = &received[0].url;
    let raw_query = url.query().unwrap_or("");
    assert!(
        raw_query.contains("ids=1") && raw_query.contains("ids=2") && raw_query.contains("ids=3"),
        "expected repeated ids pairs, got: {}",
        raw_query
    );
    assert!(
        !raw_query.contains("%5B"),
        "raw JSON literal leaked into query string: {}",
        raw_query
    );

    let _ = shutdown(server);
}

/// Build a synthetic spec with `ops_per_tag` operations on each of the
/// supplied tags. Operation IDs are deterministic: `{tag}-op-{N}` so
/// tests can assert which subset survived truncation.
fn many_ops_spec(port: u16, tags: &[&str], ops_per_tag: usize) -> String {
    let mut paths = Vec::new();
    for tag in tags {
        for i in 0..ops_per_tag {
            paths.push(format!(
                r#""/{tag}/op-{i}": {{
                    "get": {{
                        "operationId": "{tag}-op-{i}",
                        "tags": ["{tag}"],
                        "responses": {{ "200": {{ "description": "OK" }} }}
                    }}
                }}"#,
                tag = tag,
                i = i,
            ));
        }
    }
    format!(
        r#"{{
  "openapi": "3.0.0",
  "info": {{ "title": "Big", "version": "1.0.0" }},
  "servers": [{{ "url": "http://localhost:{port}" }}],
  "paths": {{ {paths} }}
}}"#,
        port = port,
        paths = paths.join(","),
    )
}

#[tokio::test]
async fn warning_fires_when_filtered_tool_count_exceeds_hint() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("big.json");
    // 3 tags × 50 ops = 150 tools — comfortably above the 100 hint.
    std::fs::write(
        &spec_path,
        many_ops_spec(mock.address().port(), &["users", "orgs", "billing"], 50),
    )
    .unwrap();
    setup_api(&temp, "big", spec_path.to_str().unwrap());

    let mut server = spawn(&temp, "big", &[]);
    // Drive one round-trip so the server has fully initialized before
    // we tear it down and inspect stderr.
    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
    }));
    let _ = server.recv();

    let stderr = shutdown(server);
    assert!(
        stderr.contains("WARNING 150 tools exceeds the ~100-tool cap"),
        "expected size warning, stderr was:\n{}",
        stderr,
    );
    assert!(
        stderr.contains("top tags by population"),
        "expected histogram line, stderr was:\n{}",
        stderr,
    );
    // All three tags should appear in the histogram with count=50.
    for tag in ["users", "orgs", "billing"] {
        assert!(
            stderr.contains(&format!("{}=50", tag)),
            "expected {}=50 in histogram, stderr was:\n{}",
            tag,
            stderr,
        );
    }
}

#[tokio::test]
async fn no_warning_when_filtered_tool_count_below_hint() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("small.json");
    // 2 tags × 20 ops = 40 tools, below the 100 hint.
    std::fs::write(
        &spec_path,
        many_ops_spec(mock.address().port(), &["users", "orgs"], 20),
    )
    .unwrap();
    setup_api(&temp, "small", spec_path.to_str().unwrap());

    let mut server = spawn(&temp, "small", &[]);
    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
    }));
    let _ = server.recv();
    let stderr = shutdown(server);

    assert!(
        !stderr.contains("WARNING"),
        "no warning expected at 40 tools, stderr was:\n{}",
        stderr,
    );
}

#[tokio::test]
async fn max_tools_truncates_deterministically_across_invocations() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("big.json");
    std::fs::write(
        &spec_path,
        many_ops_spec(mock.address().port(), &["alpha", "beta", "gamma"], 50),
    )
    .unwrap();
    setup_api(&temp, "big", spec_path.to_str().unwrap());

    let names_for_run = || -> Vec<String> {
        let mut server = spawn(&temp, "big", &["--spall-max-tools", "30"]);
        server.send(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
        }));
        let _ = server.recv();
        server.send(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }));
        let resp = server.recv();
        let tools = resp["result"]["tools"].as_array().unwrap().clone();
        let _ = shutdown(server);
        tools
            .into_iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    };

    let first = names_for_run();
    let second = names_for_run();
    assert_eq!(first.len(), 30, "max-tools cap must truncate to 30");
    assert_eq!(
        first, second,
        "truncation must be deterministic across runs"
    );
    // The sort key buckets by first tag alphabetically; with 50 ops in
    // each of alpha/beta/gamma, the 30-entry slice is fully inside the
    // alpha bucket.
    for name in &first {
        assert!(
            name.starts_with("alpha-"),
            "expected alpha bucket to fill first, got {}",
            name
        );
    }
}

#[tokio::test]
async fn list_tags_prints_tsv_then_exits_without_starting_server() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("big.json");
    std::fs::write(
        &spec_path,
        many_ops_spec(mock.address().port(), &["users", "orgs"], 3),
    )
    .unwrap();
    setup_api(&temp, "big", spec_path.to_str().unwrap());

    let cache_dir = temp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", &cache_dir)
        .arg("mcp")
        .arg("big")
        .arg("--spall-list-tags")
        .output()
        .expect("run spall mcp --spall-list-tags");

    assert!(
        output.status.success(),
        "expected exit 0, got {:?}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    assert_eq!(
        lines.next(),
        Some("tag\tcount\tsample-op-id"),
        "expected TSV header, got: {:?}",
        stdout,
    );
    let body: Vec<&str> = lines.collect();
    assert_eq!(body.len(), 2, "expected 2 tag rows, got: {:?}", body);
    // BTreeMap iteration order: alphabetical → orgs then users.
    assert!(
        body[0].starts_with("orgs\t3\torgs-op-"),
        "row 0: {}",
        body[0]
    );
    assert!(
        body[1].starts_with("users\t3\tusers-op-"),
        "row 1: {}",
        body[1]
    );
}

/// Spec with one operation per HTTP method, no body, returning 200.
/// Used by the annotations matrix test.
fn methods_spec(port: u16) -> String {
    format!(
        r#"{{
  "openapi": "3.0.0",
  "info": {{ "title": "Methods", "version": "1.0.0" }},
  "servers": [{{ "url": "http://localhost:{port}" }}],
  "paths": {{
    "/items": {{
      "get":    {{ "operationId": "listItems",   "tags": ["items"], "responses": {{ "200": {{ "description": "OK" }} }} }},
      "post":   {{ "operationId": "createItem",  "tags": ["items"], "responses": {{ "200": {{ "description": "OK" }} }} }},
      "put":    {{ "operationId": "replaceItem", "tags": ["items"], "responses": {{ "200": {{ "description": "OK" }} }} }},
      "patch":  {{ "operationId": "patchItem",   "tags": ["items"], "responses": {{ "200": {{ "description": "OK" }} }} }},
      "delete": {{ "operationId": "deleteItem",  "tags": ["items"], "responses": {{ "200": {{ "description": "OK" }} }} }}
    }},
    "/health": {{
      "get": {{
        "operationId": "health",
        "tags": ["ops", "internal"],
        "x-mcp-annotations": {{ "readOnlyHint": false, "openWorldHint": true }},
        "responses": {{ "200": {{ "description": "OK" }} }}
      }}
    }}
  }}
}}"#
    )
}

/// Spawn `spall mcp <api> --spall-transport http --spall-port 0`
/// and parse the OS-assigned port from the sentinel stderr line.
/// Returns the running child + bound socket address.
async fn spawn_http(
    temp: &TempDir,
    api: &str,
    extra_args: &[&str],
) -> (std::process::Child, String) {
    let cache_dir = temp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let mut cmd = std::process::Command::new(bin_path());
    cmd.env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", &cache_dir)
        .arg("mcp")
        .arg(api)
        .args(["--spall-transport", "http", "--spall-port", "0"])
        .args(extra_args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().expect("spawn spall mcp http");

    // Read stderr until we see the sentinel line.
    let stderr = child.stderr.take().unwrap();
    let mut reader = BufReader::new(stderr);
    let mut url: Option<String> = None;
    // Bounded read loop — 50 lines is plenty for the banner + warnings.
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
    // Restore stderr so the test can still read leftover banner lines if
    // it wants to (we don't, but keeping the handle alive prevents the
    // child from getting SIGPIPE on its next eprintln).
    child.stderr = Some(reader.into_inner());
    (child, url)
}

#[tokio::test]
async fn http_transport_round_trips_initialize_then_tools_list() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let (mut child, base_url) = spawn_http(&temp, "petstore", &[]).await;
    let client = reqwest::Client::new();

    // initialize: server should respond with Mcp-Session-Id header.
    let init = client
        .post(format!("{}/", base_url))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
        }))
        .send()
        .await
        .expect("send initialize");
    assert_eq!(init.status(), reqwest::StatusCode::OK);
    let session_id = init
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("Mcp-Session-Id header must be issued on initialize")
        .to_string();
    assert_eq!(session_id.len(), 32, "session id should be 128-bit hex");
    let init_body: Value = init.json().await.expect("init body");
    assert_eq!(init_body["result"]["serverInfo"]["name"], "spall");

    // tools/list with the issued session id.
    let list = client
        .post(format!("{}/", base_url))
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await
        .expect("send tools/list");
    assert_eq!(list.status(), reqwest::StatusCode::OK);
    let list_body: Value = list.json().await.expect("list body");
    let tools = list_body["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert_eq!(tools.len(), 2);

    let _ = child.kill();
    let _ = child.wait();
}

#[tokio::test]
async fn http_transport_rejects_request_without_session_id() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let (mut child, base_url) = spawn_http(&temp, "petstore", &[]).await;
    let client = reqwest::Client::new();

    // tools/list with no Mcp-Session-Id should be rejected with 400.
    let resp = client
        .post(format!("{}/", base_url))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await
        .expect("send tools/list");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("Mcp-Session-Id"),
        "error message should mention the missing session id: {:?}",
        body,
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[tokio::test]
async fn http_transport_origin_allowlist_blocks_unlisted_origin() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let (mut child, base_url) = spawn_http(
        &temp,
        "petstore",
        &["--spall-allowed-origin", "https://example.com"],
    )
    .await;
    let client = reqwest::Client::new();

    // Request with a non-allowlisted Origin → 403.
    let resp = client
        .post(format!("{}/", base_url))
        .header("origin", "https://evil.example.org")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
        }))
        .send()
        .await
        .expect("send initialize");
    assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);

    // Same request with the allowed Origin succeeds.
    let resp = client
        .post(format!("{}/", base_url))
        .header("origin", "https://example.com")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
        }))
        .send()
        .await
        .expect("send initialize");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let _ = child.kill();
    let _ = child.wait();
}

#[tokio::test]
async fn http_transport_default_origin_policy_blocks_remote_origin() {
    // No --spall-allowed-origin → empty allowlist. The default policy
    // is "localhost or no Origin"; a remote Origin header still 403s.
    // This closes the DNS-rebinding gap the spec calls out: without
    // the check, an attacker-controlled DNS record pointing at
    // 127.0.0.1 served from a remote page could drive a victim's
    // browser into the local server.
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let (mut child, base_url) = spawn_http(&temp, "petstore", &[]).await;
    let client = reqwest::Client::new();

    // Remote Origin → 403 even without an allowlist.
    let resp = client
        .post(format!("{}/", base_url))
        .header("origin", "https://example.com")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
        }))
        .send()
        .await
        .expect("send initialize");
    assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);

    // Localhost Origin → allowed.
    let resp = client
        .post(format!("{}/", base_url))
        .header("origin", "http://localhost:8080")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
        }))
        .send()
        .await
        .expect("send initialize");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    // No Origin header (curl, MCP test client) → allowed.
    let resp = client
        .post(format!("{}/", base_url))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
        }))
        .send()
        .await
        .expect("send initialize");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let _ = child.kill();
    let _ = child.wait();
}

#[tokio::test]
async fn tools_list_carries_annotations_and_meta_tags() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("methods.json");
    std::fs::write(&spec_path, methods_spec(mock.address().port())).unwrap();
    setup_api(&temp, "methods", spec_path.to_str().unwrap());

    let mut server = spawn(&temp, "methods", &[]);
    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
    }));
    let _ = server.recv();
    server.send(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let resp = server.recv();
    let tools = resp["result"]["tools"].as_array().expect("tools array");

    let by_name: std::collections::HashMap<String, Value> = tools
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_string(), t.clone()))
        .collect();

    let cases = [
        ("listitems", true, false, true),
        ("createitem", false, false, false),
        ("replaceitem", false, true, true),
        ("patchitem", false, true, false),
        ("deleteitem", false, true, true),
    ];
    for (name, ro, dest, idem) in cases {
        let tool = by_name
            .get(name)
            .unwrap_or_else(|| panic!("missing tool {}", name));
        let ann = &tool["annotations"];
        if name == "createitem" {
            // POST → no derived hints.
            assert!(
                ann.as_object().unwrap().is_empty(),
                "POST {} should have empty annotations, got {:?}",
                name,
                ann,
            );
        } else {
            assert_eq!(ann["readOnlyHint"], json!(ro), "{}: readOnlyHint", name);
            assert_eq!(
                ann["destructiveHint"],
                json!(dest),
                "{}: destructiveHint",
                name
            );
            assert_eq!(
                ann["idempotentHint"],
                json!(idem),
                "{}: idempotentHint",
                name
            );
        }
        // _meta.spall.tags carries the tag list.
        assert_eq!(
            tool["_meta"]["spall.tags"],
            json!(["items"]),
            "tags for {}",
            name
        );
    }

    // The /health GET sets x-mcp-annotations: { readOnlyHint: false,
    // openWorldHint: true } → readOnlyHint flipped, openWorldHint
    // added, idempotentHint retained from the derived defaults.
    let health = by_name.get("health").expect("health tool");
    assert_eq!(health["annotations"]["readOnlyHint"], json!(false));
    assert_eq!(health["annotations"]["openWorldHint"], json!(true));
    assert_eq!(health["annotations"]["idempotentHint"], json!(true));
    assert_eq!(health["_meta"]["spall.tags"], json!(["ops", "internal"]));

    let _ = shutdown(server);
}

#[tokio::test]
async fn per_tool_auth_override_dispatches_with_profile_bearer() {
    let mock = MockServer::start().await;
    // /admin requires admin bearer; /public requires no auth header at
    // all (a request carrying Authorization should fail the matcher).
    wiremock::Mock::given(method("GET"))
        .and(path("/admin"))
        .and(header("Authorization", "Bearer admin-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": "admin"})))
        .expect(1)
        .mount(&mock)
        .await;
    wiremock::Mock::given(method("GET"))
        .and(path("/public"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": "public"})))
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
    "/admin":  {{ "get": {{ "operationId": "admin-op",  "responses": {{ "200": {{ "description": "OK" }} }} }} }},
    "/public": {{ "get": {{ "operationId": "public-op", "responses": {{ "200": {{ "description": "OK" }} }} }} }}
  }}
}}"#,
        port = mock.address().port()
    );
    std::fs::write(&spec_path, spec).unwrap();

    // Wire two profiles on the api: 'admin' has the bearer, default
    // has no auth. The admin-op tool gets pinned to the admin profile.
    let apis_dir = temp.path().join("spall").join("apis");
    std::fs::create_dir_all(&apis_dir).unwrap();
    let api_toml = format!(
        r#"source = "{}"

[profile.admin]
[profile.admin.auth]
kind = "bearer"
token = "admin-secret"
"#,
        spec_path.to_str().unwrap()
    );
    std::fs::write(apis_dir.join("authed.toml"), api_toml).unwrap();

    let mut server = spawn(&temp, "authed", &["--spall-auth-tool", "admin-op=admin"]);

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
    }));
    let _ = server.recv();

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "admin-op", "arguments": {} }
    }));
    let resp = server.recv();
    assert_eq!(resp["result"]["isError"], false, "admin-op: {:?}", resp);

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": { "name": "public-op", "arguments": {} }
    }));
    let resp = server.recv();
    assert_eq!(resp["result"]["isError"], false, "public-op: {:?}", resp);

    let _ = shutdown(server);
}

/// Issue #19: profiles referenced by `--spall-auth-tool` are resolved
/// lazily on first dispatch. A profile that's validated at startup but
/// never invoked must have zero wire footprint — wiremock's
/// `.expect(0)` on the un-invoked profile's endpoint enforces this at
/// the contract level rather than coupling the test to internal cache
/// shape.
#[tokio::test]
async fn per_tool_auth_lazy_only_resolves_invoked_profile() {
    let mock = MockServer::start().await;
    wiremock::Mock::given(method("GET"))
        .and(path("/admin"))
        .and(header("Authorization", "Bearer admin-tkn"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": "admin"})))
        .expect(1)
        .mount(&mock)
        .await;
    wiremock::Mock::given(method("GET"))
        .and(path("/readonly"))
        .and(header("Authorization", "Bearer readonly-tkn"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": "readonly"})))
        .expect(0)
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
    "/admin":    {{ "get": {{ "operationId": "admin-op",    "responses": {{ "200": {{ "description": "OK" }} }} }} }},
    "/readonly": {{ "get": {{ "operationId": "readonly-op", "responses": {{ "200": {{ "description": "OK" }} }} }} }}
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
token = "admin-tkn"

[profile.readonly]
[profile.readonly.auth]
kind = "bearer"
token = "readonly-tkn"
"#,
        spec_path.to_str().unwrap()
    );
    std::fs::write(apis_dir.join("authed.toml"), api_toml).unwrap();

    let mut server = spawn(
        &temp,
        "authed",
        &[
            "--spall-auth-tool",
            "admin-op=admin",
            "--spall-auth-tool",
            "readonly-op=readonly",
        ],
    );

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
    }));
    let _ = server.recv();

    // Invoke admin-op exactly once; never invoke readonly-op. The
    // wiremock `.expect(0)` on /readonly verifies on drop that the
    // un-invoked profile was never touched.
    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "admin-op", "arguments": {} }
    }));
    let resp = server.recv();
    assert_eq!(resp["result"]["isError"], false, "admin-op: {:?}", resp);

    let _ = shutdown(server);
    // `mock` drops at end of test scope → wiremock verifies expectations.
}

#[tokio::test]
async fn unknown_tool_returns_is_error() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let mut server = spawn(&temp, "petstore", &[]);

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
    }));
    let _ = server.recv();

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "does-not-exist", "arguments": {} }
    }));
    let resp = server.recv();
    assert_eq!(resp["result"]["isError"], true);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("unknown tool"));

    let _ = shutdown(server);
}

/// A POST whose body is a notification (no reply frame) must return
/// `202 Accepted` with an empty body, per MCP spec 2025-06-18 §HTTP
/// "Sending Messages". Regression guard that 202-no-body replaced the
/// old 200-with-`null`-body behavior.
#[tokio::test]
async fn http_transport_notification_returns_202() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let (mut child, base_url) = spawn_http(&temp, "petstore", &[]).await;
    let client = reqwest::Client::new();

    // initialize to mint a session id.
    let init = client
        .post(format!("{}/", base_url))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
        }))
        .send()
        .await
        .expect("send initialize");
    assert_eq!(init.status(), reqwest::StatusCode::OK);
    let session_id = init
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("Mcp-Session-Id header must be issued on initialize")
        .to_string();

    // notifications/initialized has no `id` → no reply frame → 202.
    let resp = client
        .post(format!("{}/", base_url))
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .send()
        .await
        .expect("send notification");
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    let body = resp.bytes().await.expect("read body");
    assert!(
        body.is_empty(),
        "202 Accepted must carry no body, got {} bytes",
        body.len(),
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// A post-`initialize` request carrying an unsupported
/// `MCP-Protocol-Version` is rejected with `400 Bad Request` and a
/// JSON-RPC error envelope.
#[tokio::test]
async fn http_transport_unsupported_protocol_version_400() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let (mut child, base_url) = spawn_http(&temp, "petstore", &[]).await;
    let client = reqwest::Client::new();

    let init = client
        .post(format!("{}/", base_url))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
        }))
        .send()
        .await
        .expect("send initialize");
    let session_id = init
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("session id")
        .to_string();

    // Valid session, but a bogus protocol version → 400 + error envelope.
    let resp = client
        .post(format!("{}/", base_url))
        .header("mcp-session-id", &session_id)
        .header("mcp-protocol-version", "1999-01-01")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await
        .expect("send tools/list");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("MCP-Protocol-Version"),
        "error should name the version header: {:?}",
        body,
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// A post-`initialize` request with NO `MCP-Protocol-Version` header
/// proceeds normally: an absent header means a pre-header client, so
/// the server assumes `2025-03-26` rather than rejecting. Pins the
/// asymmetric rule (absent ⇒ ok, present-unsupported ⇒ 400).
#[tokio::test]
async fn http_transport_absent_protocol_version_ok() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let (mut child, base_url) = spawn_http(&temp, "petstore", &[]).await;
    let client = reqwest::Client::new();

    let init = client
        .post(format!("{}/", base_url))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
        }))
        .send()
        .await
        .expect("send initialize");
    let session_id = init
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("session id")
        .to_string();

    // No protocol-version header at all → request proceeds → 200.
    let resp = client
        .post(format!("{}/", base_url))
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await
        .expect("send tools/list");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["result"]["tools"].is_array(),
        "tools/list should succeed when the version header is absent: {:?}",
        body,
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// `DELETE /` with a valid `Mcp-Session-Id` terminates that session:
/// the DELETE returns 200, and a subsequent request carrying the same
/// id is rejected with 400 + the re-initialize hint (mirroring the
/// missing-session test). Proves client-initiated termination (#18).
#[tokio::test]
async fn http_transport_delete_terminates_session() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let (mut child, base_url) = spawn_http(&temp, "petstore", &[]).await;
    let client = reqwest::Client::new();

    // initialize to mint a session id.
    let init = client
        .post(format!("{}/", base_url))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
        }))
        .send()
        .await
        .expect("send initialize");
    let session_id = init
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("session id")
        .to_string();

    // DELETE with the issued session id → 200, no body.
    let del = client
        .delete(format!("{}/", base_url))
        .header("mcp-session-id", &session_id)
        .send()
        .await
        .expect("send delete");
    assert_eq!(del.status(), reqwest::StatusCode::OK);
    let del_body = del.bytes().await.expect("read delete body");
    assert!(
        del_body.is_empty(),
        "DELETE 200 should carry no body, got {} bytes",
        del_body.len(),
    );

    // The session is now gone: reusing the id → 400 + re-init hint.
    let resp = client
        .post(format!("{}/", base_url))
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await
        .expect("send tools/list");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("Mcp-Session-Id"),
        "terminated session should be rejected with the re-init hint: {:?}",
        body,
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// Session termination is idempotent: a second `DELETE` for the same
/// (now-absent) session id still returns 200 OK — the goal state
/// ("session no longer exists") is already reached (#18 AC).
#[tokio::test]
async fn http_transport_delete_idempotent() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let (mut child, base_url) = spawn_http(&temp, "petstore", &[]).await;
    let client = reqwest::Client::new();

    let init = client
        .post(format!("{}/", base_url))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
        }))
        .send()
        .await
        .expect("send initialize");
    let session_id = init
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("session id")
        .to_string();

    // First DELETE terminates the session → 200.
    let first = client
        .delete(format!("{}/", base_url))
        .header("mcp-session-id", &session_id)
        .send()
        .await
        .expect("send first delete");
    assert_eq!(first.status(), reqwest::StatusCode::OK);

    // Second DELETE for the now-absent id is still 200 (idempotent).
    let second = client
        .delete(format!("{}/", base_url))
        .header("mcp-session-id", &session_id)
        .send()
        .await
        .expect("send second delete");
    assert_eq!(
        second.status(),
        reqwest::StatusCode::OK,
        "DELETE for an absent session id must be idempotent (200 OK)",
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// `DELETE /` with no `Mcp-Session-Id` header is a malformed request:
/// 400 Bad Request. Absent vs. unknown session id are distinct — an
/// absent id is idempotent 200, but a missing header is a client error.
#[tokio::test]
async fn http_transport_delete_missing_header_400() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let (mut child, base_url) = spawn_http(&temp, "petstore", &[]).await;
    let client = reqwest::Client::new();

    // DELETE with no session-id header → 400 + error envelope.
    let resp = client
        .delete(format!("{}/", base_url))
        .send()
        .await
        .expect("send delete");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("Mcp-Session-Id"),
        "missing-header DELETE should name the session id header: {:?}",
        body,
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// The shared Origin gate applies to `DELETE` identically to `POST`:
/// a disallowed Origin under an allowlist is rejected with 403 before
/// the session-id is read. Proves `check_origin` reuse (#18 AC).
#[tokio::test]
async fn http_transport_delete_rejects_disallowed_origin() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let (mut child, base_url) = spawn_http(
        &temp,
        "petstore",
        &["--spall-allowed-origin", "https://example.com"],
    )
    .await;
    let client = reqwest::Client::new();

    // DELETE with a non-allowlisted Origin → 403, never reaching the
    // session-id check.
    let resp = client
        .delete(format!("{}/", base_url))
        .header("origin", "https://evil.example.org")
        .header("mcp-session-id", "deadbeefdeadbeefdeadbeefdeadbeef")
        .send()
        .await
        .expect("send delete");
    assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);

    let _ = child.kill();
    let _ = child.wait();
}

/// SSE content negotiation (#12): a `tools/call` POST carrying
/// `Accept: text/event-stream` whose dispatch yields multiple reply
/// frames is answered as `text/event-stream` with one `data:` line per
/// frame. The `__spall_test_multi` placeholder tool (cfg(debug_assertions)-only on
/// the dispatcher) emits a progress notification frame followed by the
/// result frame, so a conformant SSE body carries ≥2 `data:` lines.
#[tokio::test]
async fn http_transport_sse_multi_event() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let (mut child, base_url) = spawn_http(&temp, "petstore", &[]).await;
    let client = reqwest::Client::new();

    // initialize to mint a session id.
    let init = client
        .post(format!("{}/", base_url))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
        }))
        .send()
        .await
        .expect("send initialize");
    let session_id = init
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("session id")
        .to_string();

    // tools/call against the multi-frame placeholder, asking for SSE.
    let resp = client
        .post(format!("{}/", base_url))
        .header("mcp-session-id", &session_id)
        .header("accept", "text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "__spall_test_multi", "arguments": {} }
        }))
        .send()
        .await
        .expect("send tools/call");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        content_type.starts_with("text/event-stream"),
        "SSE-accepting POST must answer text/event-stream, got: {:?}",
        content_type,
    );
    // The stream completes after the last frame (`stream::iter`), so
    // reading to end is bounded.
    let body = resp.text().await.expect("read SSE body");
    let data_lines = body.lines().filter(|l| l.starts_with("data:")).count();
    assert!(
        data_lines >= 2,
        "multi-frame SSE body must carry ≥2 data: lines, got {} in:\n{}",
        data_lines,
        body,
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// JSON is the default reply shape (#12): a `tools/call` with no
/// `Accept: text/event-stream` gets `application/json` carrying the
/// single JSON-RPC result frame, not an SSE stream. Pins the
/// content-negotiation default (server MAY always answer JSON).
#[tokio::test]
async fn http_transport_json_response_default() {
    let mock = MockServer::start().await;
    wiremock::Mock::given(method("GET"))
        .and(path("/pets/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 7, "name": "Rex"})))
        .mount(&mock)
        .await;

    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let (mut child, base_url) = spawn_http(&temp, "petstore", &[]).await;
    let client = reqwest::Client::new();

    let init = client
        .post(format!("{}/", base_url))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
        }))
        .send()
        .await
        .expect("send initialize");
    let session_id = init
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("session id")
        .to_string();

    // tools/call with no Accept header → application/json single result.
    let resp = client
        .post(format!("{}/", base_url))
        .header("mcp-session-id", &session_id)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "getpetbyid", "arguments": { "petId": 7 } }
        }))
        .send()
        .await
        .expect("send tools/call");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        content_type.starts_with("application/json"),
        "default tools/call must answer application/json, got: {:?}",
        content_type,
    );
    let body: Value = resp.json().await.expect("json body");
    assert_eq!(body["id"], 2);
    assert_eq!(body["result"]["isError"], false);
    let text = body["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    let echoed: Value = serde_json::from_str(text).expect("parse echoed body");
    assert_eq!(echoed["id"], 7);
    assert_eq!(echoed["name"], "Rex");

    let _ = child.kill();
    let _ = child.wait();
}

/// GET `/` opens the server→client SSE channel (#12): with a valid
/// session id and `Accept: text/event-stream`, the server responds
/// `200 OK` + `Content-Type: text/event-stream`. The stream is
/// keep-alive-only (no server-push source yet — see #47/#48), so it
/// never completes; this asserts on the RESPONSE HEAD only and drops
/// the connection without reading the (unbounded) body.
#[tokio::test]
async fn http_transport_get_opens_sse() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec_path = temp.path().join("petstore.json");
    std::fs::write(&spec_path, pet_spec(mock.address().port())).unwrap();
    setup_api(&temp, "petstore", spec_path.to_str().unwrap());

    let (mut child, base_url) = spawn_http(&temp, "petstore", &[]).await;

    // Mint a session via a default client.
    let init_client = reqwest::Client::new();
    let init = init_client
        .post(format!("{}/", base_url))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name":"t","version":"0"} }
        }))
        .send()
        .await
        .expect("send initialize");
    let session_id = init
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("session id")
        .to_string();

    // The GET stream stays open (keep-alive), so use a short read timeout
    // and assert on the response head only. `send()` returns once headers
    // arrive; we never call `.text()` / `.bytes()` (which would block on
    // the never-ending body).
    let get_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .expect("build client");
    let resp = get_client
        .get(format!("{}/", base_url))
        .header("mcp-session-id", &session_id)
        .header("accept", "text/event-stream")
        .send()
        .await
        .expect("send GET");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        content_type.starts_with("text/event-stream"),
        "GET / with SSE Accept must open a text/event-stream channel, got: {:?}",
        content_type,
    );
    // Drop the response (and its in-flight body) without draining it.
    drop(resp);

    let _ = child.kill();
    let _ = child.wait();
}
