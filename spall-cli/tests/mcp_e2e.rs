//! End-to-end tests for `spall mcp <api>` over stdio.
//!
//! Spawn the binary as a subprocess with stdin/stdout piped; speak
//! line-delimited JSON-RPC 2.0; assert the wire shape and that the
//! backend wiremock server received the dispatched HTTP call.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use tempfile::TempDir;
use wiremock::matchers::{method, path, query_param};
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
    assert_eq!(first, second, "truncation must be deterministic across runs");
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
    assert!(body[0].starts_with("orgs\t3\torgs-op-"), "row 0: {}", body[0]);
    assert!(body[1].starts_with("users\t3\tusers-op-"), "row 1: {}", body[1]);
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
