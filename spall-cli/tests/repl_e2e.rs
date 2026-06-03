//! End-to-end test for the REPL mode.

use std::io::{BufRead, Read, Write};
use std::process::{Command, Stdio};
use tempfile::TempDir;

#[allow(dead_code)]
fn bin_path() -> String {
    std::env::var("CARGO_BIN_EXE_spall").unwrap_or_else(|_| String::from("target/debug/spall"))
}

fn minimal_spec(port: u16) -> String {
    format!(
        r#"{{
  "openapi": "3.0.0",
  "info": {{ "title": "Test", "version": "1.0.0" }},
  "servers": [{{ "url": "http://localhost:{}" }}],
  "paths": {{
    "/items": {{
      "get": {{
        "operationId": "get-items",
        "responses": {{ "200": {{ "description": "OK" }} }}
      }}
    }}
  }}
}}"#,
        port
    )
}

fn setup_api_config(temp: &TempDir, spec_path: &str) {
    let config_dir = temp.path().join("spall");
    let apis_dir = config_dir.join("apis");
    std::fs::create_dir_all(&apis_dir).unwrap();
    let api_toml = format!(r#"source = "{}""#, spec_path);
    std::fs::write(apis_dir.join("testapi.toml"), api_toml).unwrap();
}

/// Register an additional API under an arbitrary name and spec path.
fn register_api(temp: &TempDir, name: &str, spec_path: &str) {
    let apis_dir = temp.path().join("spall").join("apis");
    std::fs::create_dir_all(&apis_dir).unwrap();
    let api_toml = format!(r#"source = "{}""#, spec_path);
    std::fs::write(apis_dir.join(format!("{}.toml", name)), api_toml).unwrap();
}

/// A spec with a producer `get-items` and a path-param consumer `get-by-id`,
/// used to exercise a successful REPL pipe into a path parameter (#34).
fn chain_spec(port: u16) -> String {
    format!(
        r#"{{
  "openapi": "3.0.0",
  "info": {{ "title": "Test", "version": "1.0.0" }},
  "servers": [{{ "url": "http://localhost:{}" }}],
  "paths": {{
    "/items": {{
      "get": {{
        "operationId": "get-items",
        "responses": {{ "200": {{ "description": "OK" }} }}
      }}
    }},
    "/items/{{id}}": {{
      "get": {{
        "operationId": "get-by-id",
        "parameters": [
          {{ "name": "id", "in": "path", "required": true, "schema": {{ "type": "string" }} }}
        ],
        "responses": {{ "200": {{ "description": "OK" }} }}
      }}
    }}
  }}
}}"#,
        port
    )
}

/// A spec for a second API whose only operation is `other-op`.
fn other_api_spec(port: u16) -> String {
    format!(
        r#"{{
  "openapi": "3.0.0",
  "info": {{ "title": "Other", "version": "1.0.0" }},
  "servers": [{{ "url": "http://localhost:{}" }}],
  "paths": {{
    "/other": {{
      "get": {{
        "operationId": "other-op",
        "responses": {{ "200": {{ "description": "OK" }} }}
      }}
    }}
  }}
}}"#,
        port
    )
}

/// Build the full file-system path for a command using `find`.
fn find_bin() -> String {
    std::env::var("CARGO_BIN_EXE_spall").unwrap_or_else(|_| {
        let output = Command::new("find")
            .args([
                "target",
                "-maxdepth",
                "2",
                "-name",
                "spall",
                "-type",
                "f",
                "-executable",
            ])
            .output()
            .expect("find failed");
        let paths = String::from_utf8_lossy(&output.stdout);
        paths
            .lines()
            .next()
            .unwrap_or("target/debug/spall")
            .trim()
            .to_string()
    })
}

