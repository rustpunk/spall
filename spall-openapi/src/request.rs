//! Transport-neutral request contract.
//!
//! These types are a *descriptor* of a single HTTP request — the data the
//! request builder (#25) produces and the CLI transport later turns into a real
//! `reqwest` request. Nothing here performs I/O, opens files, or streams; every
//! type is fully buffered in memory because a request spec is small by
//! construction (headers, query pairs, and either a JSON value, a form, a
//! bounded byte body, or *descriptors* of multipart parts — file parts hold a
//! path, not the file's contents).

use serde_json::Value;
use std::path::PathBuf;

/// Lowercased HTTP header names mapped to their values.
///
/// Why a `BTreeMap` with lowercased keys: RFC 9110 defines header field names
/// as case-insensitive, so we canonicalize to lowercase to deduplicate and
/// compare reliably; `BTreeMap` gives deterministic ordering for reproducible
/// output and tests. Callers are responsible for lowercasing names on insert.
///
/// Memory model: fully buffered; header sets are small.
pub type Headers = std::collections::BTreeMap<String, String>;

/// A fully-resolved, transport-neutral description of one HTTP request.
///
/// Why: this is the contract the request builder (#25) populates and the CLI
/// transport consumes, decoupling spec-to-request logic from any specific HTTP
/// client. The URL is already absolute with path parameters substituted and
/// carries no query string — query parameters live in `query` so the transport
/// owns percent-encoding policy.
///
/// Memory model: fully buffered. All fields are owned and in-memory; this is a
/// small value object, never a stream. A `RequestBody::Multipart` file part is
/// only a *path descriptor*, so even large uploads do not inflate this struct.
#[derive(Debug, Clone)]
pub struct HttpRequestSpec {
    /// The HTTP method, reused from the spec IR so callers share one method
    /// enum across the spec and request layers.
    pub method: spall_core::ir::HttpMethod,
    /// Absolute request URL with path parameters already substituted and **no**
    /// query string appended (query lives in [`HttpRequestSpec::query`]).
    pub url: String,
    /// Query parameters as ordered `(name, value)` pairs. Order is preserved so
    /// repeated keys and array-style parameters round-trip; encoding is the
    /// transport's concern.
    pub query: Vec<(String, String)>,
    /// Request headers with lowercased names (see [`Headers`]).
    pub headers: Headers,
    /// Cookie `(name, value)` pairs, kept separate from headers so the
    /// transport can assemble a single `cookie` header per its own policy.
    pub cookies: Vec<(String, String)>,
    /// The optional request body. `None` means a bodyless request (e.g. `GET`).
    pub body: Option<RequestBody>,
}

/// The body of an [`HttpRequestSpec`], as a transport-neutral descriptor.
///
/// Why an enum: spall must support JSON, URL-encoded forms, raw bytes, and
/// multipart uploads, and the transport needs to know which so it can set the
/// right `Content-Type` and serialization. Variants exist for the full contract
/// even though #24 itself does not construct all of them — #25 populates them.
///
/// Memory model: `Json`, `Form`, and `Bytes` are fully buffered in memory.
/// `Multipart` holds only field *descriptors*; a file field carries a
/// [`PathBuf`], not the file's bytes, so the CLI can stream the file later
/// without this enum ever buffering it.
#[derive(Debug, Clone)]
pub enum RequestBody {
    /// A JSON body, serialized by the transport. Fully buffered in memory.
    Json(Value),
    /// A URL-encoded form body as ordered `(name, value)` pairs. Buffered.
    Form(Vec<(String, String)>),
    /// A raw byte body with an explicit content type. Buffered in memory; size
    /// is the caller's concern (the byte-cap guard arrives in #44).
    Bytes {
        /// The MIME type to send as `Content-Type`.
        content_type: String,
        /// The raw body bytes.
        data: Vec<u8>,
    },
    /// A `multipart/form-data` body described as a list of fields. Each field is
    /// a neutral descriptor; file fields reference a path rather than buffering
    /// contents, so the CLI streams files when it builds the real request.
    Multipart(Vec<MultipartField>),
}

/// One named part of a [`RequestBody::Multipart`] body.
///
/// Why: multipart bodies are an ordered sequence of named parts; this pairs the
/// field name with its neutral value descriptor. Memory model follows
/// [`MultipartValue`].
#[derive(Debug, Clone)]
pub struct MultipartField {
    /// The form field name for this part.
    pub name: String,
    /// The part's value descriptor.
    pub value: MultipartValue,
}

/// The value of a [`MultipartField`] — a neutral descriptor, never a live
/// stream.
///
/// Why: spall describes multipart parts here and lets the CLI turn them into a
/// real streaming `reqwest::multipart::Part` later. Keeping this a pure
/// descriptor means the contract crate never touches the filesystem or holds
/// large uploads in memory.
///
/// Memory model: `Text` and `Bytes` are buffered (text parts are small; byte
/// parts size is the caller's concern). `File` holds only a path plus optional
/// metadata — the file's contents are **not** read here; the CLI streams the
/// file when it builds the transport request.
#[derive(Debug, Clone)]
pub enum MultipartValue {
    /// A textual part value, buffered in memory.
    Text(String),
    /// A file part described by its path. The file is **not** opened or read by
    /// this crate; the CLI streams it later. `filename` and `content_type`
    /// override what the transport would otherwise infer from the path.
    File {
        /// Path to the file on disk, read later by the transport.
        path: PathBuf,
        /// Optional override for the transmitted filename.
        filename: Option<String>,
        /// Optional override for the part's `Content-Type`.
        content_type: Option<String>,
    },
    /// An in-memory byte part with an explicit filename and content type.
    /// Buffered; size is the caller's concern.
    Bytes {
        /// The transmitted filename for this part.
        filename: String,
        /// The part's `Content-Type`.
        content_type: String,
        /// The raw part bytes.
        data: Vec<u8>,
    },
}
