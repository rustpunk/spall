use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use secrecy::{ExposeSecret, SecretString};

/// Inject an OAuth2 access token as `Authorization: Bearer <token>`.
pub fn apply(token: &SecretString, headers: &mut HeaderMap) {
    let value = format!("Bearer {}", token.expose_secret());
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("invalid")),
    );
}

/// Perform a lightweight client-credentials token fetch.
#[allow(dead_code)]
pub async fn fetch_client_credentials(
    token_url: &str,
    client_id: &str,
    client_secret: Option<&str>,
) -> Result<SecretString, crate::SpallCliError> {
    let mut params = vec![("grant_type", "client_credentials"), ("client_id", client_id)];
    if let Some(secret) = client_secret {
        params.push(("client_secret", secret));
    }

    let body = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&");

    let client = crate::http::build_fetch_client(crate::http::resolve_env_proxy().as_deref())
        .map_err(|e| crate::SpallCliError::Network(e.to_string()))?;
    let resp = client
        .post(token_url)
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| crate::SpallCliError::Network(format!("OAuth2 token request failed: {}", e)))?;

    if !resp.status().is_success() {
        return Err(crate::SpallCliError::Network(format!(
            "OAuth2 token endpoint returned {}",
            resp.status()
        )));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| {
        crate::SpallCliError::Network(format!("Failed to parse OAuth2 token response: {}", e))
    })?;

    let token = json["access_token"]
        .as_str()
        .ok_or_else(|| crate::SpallCliError::Usage("OAuth2 response missing access_token".to_string()))?;

    Ok(SecretString::new(token.to_string().into()))
}