#[tokio::test]
async fn repl_dispatches_api_command() {
    let mock = wiremock::MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec = minimal_spec(mock.address().port());
    let spec_path = temp.path().join("spec.json").to_str().unwrap().to_string();
    std::fs::write(&spec_path, &spec).unwrap();
    setup_api_config(&temp, &spec_path);

    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({"result": "ok"})),
        )
        .mount(&mock)
        .await;

    let mut child = Command::new(find_bin())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn spall repl");

    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut stdout_reader = std::io::BufReader::new(stdout);

    // Wait for the prompt (or banner) to appear.
    let mut line = String::new();
    loop {
        line.clear();
        let n = stdout_reader.read_line(&mut line).expect("read stdout");
        if n == 0 {
            break;
        }
        if line.contains("spall") {
            break;
        }
    }

    // Send the command through REPL.
    {
        let mut s = stdin;
        writeln!(s, "testapi get-items").unwrap();
        writeln!(s, "exit").unwrap();
        // stdin closes when s drops
    }

    // Collect stdout.
    let mut output = String::new();
    loop {
        line.clear();
        let n = stdout_reader.read_line(&mut line).expect("read stdout");
        if n == 0 {
            break;
        }
        output.push_str(&line);
    }

    let status = child.wait().expect("wait for child");
    assert!(
        status.success(),
        "REPL exited with non-zero status. stderr: {}",
        {
            let mut stderr_child = child.stderr.take().map(std::io::BufReader::new);
            stderr_child.as_mut().map_or_else(String::new, |r| {
                let mut buf = String::new();
                let _ = r.read_to_string(&mut buf);
                buf
            })
        }
    );

    assert!(
        output.contains("result"),
        "Expected API response in REPL stdout, got: {}",
        output
    );
}

#[tokio::test]
async fn repl_special_commands() {
    let temp = TempDir::new().unwrap();

    let mut child = Command::new(find_bin())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn spall repl");

    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut stdout_reader = std::io::BufReader::new(stdout);

    // Wait for banner / prompt.
    wait_for_repl_prompt(&mut stdout_reader);

    {
        let mut s = stdin;
        writeln!(s, "help").unwrap();
        writeln!(s, "history").unwrap();
        writeln!(s, "quit").unwrap();
    }

    let mut output = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        let n = stdout_reader.read_line(&mut line).expect("read stdout");
        if n == 0 {
            break;
        }
        output.push_str(&line);
    }

    let status = child.wait().expect("wait for child");
    assert!(
        status.success(),
        "REPL exited with non-zero status. stderr: {}",
        {
            let mut stderr_child = child.stderr.take().map(std::io::BufReader::new);
            stderr_child.as_mut().map_or_else(String::new, |r| {
                let mut buf = String::new();
                let _ = r.read_to_string(&mut buf);
                buf
            })
        }
    );
    assert!(
        output.contains("Special commands") || output.contains("help"),
        "Expected help text in REPL stdout, got: {}",
        output
    );
    assert!(
        output.contains("No history") || output.contains("Timestamp"),
        "Expected history output in REPL stdout, got: {}",
        output
    );
}

fn wait_for_repl_prompt(reader: &mut std::io::BufReader<std::process::ChildStdout>) {
    let mut buf = String::new();
    loop {
        buf.clear();
        let n = reader.read_line(&mut buf).expect("read stdout");
        if n == 0 {
            return;
        }
        if buf.contains("spall") {
            return;
        }
    }
}

/// #37 regression: a stale response from an earlier single command must NOT
/// leak into a later pipe whose stage-0 produced no response. With the former
/// process-global `LAST_RESPONSE`, `testapi get-items` populated the static,
/// and the later `history | ...` pipe consumed that stale value. With the
/// per-pipe `ResponseContext`, stage-0 `history` never writes a response, so
/// the pipe must fail with `NoPreviousResponse`.
#[tokio::test]
async fn repl_pipe_does_not_leak_stale_response() {
    let mock = wiremock::MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec = minimal_spec(mock.address().port());
    let spec_path = temp.path().join("spec.json").to_str().unwrap().to_string();
    std::fs::write(&spec_path, &spec).unwrap();
    setup_api_config(&temp, &spec_path);

    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": 7})),
        )
        .mount(&mock)
        .await;

    let mut child = Command::new(find_bin())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn spall repl");

    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut stdout_reader = std::io::BufReader::new(stdout);
    wait_for_repl_prompt(&mut stdout_reader);

    {
        let mut s = stdin;
        // Populate a response under the OLD global behaviour.
        writeln!(s, "testapi get-items").unwrap();
        // Stage 0 (`history list`) is a non-operation command: it must NOT
        // supply a response to stage 1.
        writeln!(s, "history list | get-items --id $.id").unwrap();
        writeln!(s, "exit").unwrap();
    }

    // Drain stdout so the child can exit.
    let mut line = String::new();
    loop {
        line.clear();
        if stdout_reader.read_line(&mut line).expect("read stdout") == 0 {
            break;
        }
    }

    let mut stderr_buf = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr_buf);
    }
    let _ = child.wait().expect("wait for child");

    assert!(
        stderr_buf.contains("no response from previous stage"),
        "pipe with a non-operation stage-0 must fail with NoPreviousResponse, stderr: {}",
        stderr_buf
    );
}

