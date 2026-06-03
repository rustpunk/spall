//! Layer 1 of the read contract: the low-level streaming response.
//!
//! This module defines the escape hatch: one request maps to a streaming body
//! the caller owns. It deliberately never buffers the body into a `Vec<u8>`.

use crate::request::Headers;
use crate::status::Status;

/// A streaming HTTP response — Layer 1 of the two-layer read contract.
///
/// Why: this is the low-level escape hatch. The status and headers are small
/// and fully materialized, but the **body is a streaming `Read`**, not a
/// buffered `Vec<u8>`. This lets callers that need raw bytes (save-to-file,
/// raw-per-page output) consume an arbitrarily large response without ever
/// holding it all in memory, and it is the source the Layer-2 item iterator
/// reads incrementally.
///
/// Memory model: **streams**. The body is owned by the caller as a boxed
/// `Read`; nothing in this struct buffers the response payload. Only the status
/// (one `u16`) and headers (a small map) are resident.
pub struct ResponseStream {
    /// The response status code (see [`Status`]).
    pub status: Status,
    /// The response headers with lowercased names (see [`Headers`]).
    pub headers: Headers,
    /// The streaming response body. The caller owns this `Read` and pulls bytes
    /// on demand; it is `Send` so a transport may produce it on another thread.
    /// It is never a buffered `Vec<u8>` — that is the whole point of Layer 1.
    pub body: Box<dyn std::io::Read + Send>,
}
