//! Pagination support: RFC 5988 Link header parser and result concatenation.

use reqwest::header::HeaderMap;

/// Paginator configuration.
#[derive(Debug, Clone)]
pub struct Paginator {
    pub max_pages: usize,
}

impl Default for Paginator {
    fn default() -> Self {
        Self { max_pages: 100 }
    }
}

impl Paginator {
    /// Extract the `rel=next` URL from an HTTP `Link` header.
    pub fn next_url(&self, headers: &HeaderMap) -> Option<String> {
        let link = headers.get("link")?.to_str().ok()?;
        parse_link_header(link)
            .into_iter()
            .find(|(rel, _url)| rel == "next")
            .map(|(_, url)| url)
    }

    /// Concatenate page JSON values into a single value.
    ///
    /// - If every page is an array, all elements are flattened into one array.
    /// - If a page is not an array, it is pushed as a single item.
    pub fn concat_results(&self, pages: Vec<serde_json::Value>) -> serde_json::Value {
        let mut results = Vec::new();
        for page in pages {
            if let Some(arr) = page.as_array() {
                results.extend(arr.iter().cloned());
            } else {
                results.push(page);
            }
        }
        serde_json::Value::Array(results)
    }
}

/// Parse an RFC 5988 `Link` header value into `(rel, url)` tuples.
///
/// Example input:
/// `\u003chttps://api.example.com?page=2\u003e; rel="next", \u003chttps://api.example.com?page=1\u003e; rel="prev"`
fn parse_link_header(link: &str) -> Vec<(String, String)> {
    crate::links::parse_rfc5988(link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_github_link_header() {
        let link = r#"<https://api.github.com/repos?per_page=100&page=2>; rel="next", <https://api.github.com/repos?per_page=100&page=5>; rel="last""#;
        let parsed = parse_link_header(link);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0, "next");
        assert_eq!(
            parsed[0].1,
            "https://api.github.com/repos?per_page=100&page=2"
        );
        assert_eq!(parsed[1].0, "last");
    }

    #[test]
    fn no_next_url() {
        let link = r#"<https://api.github.com/repos?per_page=100&page=1>; rel="first", <https://api.github.com/repos?per_page=100&page=5>; rel="last""#;
        let paginator = Paginator::default();
        let mut headers = HeaderMap::new();
        headers.insert("link", link.parse().unwrap());
        assert!(paginator.next_url(&headers).is_none());
    }

    #[test]
    fn concat_arrays_flattens() {
        let paginator = Paginator::default();
        let pages = vec![serde_json::json!([1, 2]), serde_json::json!([3, 4])];
        let result = paginator.concat_results(pages);
        assert_eq!(result, serde_json::json!([1, 2, 3, 4]));
    }

    #[test]
    fn concat_mixed_wraps_non_array() {
        let paginator = Paginator::default();
        let pages = vec![serde_json::json!([1, 2]), serde_json::json!({"meta": true})];
        let result = paginator.concat_results(pages);
        assert_eq!(result, serde_json::json!([1, 2, {"meta": true}]));
    }
}
