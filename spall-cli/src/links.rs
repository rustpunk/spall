//! Hypermedia link extraction from API responses.
//!
//! Supports four common formats:
//!
//! - **RFC 5988 `Link` header** — `<https://api/x?page=2>; rel="next"`
//! - **HAL** — `{"_links": {"next": {"href": "..."}}}`
//! - **JSON:API** — `{"links": {"next": "..."}}` (string or `{href}`-object)
//! - **Siren** — `{"links": [{"rel": ["next"], "href": "..."}]}`
//!
//! The JSON:API and Siren conventions both use a bare `links` key but are
//! discriminated by shape (object vs. array), so a single response may be
//! parsed by either without ambiguity.

use reqwest::header::HeaderMap;
use serde_json::Value;
use std::collections::BTreeMap;

/// A single hypermedia link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub rel: String,
    pub href: String,
}

/// Links extracted from a response's headers and body, grouped by `rel`.
///
/// Multiple links may share a `rel`; lookups return the first occurrence.
#[derive(Debug, Default, Clone)]
pub struct Links {
    by_rel: BTreeMap<String, Vec<Link>>,
}

impl Links {
    /// Build a `Links` from response headers and an optional JSON body.
    #[must_use]
    pub fn from_response(headers: &HeaderMap, body: Option<&Value>) -> Self {
        let mut links = Self::default();
        links.absorb_link_header(headers);
        if let Some(v) = body {
            links.absorb_hal(v);
            links.absorb_jsonapi_or_siren(v);
        }
        links
    }

    /// Return the first link with the given `rel`, if any.
    #[must_use]
    pub fn rel(&self, name: &str) -> Option<&Link> {
        self.by_rel.get(name).and_then(|v| v.first())
    }

    /// Iterate over all `rel` names that were discovered.
    #[allow(dead_code)] // Exposed for future REPL `:links` introspection.
    pub fn rels(&self) -> impl Iterator<Item = &str> {
        self.by_rel.keys().map(String::as_str)
    }

    /// Total number of links across all rels (test/debug helper).
    #[cfg(test)]
    fn len(&self) -> usize {
        self.by_rel.values().map(Vec::len).sum()
    }

