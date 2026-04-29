//! End-to-end test for the REPL mode.

use std::io::{BufRead, Read, Write};
use std::process::{Command, Stdio};
use tempfile::TempDir;

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
