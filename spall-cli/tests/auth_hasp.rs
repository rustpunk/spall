//! End-to-end hasp integration tests: verify `env://` and `file://` token_url resolution.

use std::process::Command;
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{MockServer, ResponseTemplate};

fn bin_path() -> String {
    std::env::var("CARGO_BIN_EXE_spall")
        .unwrap_or_else(|_| String::from("target/debug/spall"))
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

fn setup_api(temp: &TempDir, api_name: &str, spec_path: &str, toml_extra: &str) {
    let config_dir = temp.path().join("spall");
    let apis_dir = config_dir.join("apis");
    std::fs::create_dir_all(&apis_dir).unwrap();

    let mut api_toml = format!(r#"source = "{}"{}"#, spec_path, toml_extra);
    if !toml_extra.is_empty() && !toml_extra.starts_with('\n') {
        api_toml.insert(spec_path.len() + 11, '\n');
    }
    std::fs::write(apis_dir.join(format!("{}.toml", api_name)), api_toml).unwrap();
}

#[tokio::test]
async fn bearer_auth_from_env_via_hasp() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec = minimal_spec(mock.address().port());
    let spec_path = temp.path().join("spec.json").to_str().unwrap().to_string();
    std::fs::write(&spec_path, &spec).unwrap();

    setup_api(
        &temp,
        "testapi",
        &spec_path,
        "\n[auth]\nkind = \"bearer\"\ntoken_url = \"env://SPALL_HASP_TEST_TOKEN\"\n",
    );

    wiremock::Mock::given(method("GET"))
        .and(path("/items"))
        .and(header("Authorization", "Bearer hasp-env-token"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("SPALL_HASP_TEST_TOKEN", "hasp-env-token")
        .args(["testapi", "get-items"])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn bearer_auth_from_file_via_hasp() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec = minimal_spec(mock.address().port());
    let spec_path = temp.path().join("spec.json").to_str().unwrap().to_string();
    std::fs::write(&spec_path, &spec).unwrap();

    // Write secret to a file inside the temp dir.
    let secret_path = temp.path().join("secret.txt");
    std::fs::write(&secret_path, "hasp-file-token\n").unwrap();
    let file_url = format!("file://{}", secret_path.to_str().unwrap());

    setup_api(
        &temp,
        "testapi",
        &spec_path,
        &format!("\n[auth]\nkind = \"bearer\"\ntoken_url = \"{}\"\n", file_url),
    );

    wiremock::Mock::given(method("GET"))
        .and(path("/items"))
        .and(header("Authorization", "Bearer hasp-file-token"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .args(["testapi", "get-items"])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn basic_auth_password_from_env_via_hasp() {
    let mock = MockServer::start().await;
    let temp = TempDir::new().unwrap();
    let spec = minimal_spec(mock.address().port());
    let spec_path = temp.path().join("spec.json").to_str().unwrap().to_string();
    std::fs::write(&spec_path, &spec).unwrap();

    setup_api(
        &temp,
        "testapi",
        &spec_path,
        "\n[auth]\nkind = \"basic\"\nusername = \"alice\"\npassword_url = \"env://ALICE_HASP_PASS\"\n",
    );

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let expected = format!("Basic {}", STANDARD.encode("alice:secret"));

    wiremock::Mock::given(method("GET"))
        .and(path("/items"))
        .and(header("Authorization", expected.as_str()))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock)
        .await;

    let output = Command::new(bin_path())
        .env("XDG_CONFIG_HOME", temp.path())
        .env("ALICE_HASP_PASS", "secret")
        .args(["testapi", "get-items"])
        .output()
        .expect("failed to run spall");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
