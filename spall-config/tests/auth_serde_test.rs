//! Issue #13 Phase 3: round-trip + redaction tests for `AuthConfig`'s
//! `Option<SecretString>` fields.
//!
//! Locks down three invariants:
//! 1. Inline TOML credentials still deserialize into `SecretString`,
//!    preserving the existing `token = "..."` fixture syntax used
//!    throughout `spall-cli/tests/`.
//! 2. The (currently unused) `Serialize` derive on `AuthConfig` cannot
//!    emit the plaintext value of any secret field. The serializer
//!    redacts to `None` via `serialize_redacted_secret`; round-trip
//!    through serialize+deserialize is `None`-preserving by design.
//! 3. `#[serde(deny_unknown_fields)]` is not weakened by the new
//!    `deserialize_with` adapters — unknown keys still error.

use secrecy::{ExposeSecret, SecretString};
use spall_config::auth::{AuthConfig, AuthKind};

#[test]
fn secret_fields_round_trip_through_toml_deserialize() {
    let toml_src = r#"
kind = "bearer"
token = "hunter2"
password = "pw123"
client_secret = "client-secret-val"
"#;
    let cfg: AuthConfig = toml::from_str(toml_src).expect("parse TOML");
    assert_eq!(cfg.kind, Some(AuthKind::Bearer));
    assert_eq!(
        cfg.token.as_ref().map(|s| s.expose_secret()),
        Some("hunter2"),
    );
    assert_eq!(
        cfg.password.as_ref().map(|s| s.expose_secret()),
        Some("pw123"),
    );
    assert_eq!(
        cfg.client_secret.as_ref().map(|s| s.expose_secret()),
        Some("client-secret-val"),
    );
}

#[test]
fn serde_json_serialize_redacts_secret_values() {
    let cfg = AuthConfig {
        kind: Some(AuthKind::Bearer),
        token: Some(SecretString::new("hunter2".to_string().into())),
        password: Some(SecretString::new("pw123".to_string().into())),
        client_secret: Some(SecretString::new("client-secret-val".to_string().into())),
        ..Default::default()
    };
    let json = serde_json::to_string(&cfg).expect("serialize JSON");
    // The plaintext secret values must NEVER appear in the output —
    // the actual security property. The JSON serializer emits the
    // field keys with `null` values (since serialize_none() produces
    // null in JSON), which is harmless.
    assert!(
        !json.contains("hunter2"),
        "token plaintext leaked into JSON output: {}",
        json,
    );
    assert!(
        !json.contains("pw123"),
        "password plaintext leaked into JSON output: {}",
        json,
    );
    assert!(
        !json.contains("client-secret-val"),
        "client_secret plaintext leaked into JSON output: {}",
        json,
    );
    // Confirm the redaction shape: each secret field's value is `null`.
    assert!(json.contains("\"token\":null"), "expected null token: {}", json);
    assert!(json.contains("\"password\":null"), "expected null password: {}", json);
    assert!(
        json.contains("\"client_secret\":null"),
        "expected null client_secret: {}",
        json,
    );
}

#[test]
fn toml_serialize_omits_secret_fields() {
    let cfg = AuthConfig {
        kind: Some(AuthKind::Bearer),
        // Include one non-secret survivor so the resulting TOML table
        // isn't empty after all the secret fields redact to None.
        username: Some("alice".to_string()),
        token: Some(SecretString::new("hunter2".to_string().into())),
        password: Some(SecretString::new("pw123".to_string().into())),
        client_secret: Some(SecretString::new("client-secret-val".to_string().into())),
        ..Default::default()
    };
    let out = toml::to_string(&cfg).expect("serialize TOML");
    assert!(
        !out.contains("hunter2"),
        "token plaintext leaked into TOML output: {}",
        out,
    );
    assert!(
        !out.contains("pw123"),
        "password plaintext leaked into TOML output: {}",
        out,
    );
    assert!(
        !out.contains("client-secret-val"),
        "client_secret plaintext leaked into TOML output: {}",
        out,
    );
    // TOML has no null, so `serialize_none()` causes the field to be
    // omitted entirely — assert key absence.
    assert!(
        !out.contains("token"),
        "TOML should omit the token key entirely: {}",
        out,
    );
    assert!(
        !out.contains("password"),
        "TOML should omit the password key entirely: {}",
        out,
    );
    assert!(
        !out.contains("client_secret"),
        "TOML should omit the client_secret key entirely: {}",
        out,
    );
    // Sanity: the non-secret survivor remains.
    assert!(out.contains("alice"), "username should survive: {}", out);
}

#[test]
fn deny_unknown_fields_still_rejects_after_secret_migration() {
    // The new `deserialize_with` adapters wrap a known field through
    // `Option::<String>::deserialize`, not a flatten/passthrough — so
    // `#[serde(deny_unknown_fields)]` continues to fire only on
    // genuinely unknown keys. Guard against a future refactor that
    // would weaken this contract.
    let result: Result<AuthConfig, _> = toml::from_str(
        r#"
kind = "bearer"
nope_unknown = "x"
"#,
    );
    assert!(
        result.is_err(),
        "deny_unknown_fields should reject 'nope_unknown'; got: {:?}",
        result,
    );
}
