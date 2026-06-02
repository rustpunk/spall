use spall_config::auth::default_token_env;
use spall_config::credentials::{classify_bare_token, CredentialKind};

#[test]
fn default_token_env_basic() {
    assert_eq!(default_token_env("foo-bar"), "SPALL_FOO_BAR_TOKEN");
}

#[test]
fn classify_user_pass_is_basic() {
    assert_eq!(classify_bare_token("alice:secret"), CredentialKind::Basic);
}

#[test]
fn classify_plain_token_is_bearer() {
    assert_eq!(
        classify_bare_token("ghp_1234567890abcdef"),
        CredentialKind::Bearer
    );
}

#[test]
fn classify_url_shaped_is_bearer() {
    // A single colon and no whitespace, but a scheme URL — never credentials.
    assert_eq!(
        classify_bare_token("https://example.com"),
        CredentialKind::Bearer
    );
    assert_eq!(
        classify_bare_token("keyring://service/key"),
        CredentialKind::Bearer
    );
    assert_eq!(classify_bare_token("env://SOME_VAR"), CredentialKind::Bearer);
}

#[test]
fn classify_multi_colon_is_bearer() {
    assert_eq!(classify_bare_token("a:b:c"), CredentialKind::Bearer);
}

#[test]
fn classify_empty_half_is_bearer() {
    assert_eq!(classify_bare_token(":secret"), CredentialKind::Bearer);
    assert_eq!(classify_bare_token("user:"), CredentialKind::Bearer);
}

#[test]
fn classify_whitespace_is_bearer() {
    // Embedded whitespace disqualifies the user:pass shorthand.
    assert_eq!(classify_bare_token("user :pass"), CredentialKind::Bearer);
    assert_eq!(classify_bare_token("a b"), CredentialKind::Bearer);
}
