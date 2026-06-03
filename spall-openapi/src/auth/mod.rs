//! Transport-neutral auth *request contributors*.
//!
//! Each contributor takes an **already-resolved** secret
//! ([`secrecy::SecretString`]) and mutates an
//! [`HttpRequestSpec`](crate::request::HttpRequestSpec) in place — injecting an
//! `Authorization` header, an API-key header/query pair, or building the OAuth2
//! client-credentials token-request spec the caller then executes. Nothing here
//! performs I/O, builds an HTTP client, opens a keyring, or prompts; it is the
//! neutral header/query/body half of spall's auth.
//!
//! ## What stays in the CLI
//!
//! The rich half of auth lives in `spall-cli` and is intentionally **not**
//! mirrored here:
//!
//! * The resolution chain (CLI override → inline TOML token → `keyring://` /
//!   `env://` via `hasp` → `password_env` → `token_env` → `SPALL_<API>_TOKEN`
//!   → interactive password prompt) that decides *which* secret to use.
//! * The OAuth2 Authorization-Code + PKCE login flow, on-disk token cache,
//!   transparent refresh, and the `spall_config` types (`ResolvedAuth`,
//!   `AuthConfig`) that drive resolution.
//!
//! The CLI resolves a secret through that chain, then calls one of the
//! contributors below at the request-construction boundary. This module mirrors
//! the CLI's `auth/` file layout (one file per scheme) to ease the #28
//! delegation, where the CLI's `apply` will route to these functions instead of
//! the reqwest-specific ones it carries today.
//!
//! ## Header model
//!
//! [`Headers`](crate::request::Headers) is a lowercased
//! `BTreeMap<String, String>`, so unlike the CLI's `reqwest::HeaderMap` there is
//! no `HeaderName`/`HeaderValue` validation step:
//! any name or value is storable verbatim. The CLI's validation fallbacks
//! (`x-api-key` / `invalid`) are a reqwest-construction concern and move to the
//! transport boundary in #28 (see [`api_key`]).
//!
//! ## Security
//!
//! Secrets remain inside `SecretString`; `expose_secret()` is called only at
//! the header / query / body construction boundary (each such line carries a
//! `// SECURITY:` comment). No secret is placed into any IR or persisted type,
//! and nothing here derives a `Debug`/`Display` that prints a secret.

pub mod apikey;
pub mod basic;
pub mod bearer;
pub mod oauth2;

pub use apikey::{ApiKeyLocation, api_key};
pub use basic::basic;
pub use bearer::bearer;
pub use oauth2::{oauth2_access_token, oauth2_client_credentials_request};

#[cfg(test)]
mod test_support {
    use crate::request::HttpRequestSpec;

    /// A bodyless, header-empty request spec the per-scheme tests mutate.
    pub(crate) fn empty_spec() -> HttpRequestSpec {
        HttpRequestSpec {
            method: spall_core::ir::HttpMethod::Get,
            url: "https://api.example.com/v1/thing".to_string(),
            query: Vec::new(),
            headers: crate::request::Headers::new(),
            cookies: Vec::new(),
            body: None,
        }
    }
}
