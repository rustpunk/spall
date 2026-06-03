//! Neutral RFC 5988 `Link` header parsing.
//!
//! This is the transport-neutral home of the RFC 5988 parser. It works on a
//! plain `&str` Link-header value and returns `(rel, target)` pairs in document
//! order; it depends on no HTTP client. [`crate::paginate::Paginator`] uses it
//! to find the `rel=next` target that drives automatic pagination.
//!
//! Scope: this module covers **only** the `Link` *header* grammar. Body-based
//! link discovery (HAL `_links`, JSON:API `links`, Siren) is a distinct
//! CLI-side feature (`--spall-follow`) and deliberately lives in `spall-cli`,
//! not here.
//!
//! Memory model: **fully buffered**, but only over the (small) header string —
//! no response body is ever read here.

/// Parses an RFC 5988 `Link` header value into `(rel, target)` pairs.
///
/// Each comma-separated link is split on `;` into a `<target>` segment and
/// `rel=...` parameter; the angle brackets and any surrounding quotes on the
/// `rel` value are stripped. Links missing either a target or a `rel` are
/// skipped. Pairs are returned in the order they appear, so the first
/// `rel="next"` wins for [`crate::paginate::Paginator::next_url`].
///
/// Example input:
/// `<https://api.example.com?page=2>; rel="next", <https://api.example.com?page=1>; rel="prev"`
///
/// Memory model: fully buffered over the header string; allocates one `String`
/// per emitted `rel`/`target`. No I/O, no body access.
#[must_use]
pub fn parse_rfc5988(link: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for part in link.split(',') {
        let mut url = None;
        let mut rel = None;
        for segment in part.split(';') {
            let seg = segment.trim();
            if seg.starts_with('<') && seg.ends_with('>') {
                url = Some(seg[1..seg.len() - 1].to_string());
            } else if let Some(rest) = seg.strip_prefix("rel=") {
                rel = Some(rest.trim().trim_matches('"').to_string());
            }
        }
        if let (Some(u), Some(r)) = (url, rel) {
            out.push((r, u));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc5988_parses_github_style_link_header() {
        let l = parse_rfc5988(
            r#"<https://api.github.com/repos?per_page=100&page=2>; rel="next", <https://api.github.com/repos?per_page=100&page=5>; rel="last""#,
        );
        assert_eq!(l.len(), 2);
        assert_eq!(
            l[0],
            (
                "next".to_string(),
                "https://api.github.com/repos?per_page=100&page=2".to_string()
            )
        );
        assert_eq!(l[1].0, "last");
        assert_eq!(
            l[1].1,
            "https://api.github.com/repos?per_page=100&page=5".to_string()
        );
    }

    #[test]
    fn no_rel_or_no_target_is_skipped() {
        // A segment with a target but no rel, and one with rel but no target.
        let l = parse_rfc5988(r#"<https://api/x?p=2>, rel="next""#);
        assert!(l.is_empty());
    }

    #[test]
    fn unquoted_rel_is_accepted() {
        let l = parse_rfc5988("<https://api/x?p=2>; rel=next");
        assert_eq!(
            l,
            vec![("next".to_string(), "https://api/x?p=2".to_string())]
        );
    }

    #[test]
    fn empty_header_yields_nothing() {
        assert!(parse_rfc5988("").is_empty());
    }
}
