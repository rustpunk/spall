//! Transport-neutral HTTP status code.
//!
//! Memory model: a single `u16`, `Copy`, never heap-allocates. This module
//! buffers nothing and streams nothing — it is a pure value type.

/// A transport-neutral HTTP status code.
///
/// Why: this crate is transport-agnostic, so it cannot depend on
/// `reqwest::StatusCode`. `Status` is the neutral replacement that preserves
/// exactly the classification the CLI exit-code map relies on (`4xx -> 4`,
/// `5xx -> 5`). It is a thin newtype over `u16` rather than an enum because the
/// HTTP status space is open and numeric: servers may return codes the
/// standard does not enumerate, and we must round-trip them faithfully.
///
/// Memory model: `Copy`, stack-only, one `u16`. Construction and every method
/// are allocation-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Status(pub u16);

impl Status {
    /// Returns the raw numeric status code.
    ///
    /// Why: callers (notably the CLI exit-code classifier) need the bare number
    /// to format or compare. Takes `self` by value because `Status` is `Copy`.
    #[must_use]
    pub fn as_u16(self) -> u16 {
        self.0
    }

    /// Reports whether the code is in the 2xx success range (`200..=299`).
    ///
    /// Why: mirrors `reqwest::StatusCode::is_success` so success detection
    /// survives the transport extraction unchanged. Allocation-free.
    #[must_use]
    pub fn is_success(self) -> bool {
        (200..=299).contains(&self.0)
    }

    /// Reports whether the code is in the 4xx client-error range (`400..=499`).
    ///
    /// Why: drives the CLI exit code `4` for client errors. Allocation-free.
    #[must_use]
    pub fn is_client_error(self) -> bool {
        (400..=499).contains(&self.0)
    }

    /// Reports whether the code is in the 5xx server-error range (`500..=599`).
    ///
    /// Why: drives the CLI exit code `5` for server errors. Allocation-free.
    #[must_use]
    pub fn is_server_error(self) -> bool {
        (500..=599).contains(&self.0)
    }
}

impl From<u16> for Status {
    /// Wraps a raw status number with no validation.
    ///
    /// Why: the HTTP status space is open, so any `u16` is a legal `Status`;
    /// rejecting out-of-range values here would lose information a server
    /// legitimately sent. Allocation-free.
    fn from(code: u16) -> Self {
        Status(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_ranges() {
        assert!(Status(200).is_success());
        assert!(Status(299).is_success());
        assert!(!Status(300).is_success());

        assert!(Status(400).is_client_error());
        assert!(Status(404).is_client_error());
        assert!(Status(499).is_client_error());
        assert!(!Status(399).is_client_error());
        assert!(!Status(500).is_client_error());

        assert!(Status(500).is_server_error());
        assert!(Status(503).is_server_error());
        assert!(Status(599).is_server_error());
        assert!(!Status(499).is_server_error());
    }

    #[test]
    fn round_trips_raw_code() {
        let s = Status::from(418);
        assert_eq!(s.as_u16(), 418);
        // Non-standard codes round-trip faithfully.
        let s = Status::from(599);
        assert_eq!(s.as_u16(), 599);
    }
}
