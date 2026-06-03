//! Native automatic pagination, driven by the response `Link` header.
//!
//! [`Paginator`] computes the next page's URL from a page's headers. It is the
//! one pagination policy spall implements today: RFC 5988 `Link` `rel=next`.
//! Offset/limit, page-number, and body-cursor strategies are intentionally
//! **not** implemented here (they are unimplemented in spall today and out of
//! scope for the extraction).
//!
//! ## Two ways to use it
//!
//! * **Automatic (default).** Hand a [`Paginator`] to
//!   [`crate::stream::ItemStream::paginated`]. The item stream follows
//!   `rel=next` across pages, strips each page's envelope via the
//!   [`crate::datapath::DataPath`], and yields one de-paginated, lazy stream of
//!   items — the next page is fetched only when the current page drains, and no
//!   page is ever buffered whole (replacing the old eager `concat_results`).
//! * **Raw per page (opt-in building block).** A caller that wants raw page
//!   bodies can loop manually with [`Paginator::next_url`] as the only moving
//!   part: fetch a [`crate::response::ResponseStream`], consume its Layer-1 raw
//!   body however it likes, call `next_url(&headers)` on that page's headers,
//!   and fetch again until it returns `None` (or a page budget is hit). This
//!   path does no auto-following and no item-flattening — it is purely the
//!   next-URL oracle.
//!
//! Memory model: [`Paginator`] is a tiny fully-buffered config value (one
//! `usize`); [`Paginator::next_url`] reads only the (small) header map.

use crate::links::parse_rfc5988;
use crate::request::Headers;

/// Automatic-pagination configuration.
///
/// Why: pagination needs a hard ceiling so a misbehaving (or maliciously
/// circular) `rel=next` chain cannot loop forever. `max_pages` is that ceiling;
/// it matches the CLI's historical default of 100.
///
/// Memory model: fully buffered and tiny — a single `usize`.
#[derive(Debug, Clone)]
pub struct Paginator {
    /// Maximum number of pages to fetch before stopping, regardless of whether
    /// a further `rel=next` link exists. Caps unbounded / circular link chains.
    pub max_pages: usize,
}

impl Default for Paginator {
    /// Defaults `max_pages` to 100, matching spall's historical CLI default.
    fn default() -> Self {
        Self { max_pages: 100 }
    }
}

impl Paginator {
    /// Computes the next page's URL from a page's response headers.
    ///
    /// Reads the neutral lowercased `link` header, parses it with the RFC 5988
    /// parser ([`crate::links::parse_rfc5988`]), and returns the target of the
    /// first `rel=next` link. Returns `None` when there is no `link` header or
    /// no `rel=next` within it — that is the signal to stop paginating.
    ///
    /// The next URL always comes from the **headers**, never from the response
    /// body, so an automatic [`crate::stream::ItemStream`] can drive a
    /// forward-only body reader with no rewind.
    ///
    /// This is also the sole building block for the opt-in raw-per-page loop
    /// described on this module: call it on each fetched page's headers to get
    /// the next URL, without any automatic following or item-flattening.
    ///
    /// Memory model: reads only the small header map; allocates at most the
    /// returned `String`.
    #[must_use]
    pub fn next_url(&self, headers: &Headers) -> Option<String> {
        let link = headers.get("link")?;
        parse_rfc5988(link)
            .into_iter()
            .find(|(rel, _target)| rel == "next")
            .map(|(_rel, target)| target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdrs(link: &str) -> Headers {
        let mut h = Headers::new();
        h.insert("link".to_string(), link.to_string());
        h
    }

    #[test]
    fn default_max_pages_is_100() {
        assert_eq!(Paginator::default().max_pages, 100);
    }

    #[test]
    fn next_url_finds_rel_next() {
        let p = Paginator::default();
        let h = hdrs(
            r#"<https://api.github.com/repos?page=2>; rel="next", <https://api.github.com/repos?page=5>; rel="last""#,
        );
        assert_eq!(
            p.next_url(&h).as_deref(),
            Some("https://api.github.com/repos?page=2")
        );
    }

    #[test]
    fn next_url_none_without_next() {
        let p = Paginator::default();
        let h = hdrs(r#"<https://api/x?page=1>; rel="first", <https://api/x?page=5>; rel="last""#);
        assert!(p.next_url(&h).is_none());
    }

    #[test]
    fn next_url_none_without_link_header() {
        let p = Paginator::default();
        assert!(p.next_url(&Headers::new()).is_none());
    }
}
