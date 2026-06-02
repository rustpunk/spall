//! Credential classification — the single source of truth for deciding whether
//! a raw, prefix-less auth token is HTTP Basic (`user:pass`) or a Bearer token.
//!
//! Every consumer (the `--spall-auth` CLI override in `spall-cli`, and any
//! future credential path) routes its Basic-vs-Bearer decision through
//! [`classify_bare_token`], so the security-sensitive heuristic lives in exactly
//! one place and cannot drift between call sites.

/// Kind of credential a raw token maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    Bearer,
    Basic,
    ApiKey,
}

/// Classify a prefix-less token as Basic or Bearer.
///
/// Returns [`CredentialKind::Basic`] **only** when `token` is an unambiguous
/// `user:pass`: no ASCII whitespace, exactly one colon, both halves non-empty,
/// and the substring after the colon does not start with `//`. That last clause
/// rejects every `scheme://...` URL (e.g. `https://host`, `keyring://svc`,
/// `env://VAR`), which also contains a single colon but must never be read as
/// credentials. Every other token is [`CredentialKind::Bearer`], the safe
/// default. (`ApiKey` is never inferred from a bare token; callers select it
/// explicitly from config.)
///
/// Callers that handle explicit `Bearer ` / `Basic ` prefixes should strip and
/// resolve those first, then fall through to this classifier for the bare case.
#[must_use]
pub fn classify_bare_token(token: &str) -> CredentialKind {
    if token.contains(|c: char| c.is_ascii_whitespace()) {
        return CredentialKind::Bearer;
    }
    if token.split(':').count() != 2 {
        return CredentialKind::Bearer;
    }
    match token.split_once(':') {
        Some((u, p)) if !u.is_empty() && !p.is_empty() && !p.starts_with("//") => {
            CredentialKind::Basic
        }
        _ => CredentialKind::Bearer,
    }
}
