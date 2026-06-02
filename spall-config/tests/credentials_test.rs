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

/// Issue #38 exact failure: a URL-shaped env token used to parse as
/// `("https", "//example.com/x")` → Basic → silent 401. It must now be Bearer.
#[test]
fn infer_auth_url_shaped_is_bearer() {
    let api_name = "infer-url-shaped";
    let var = default_token_env(api_name);
    let r = CredentialResolver {
        api_name: api_name.into(),
    };
    std::env::set_var(&var, "https://example.com/x");
    let cred = r.resolve(None).expect("should resolve");
    assert_eq!(cred.kind, CredentialKind::Bearer);
    assert_eq!(cred.value.expose_secret(), "https://example.com/x");
    std::env::remove_var(&var);
}

/// A multi-colon token (`a:b:c`) is ambiguous and must classify as Bearer from
/// the env path, with its value passed through unchanged.
#[test]
fn infer_auth_multi_colon_is_bearer() {
    let api_name = "infer-multi-colon";
    let var = default_token_env(api_name);
    let r = CredentialResolver {
        api_name: api_name.into(),
    };
    std::env::set_var(&var, "a:b:c");
    let cred = r.resolve(None).expect("should resolve");
    assert_eq!(cred.kind, CredentialKind::Bearer);
    assert_eq!(cred.value.expose_secret(), "a:b:c");
    std::env::remove_var(&var);
}

/// A non-http `scheme://` token (`keyring://...`) must also be Bearer: the
/// `//`-after-colon clause rejects every scheme-like URL, not just http(s).
#[test]
fn infer_auth_scheme_like_keyring_is_bearer() {
    let api_name = "infer-keyring";
    let var = default_token_env(api_name);
    let r = CredentialResolver {
        api_name: api_name.into(),
    };
    std::env::set_var(&var, "keyring://spall/x");
    let cred = r.resolve(None).expect("should resolve");
    assert_eq!(cred.kind, CredentialKind::Bearer);
    assert_eq!(cred.value.expose_secret(), "keyring://spall/x");
    std::env::remove_var(&var);
}

/// A `--spall-auth` token containing whitespace (and no recognized prefix) must
/// be Bearer: whitespace makes it an ambiguous, non-`user:pass` value.
#[test]
fn cli_basic_with_whitespace_is_bearer() {
    let r = CredentialResolver {
        api_name: "github".into(),
    };
    let cred = r.resolve(Some("user pass")).expect("should resolve");
    assert_eq!(cred.kind, CredentialKind::Bearer);
    assert_eq!(cred.value.expose_secret(), "user pass");
}

/// Core cross-source-consistency property: a byte-identical token classifies to
/// the same kind and exposes the same value whether it arrives via `--spall-auth`
/// or via the `SPALL_<API>_TOKEN` env var. This is the invariant issue #38 broke.
#[test]
fn cli_and_env_classify_identically() {
    let cases = [
        ("a:b:c", CredentialKind::Bearer),
        ("https://example.com/x", CredentialKind::Bearer),
        ("user:pass", CredentialKind::Basic),
        ("ghp_abc123", CredentialKind::Bearer),
    ];

    for (i, (tok, expected_kind)) in cases.iter().enumerate() {
        // CLI path.
        let cli_resolver = CredentialResolver {
            api_name: "consistency-cli".into(),
        };
        let cli_cred = cli_resolver.resolve(Some(tok)).expect("cli should resolve");

        // Env path: unique api_name per case to avoid env cross-talk.
        let api_name = format!("consistency-env-{}", i);
        let var = default_token_env(&api_name);
        let env_resolver = CredentialResolver {
            api_name: api_name.clone(),
        };
        std::env::set_var(&var, tok);
        let env_cred = env_resolver.resolve(None).expect("env should resolve");
        std::env::remove_var(&var);

        assert_eq!(
            cli_cred.kind, *expected_kind,
            "CLI kind mismatch for {tok:?}"
        );
        assert_eq!(
            env_cred.kind, *expected_kind,
            "env kind mismatch for {tok:?}"
        );
        assert_eq!(
            cli_cred.kind, env_cred.kind,
            "CLI and env disagree on kind for {tok:?}"
        );
        assert_eq!(
            cli_cred.value.expose_secret(),
            env_cred.value.expose_secret(),
            "CLI and env disagree on exposed value for {tok:?}"
        );
    }
}
