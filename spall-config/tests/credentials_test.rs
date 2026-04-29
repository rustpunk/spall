use spall_config::auth::default_token_env;
use spall_config::credentials::CredentialResolver;

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
