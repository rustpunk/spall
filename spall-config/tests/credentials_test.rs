use secrecy::ExposeSecret;
use spall_config::auth::default_token_env;
use spall_config::credentials::{CredentialKind, CredentialResolver};

#[test]
fn env_var_name_formatting() {
    let r = CredentialResolver {
        api_name: "my-api".into(),
    };
    assert_eq!(r.env_var_name(), "SPALL_MY_API_TOKEN");
}

#[test]
fn default_token_env_basic() {
    assert_eq!(default_token_env("foo-bar"), "SPALL_FOO_BAR_TOKEN");
}

#[test]
fn resolve_with_cli_auth_bearer() {
    let r = CredentialResolver {
        api_name: "github".into(),
    };
    let cred = r.resolve(Some("Bearer abc123")).expect("should resolve");
    assert_eq!(cred.kind, CredentialKind::Bearer);
    assert_eq!(cred.value.expose_secret(), "abc123");
}

#[test]
fn resolve_with_cli_auth_basic() {
    let r = CredentialResolver {
        api_name: "github".into(),
    };
    let cred = r.resolve(Some("alice:secret")).expect("should resolve");
    assert_eq!(cred.kind, CredentialKind::Basic);
    assert_eq!(cred.value.expose_secret(), "alice:secret");
}

#[test]
fn resolve_with_cli_auth_basic_prefix() {
    let r = CredentialResolver {
        api_name: "github".into(),
    };
    let cred = r
        .resolve(Some("Basic YWxpY2U6c2VjcmV0"))
        .expect("should resolve");
    assert_eq!(cred.kind, CredentialKind::Basic);
    assert_eq!(cred.value.expose_secret(), "alice:secret");
}

#[test]
fn resolve_with_env_var() {
    let api_name = "env-var-api";
    let var = default_token_env(api_name);
    let r = CredentialResolver {
        api_name: api_name.into(),
    };
    std::env::set_var(&var, "env-token");
    let cred = r.resolve(None).expect("should resolve from env");
    assert_eq!(cred.kind, CredentialKind::Bearer);
    assert_eq!(cred.value.expose_secret(), "env-token");
    std::env::remove_var(&var);
}

#[test]
fn resolve_no_auth() {
    let api_name = "no-auth-api";
    let r = CredentialResolver {
        api_name: api_name.into(),
    };
    std::env::remove_var(default_token_env(api_name));
    assert!(r.resolve(None).is_none());
}

#[test]
fn infer_auth_basic() {
    let api_name = "infer-basic";
    let var = default_token_env(api_name);
    let r = CredentialResolver {
        api_name: api_name.into(),
    };
    std::env::set_var(&var, "user:pass");
    let cred = r.resolve(None).expect("should resolve");
    assert_eq!(cred.kind, CredentialKind::Basic);
    assert_eq!(cred.value.expose_secret(), "user:pass");
    std::env::remove_var(&var);
}

#[test]
fn infer_auth_bearer() {
    let api_name = "infer-bearer";
    let var = default_token_env(api_name);
    let r = CredentialResolver {
        api_name: api_name.into(),
    };
    std::env::set_var(&var, "ghp_1234567890abcdef");
    let cred = r.resolve(None).expect("should resolve");
    assert_eq!(cred.kind, CredentialKind::Bearer);
    assert_eq!(cred.value.expose_secret(), "ghp_1234567890abcdef");
    std::env::remove_var(&var);
}