    /// True when no links were discovered.
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.by_rel.is_empty()
    }

    fn push(&mut self, link: Link) {
        self.by_rel
            .entry(link.rel.clone())
            .or_default()
            .push(link);
    }

    fn absorb_link_header(&mut self, headers: &HeaderMap) {
        if let Some(h) = headers.get("link").and_then(|v| v.to_str().ok()) {
            for (rel, href) in parse_rfc5988(h) {
                self.push(Link { rel, href });
            }
        }
    }

    fn absorb_hal(&mut self, body: &Value) {
        let Some(obj) = body.get("_links").and_then(Value::as_object) else {
            return;
        };
        for (rel, val) in obj {
            match val {
                Value::Object(o) => {
                    if let Some(href) = o.get("href").and_then(Value::as_str) {
                        self.push(Link {
                            rel: rel.clone(),
                            href: href.to_string(),
                        });
                    }
                }
                Value::Array(arr) => {
                    for item in arr {
                        if let Some(href) = item.get("href").and_then(Value::as_str) {
                            self.push(Link {
                                rel: rel.clone(),
                                href: href.to_string(),
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn absorb_jsonapi_or_siren(&mut self, body: &Value) {
        let Some(links) = body.get("links") else {
            return;
        };
        match links {
            // JSON:API: object keyed by rel.
            Value::Object(obj) => {
                for (rel, val) in obj {
                    let href = match val {
                        Value::String(s) => Some(s.clone()),
                        Value::Object(o) => o.get("href").and_then(Value::as_str).map(String::from),
                        _ => None,
                    };
                    if let Some(h) = href {
                        self.push(Link {
                            rel: rel.clone(),
                            href: h,
                        });
                    }
                }
            }
            // Siren: array of {rel: [...], href: ...}.
            Value::Array(arr) => {
                for entry in arr {
                    let href = entry.get("href").and_then(Value::as_str);
                    let rels = entry.get("rel").and_then(Value::as_array);
                    if let (Some(h), Some(rels)) = (href, rels) {
                        for r in rels {
                            if let Some(rel) = r.as_str() {
                                self.push(Link {
                                    rel: rel.to_string(),
                                    href: h.to_string(),
                                });
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Parse an RFC 5988 `Link` header value into `(rel, href)` pairs.
///
/// Example: `<https://api.example.com?page=2>; rel="next", <…>; rel="prev"`.
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
    use serde_json::json;

    fn hdrs(link: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("link", link.parse().unwrap());
        h
    }

    #[test]
    fn rfc5988_parses_github_style_link_header() {
        let l = parse_rfc5988(
            r#"<https://api.github.com/x?page=2>; rel="next", <https://api.github.com/x?page=5>; rel="last""#,
        );
        assert_eq!(l.len(), 2);
        assert_eq!(l[0], ("next".to_string(), "https://api.github.com/x?page=2".to_string()));
        assert_eq!(l[1].0, "last");
    }

    #[test]
    fn from_response_picks_up_link_header_only() {
        let headers = hdrs(r#"<https://api/x?p=2>; rel="next""#);
        let links = Links::from_response(&headers, None);
        assert_eq!(links.rel("next").unwrap().href, "https://api/x?p=2");
        assert!(links.rel("prev").is_none());
    }

    #[test]
    fn hal_links_object_form() {
        let body = json!({
            "_links": {
                "self": {"href": "/users/1"},
                "next": {"href": "/users?page=2"}
            },
            "name": "alice"
        });
        let links = Links::from_response(&HeaderMap::new(), Some(&body));
        assert_eq!(links.rel("self").unwrap().href, "/users/1");
        assert_eq!(links.rel("next").unwrap().href, "/users?page=2");
    }

    #[test]
    fn hal_links_array_form() {
        let body = json!({
            "_links": {
                "item": [
                    {"href": "/items/1"},
                    {"href": "/items/2"}
                ]
            }
        });
        let links = Links::from_response(&HeaderMap::new(), Some(&body));
        // First occurrence wins for rel("item"); two total entries.
        assert_eq!(links.rel("item").unwrap().href, "/items/1");
        assert_eq!(links.len(), 2);
    }

    #[test]
    fn jsonapi_links_string_and_object_forms() {
        let body = json!({
            "data": [],
            "links": {
                "self": "/articles",
                "next": {"href": "/articles?page[number]=2"}
            }
        });
        let links = Links::from_response(&HeaderMap::new(), Some(&body));
        assert_eq!(links.rel("self").unwrap().href, "/articles");
        assert_eq!(links.rel("next").unwrap().href, "/articles?page[number]=2");
    }

    #[test]
    fn siren_links_array_form_with_multiple_rels() {
        let body = json!({
            "class": ["order"],
            "links": [
                {"rel": ["self"], "href": "/orders/1"},
                {"rel": ["next", "fwd"], "href": "/orders/2"}
            ]
        });
        let links = Links::from_response(&HeaderMap::new(), Some(&body));
        assert_eq!(links.rel("self").unwrap().href, "/orders/1");
        assert_eq!(links.rel("next").unwrap().href, "/orders/2");
        assert_eq!(links.rel("fwd").unwrap().href, "/orders/2");
    }

    #[test]
    fn header_and_body_links_coexist() {
        let headers = hdrs(r#"<https://api/x?p=2>; rel="next""#);
        let body = json!({
            "_links": {"self": {"href": "/x"}}
        });
        let links = Links::from_response(&headers, Some(&body));
        assert_eq!(links.rel("next").unwrap().href, "https://api/x?p=2");
        assert_eq!(links.rel("self").unwrap().href, "/x");
    }

    #[test]
    fn empty_when_nothing_present() {
        let links = Links::from_response(&HeaderMap::new(), Some(&json!({"data": 1})));
        assert!(links.is_empty());
        assert_eq!(links.len(), 0);
    }
}
