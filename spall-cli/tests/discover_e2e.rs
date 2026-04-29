//! End-to-end tests for `spall api discover` (RFC 8631 autodiscovery).

use std::process::Command;
use tempfile::TempDir;

fn bin_path() -> String {
    std::env::var("CARGO_BIN_EXE_spall").unwrap_or_else(|_| String::from("target/debug/spall"))
}

#[tokio::test]
async fn discover_via_link_header() {
    let mock = wiremock::MockServer::start().await;
    let temp = TempDir::new().unwrap();

    let spec_url = format!("http://localhost:{}/openapi.json", mock.address().port());
    let probe_url = format!("http://localhost:{}/", mock.address().port());

    let spec = serde_json::json!({
        "openapi": "3.0.0",
        "info": { "title": "My Test API", "version": "1.0.0" },
        "servers": [{"url": probe_url}],
        "paths": {}
    });

    // Probe response with Link header.
    wiremock::Mock::given(wiremock::matchers::method("HEAD"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("Link", format!("<{}>; rel=\"service-desc\"", spec_url)),
        )
        .mount(&mock)
        .await;

    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/openapi.json"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(spec))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args(["api", "discover", &probe_url])
        .output()
        .expect("failed to run spall");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stderr: {}", stderr);
    assert!(
        stderr.contains("my-test-api") || stderr.contains("My Test API"),
        "Expected discovery output, got stderr: {}",
        stderr
    );

    // Verify it was registered.
    let list = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args(["api", "list"])
        .output()
        .expect("failed to run spall");

    let list_stderr = String::from_utf8_lossy(&list.stderr);
    assert!(
        list_stderr.contains("my-test-api"),
        "Expected API in list: {}",
        list_stderr
    );
}
