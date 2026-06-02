//! `spall-openapi`: the transport-agnostic request/response contract for spall.
//!
//! This crate defines the *neutral* boundary between spall's spec/request logic
//! and any concrete HTTP transport. It deliberately depends on **no HTTP
//! client** (no `reqwest`/`ureq`/`hyper`), no async runtime (`tokio`/`futures`),
//! no CLI framework (`clap`), and no C/`*-sys` crates. The CLI plugs a real
//! transport in on top of these types; tests and a mock server can plug in
//! something else, all speaking the same contract.
//!
//! ## Two-layer read contract
//!
//! Reading a response is offered at two layers:
//!
//! * **Layer 1 — [`ResponseStream`] (escape hatch).** One request yields a
//!   status, headers, and a *streaming* body (`Box<dyn Read + Send>`). The
//!   caller owns the body and is never handed a buffered `Vec<u8>`. Use this
//!   for save-to-file or raw-per-page output.
//! * **Layer 2 — [`ItemStream`] (default).** A lazy [`Iterator`] of
//!   `serde_json::Value` items drawn from the array located by a [`DataPath`].
//!   Items are parsed one at a time; the page is never fully buffered.
//!
//! ## Hand-rolled bounded-memory parser
//!
//! Layer 2 is driven by [`JsonSkimmer`], an in-house forward-only streaming
//! pull reader. It navigates to the item array without materializing skipped
//! values and captures exactly one element's bytes at a time before delegating
//! the final parse to `serde_json`. Peak memory is therefore bounded by the
//! largest single element, not by the page size — a property verified by the
//! gating test in `tests/stream_bound.rs`.
//!
//! ## Request side
//!
//! [`HttpRequestSpec`] and [`RequestBody`] describe a single request neutrally;
//! file uploads are *descriptors* (a path, not bytes) so the transport streams
//! them later. [`Status`] is the neutral status newtype that preserves spall's
//! `4xx`/`5xx` exit-code classification without depending on a transport's
//! status type.

pub mod datapath;
pub mod request;
pub mod response;
pub mod status;
pub mod stream;

pub use datapath::{DataPath, DataPathError};
pub use request::{Headers, HttpRequestSpec, MultipartField, MultipartValue, RequestBody};
pub use response::ResponseStream;
pub use status::Status;
pub use stream::{ItemStream, JsonSkimmer, StreamError};