/// #40 regression: a pipe whose chain target belongs to a *different*
/// registered API must fail with an actionable error naming the missing
/// target, not silently dispatch against the stage-0 API or report a confusing
/// "Unknown operation". Chaining stays within the stage-0 API.
#[tokio::test]
async fn repl_pipe_cross_api_target_is_actionable_error() {
    let mock = wiremock::MockServer::start().await;
    let temp = TempDir::new().unwrap();

    let spec = minimal_spec(mock.address().port());
    let spec_path = temp.path().join("spec.json").to_str().unwrap().to_string();
    std::fs::write(&spec_path, &spec).unwrap();
    setup_api_config(&temp, &spec_path);

    // A second API whose op `other-op` is NOT part of testapi's spec.
    let other_spec = other_api_spec(mock.address().port());
    let other_path = temp.path().join("other.json").to_str().unwrap().to_string();
    std::fs::write(&other_path, &other_spec).unwrap();
    register_api(&temp, "otherapi", &other_path);

    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/items"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": 7})),
        )
        .mount(&mock)
        .await;

    let mut child = Command::new(find_bin())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn spall repl");

    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut stdout_reader = std::io::BufReader::new(stdout);
    wait_for_repl_prompt(&mut stdout_reader);

    {
        let mut s = stdin;
        // Stage 0 hits testapi; the chain target `other-op` lives in otherapi.
        writeln!(s, "testapi get-items | other-op --id $.id").unwrap();
        writeln!(s, "exit").unwrap();
    }

    let mut line = String::new();
    loop {
        line.clear();
        if stdout_reader.read_line(&mut line).expect("read stdout") == 0 {
            break;
        }
    }

    let mut stderr_buf = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr_buf);
    }
    let _ = child.wait().expect("wait for child");

    assert!(
        stderr_buf.contains("other-op") && stderr_buf.contains("not an operation of API"),
        "cross-API chain target must produce an actionable error, stderr: {}",
        stderr_buf
    );
}

/// #34 (REPL): a successful pipe feeds a captured id into a path parameter of a
/// same-API target op, hitting the resolved path exactly once.
#[tokio::test]
async fn repl_pipe_into_path_param_succeeds() {
    let mock = wiremock::MockServer::start().await;
    let temp = TempDir::new().unwrap();

    let spec = chain_spec(mock.address().port());
    let spec_path = temp.path().join("spec.json").to_str().unwrap().to_string();
    std::fs::write(&spec_path, &spec).unwrap();
    setup_api_config(&temp, &spec_path);

    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/items"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "99"})),
        )
        .expect(1)
        .mount(&mock)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/items/99"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})),
        )
        .expect(1)
        .mount(&mock)
        .await;

    let mut child = Command::new(find_bin())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn spall repl");

    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut stdout_reader = std::io::BufReader::new(stdout);
    wait_for_repl_prompt(&mut stdout_reader);

    {
        let mut s = stdin;
        writeln!(s, "testapi get-items | get-by-id --id id").unwrap();
        writeln!(s, "exit").unwrap();
    }

    let mut line = String::new();
    loop {
        line.clear();
        if stdout_reader.read_line(&mut line).expect("read stdout") == 0 {
            break;
        }
    }

    let mut stderr_buf = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr_buf);
    }
    let _ = child.wait().expect("wait for child");

    assert!(
        !stderr_buf.contains("Pipe failed"),
        "REPL pipe into a path param should not fail, stderr: {}",
        stderr_buf
    );
    // The .expect(1) mocks verify /items and /items/99 were each hit once.
}
