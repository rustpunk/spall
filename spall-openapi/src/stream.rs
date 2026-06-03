//! Layer 2 of the read contract: the hand-rolled bounded-memory item iterator.
//!
//! This module is the load-bearing streaming core. It contains:
//!
//! * [`JsonSkimmer`] — a forward-only pull reader over a [`BufReader`] with a
//!   one-byte peek. It navigates a JSON document to the item array (without
//!   materializing skipped values) and then captures array elements one at a
//!   time.
//! * [`ItemStream`] — an [`Iterator`] of `serde_json::Value` that drives a
//!   `JsonSkimmer` over a single response page.
//!
//! ## Why hand-rolled
//!
//! A streaming parser we own lets the byte-cap guard (#44) live exactly where
//! element bytes are captured. We never depend on a third-party streaming
//! parser, and the final value parse still delegates to battle-tested
//! `serde_json`.
//!
//! ## Memory model (the load-bearing property)
//!
//! Navigation **streams**: skipped object members are walked structurally by
//! counting braces/brackets and are never materialized. Element capture buffers
//! **exactly one element's bytes** at a time, then parses that slice. Peak heap
//! is therefore bounded by the largest single element, *not* by the page size —
//! a multi-hundred-megabyte page of small elements drains in a few kilobytes of
//! resident buffer. This is verified by `tests/stream_bound.rs`.

use crate::datapath::DataPath;
use crate::paginate::Paginator;
use crate::request::Headers;
use crate::response::ResponseStream;
use serde_json::Value;
use std::io::{BufReader, Read};
use thiserror::Error;

/// Maximum JSON nesting depth honored while skipping or capturing a value.
///
/// Why: an attacker can send `[[[[...` to blow the stack or force unbounded
/// bookkeeping. We cap nesting and return [`StreamError::DepthExceeded`] rather
/// than recursing. 128 is generous for real API payloads while still rejecting
/// pathological nesting bombs.
const MAX_NESTING_DEPTH: usize = 128;

/// An error produced while streaming items out of a response body.
///
/// Why: the skimmer must never panic on malformed input (untrusted server
/// data), so every failure surfaces here. The serde_json variant carries only
/// the formatted message, never the original error object, to avoid leaking
/// internal types across the crate boundary.
#[derive(Debug, Error)]
pub enum StreamError {
    /// The body did not begin with the JSON structure the data path required
    /// (e.g. expected `[` or `{` and found something else).
    #[error("response body is not the expected JSON shape")]
    NotJson,
    /// The data path did not resolve to an array: a key was missing, or the
    /// value at the path was not a JSON array. The string names the offending
    /// path/key for diagnostics.
    #[error("data path did not resolve to a JSON array: {0}")]
    PathNotArray(String),
    /// A value exceeded the internal maximum nesting depth (128) while being
    /// skipped or captured. The cap prevents stack/bookkeeping blowups from
    /// pathologically nested input.
    #[error("JSON nesting depth exceeded the maximum of {MAX_NESTING_DEPTH}")]
    DepthExceeded,
    /// An underlying I/O error reading from the body.
    #[error("I/O error reading response body")]
    Io(#[from] std::io::Error),
    /// `serde_json` failed to parse a captured element. The string is the
    /// formatted serde_json message (no internal types leak out).
    #[error("failed to parse JSON element: {0}")]
    Json(String),
}

/// The shape of a [`DataPath::TopLevel`] document, as resolved by
/// [`JsonSkimmer::seek_top_level_lenient`].
///
/// Why: the default top-level data path reproduces the old `concat_results`
/// leniency — an array root streams element-by-element, while a non-array root
/// is yielded as one whole item. This enum carries that decision back to the
/// [`ItemStream`] state machine so each case drives the right path.
///
/// Memory model: [`TopLevelShape::Array`] is a zero-size marker (the array
/// streams). [`TopLevelShape::Single`] **buffers** exactly one value — the same
/// buffering the old eager concatenation did for a non-array page.
pub enum TopLevelShape {
    /// The root is an array; the reader is positioned just after its `[` and
    /// elements stream via [`JsonSkimmer::next_element`].
    Array,
    /// The root is a non-array value, captured whole as a single item.
    Single(Value),
}

/// A forward-only streaming JSON pull reader.
///
/// Why: this is the in-house parser core. It reads bytes from a `Box<dyn Read>`
/// through a `BufReader` with a single byte of pushback, navigates to the item
/// array described by a [`DataPath`], and then yields the bytes of one array
/// element at a time. It never seeks backwards and never materializes a skipped
/// value.
///
/// Memory model: **streams**. The only meaningful allocation is the
/// per-element capture buffer in [`JsonSkimmer::next_element`], which holds at
/// most one element's bytes and is reused across calls. Navigation allocates
/// nothing beyond transient key strings.
pub struct JsonSkimmer<R: Read> {
    reader: BufReader<R>,
    /// One byte of pushback: `Some(b)` means `b` has been peeked but not
    /// consumed. This gives the single-byte lookahead the grammar needs.
    peeked: Option<u8>,
    /// Reused element-capture buffer, so repeated elements do not re-allocate.
    scratch: Vec<u8>,
}

impl<R: Read> JsonSkimmer<R> {
    /// Wraps a reader in a new skimmer positioned at the start of the document.
    ///
    /// Why: callers hand us the raw streaming body; we add buffering and the
    /// one-byte peek the grammar walk needs. Allocates only the `BufReader`'s
    /// internal buffer; reads nothing yet.
    #[must_use]
    pub fn new(reader: R) -> Self {
        JsonSkimmer {
            reader: BufReader::new(reader),
            peeked: None,
            scratch: Vec::new(),
        }
    }

    /// Reads one byte, honoring any pushed-back peek. Returns `Ok(None)` at EOF.
    fn read_byte(&mut self) -> Result<Option<u8>, StreamError> {
        if let Some(b) = self.peeked.take() {
            return Ok(Some(b));
        }
        let mut buf = [0u8; 1];
        match self.reader.read(&mut buf) {
            Ok(0) => Ok(None),
            Ok(_) => Ok(Some(buf[0])),
            Err(e) => Err(StreamError::Io(e)),
        }
    }

    /// Peeks the next byte without consuming it. Returns `Ok(None)` at EOF.
    fn peek_byte(&mut self) -> Result<Option<u8>, StreamError> {
        if self.peeked.is_none() {
            self.peeked = self.read_byte()?;
        }
        Ok(self.peeked)
    }

    /// Consumes any run of JSON insignificant whitespace.
    fn skip_ws(&mut self) -> Result<(), StreamError> {
        loop {
            match self.peek_byte()? {
                Some(b' ' | b'\t' | b'\n' | b'\r') => {
                    let _ = self.read_byte()?;
                }
                _ => return Ok(()),
            }
        }
    }

    /// Navigates from the document start to *inside* the item array named by
    /// `path`, leaving the reader positioned just after the opening `[`.
    ///
    /// Why: this is how Layer 2 finds the array regardless of where the API
    /// nests it, without buffering the document. For [`DataPath::TopLevel`] it
    /// skips whitespace and expects `[`. For [`DataPath::Pointer`] it expects
    /// `{`, then walks members: it reads each key string, expects `:`, and if
    /// the key equals the current segment it descends (the next segment expects
    /// another `{`, the final segment expects `[`); otherwise it structurally
    /// **skips** the member's value (depth-counting `{}`/`[]` and honoring
    /// string/escape state, never materializing it) and continues past `,`/`}`.
    ///
    /// # Errors
    /// * [`StreamError::NotJson`] if the opening token is not the `[`/`{` the
    ///   grammar requires.
    /// * [`StreamError::PathNotArray`] if a key is missing or the value at the
    ///   path is not an array.
    /// * [`StreamError::DepthExceeded`], [`StreamError::Io`] from skipping.
    ///
    /// Memory model: streams; allocates only transient key strings while
    /// matching, and never the skipped values.
    #[must_use = "navigation can fail and the Result must be handled"]
    pub fn seek_to_data_path(&mut self, path: &DataPath) -> Result<(), StreamError> {
        match path {
            DataPath::TopLevel => {
                self.skip_ws()?;
                match self.read_byte()? {
                    Some(b'[') => Ok(()),
                    Some(_) => Err(StreamError::PathNotArray("<top-level>".to_string())),
                    None => Err(StreamError::NotJson),
                }
            }
            DataPath::Pointer(segments) => self.seek_pointer(segments),
        }
    }

    /// Navigates a [`DataPath::TopLevel`] document with `concat_results`
    /// leniency: if the root value is an array, positions the reader just after
    /// the opening `[` and returns [`TopLevelShape::Array`]; if the root is any
    /// non-array JSON value, captures that **whole** value and returns
    /// [`TopLevelShape::Single`] holding it.
    ///
    /// Why: the old eager `concat_results` flattened array pages but wrapped a
    /// non-array page (e.g. a bare object envelope) as a single element. The
    /// de-paginated [`ItemStream`] must reproduce that exactly for the default
    /// top-level data path. Pointer paths stay strict (a non-array there is a
    /// configuration error), so this leniency lives only here.
    ///
    /// # Errors
    /// * [`StreamError::NotJson`] if the body is empty / not JSON.
    /// * [`StreamError::DepthExceeded`], [`StreamError::Io`],
    ///   [`StreamError::Json`] from capturing a non-array root.
    ///
    /// Memory model: the array case **streams** (nothing captured yet). The
    /// single-value case is **inherently buffered** for that one value — the
    /// same buffering `concat_results` performed when it wrapped a non-array
    /// page. #44 adds the `max_buffered_bytes` guard around this capture.
    #[must_use = "navigation can fail and the Result must be handled"]
    pub fn seek_top_level_lenient(&mut self) -> Result<TopLevelShape, StreamError> {
        self.skip_ws()?;
        match self.peek_byte()? {
            Some(b'[') => {
                // Consume the opening bracket; elements stream from here.
                let _ = self.read_byte()?;
                Ok(TopLevelShape::Array)
            }
            None => Err(StreamError::NotJson),
            Some(_) => {
                // A non-array root: capture the whole value as a single item.
                self.scratch.clear();
                self.capture_value()?;
                let value = serde_json::from_slice::<Value>(&self.scratch)
                    .map_err(|e| StreamError::Json(e.to_string()))?;
                Ok(TopLevelShape::Single(value))
            }
        }
    }

    /// Walks the object members for each pointer segment, descending into the
    /// matching key and structurally skipping every non-matching value.
    fn seek_pointer(&mut self, segments: &[String]) -> Result<(), StreamError> {
        for (idx, segment) in segments.iter().enumerate() {
            let is_final = idx + 1 == segments.len();
            // At the start of each segment we must be at an object.
            self.skip_ws()?;
            match self.read_byte()? {
                Some(b'{') => {}
                Some(_) | None => {
                    return Err(StreamError::PathNotArray(segment.clone()));
                }
            }

            // Walk members until we find `segment` or exhaust the object.
            let found = self.find_member(segment)?;
            if !found {
                return Err(StreamError::PathNotArray(segment.clone()));
            }

            // We are positioned right after the matching key's `:`.
            self.skip_ws()?;
            if is_final {
                // The final segment's value must be an array; consume its `[`.
                match self.read_byte()? {
                    Some(b'[') => return Ok(()),
                    Some(_) | None => {
                        return Err(StreamError::PathNotArray(segment.clone()));
                    }
                }
            }
            // Non-final: the value must be an object; loop will consume its `{`
            // via the peek below, so push it back for the next iteration.
            match self.peek_byte()? {
                Some(b'{') => { /* next iteration consumes it */ }
                Some(_) | None => {
                    return Err(StreamError::PathNotArray(segment.clone()));
                }
            }
        }
        // An empty segment list is logically top-level; treated as not-array
        // here because Pointer is constructed non-empty by from_pointer.
        Err(StreamError::PathNotArray("<empty pointer>".to_string()))
    }

    /// Scans the current object's members looking for `target`. On a match,
    /// leaves the reader just after that key's `:` and returns `true`. On a
    /// non-match, structurally skips the value and continues. Returns `false`
    /// if the object closes without the key.
    ///
    /// Precondition: the opening `{` has already been consumed.
    fn find_member(&mut self, target: &str) -> Result<bool, StreamError> {
        loop {
            self.skip_ws()?;
            match self.peek_byte()? {
                // Empty object or end of members.
                Some(b'}') => {
                    let _ = self.read_byte()?;
                    return Ok(false);
                }
                Some(b'"') => {}
                _ => return Err(StreamError::PathNotArray(target.to_string())),
            }

            let key = self.read_string()?;
            self.skip_ws()?;
            match self.read_byte()? {
                Some(b':') => {}
                _ => return Err(StreamError::PathNotArray(target.to_string())),
            }

            if key == target {
                return Ok(true);
            }

            // Not our key: structurally skip the whole value.
            self.skip_value()?;
            // Then consume the separator: ',' continues, '}' ends the object.
            self.skip_ws()?;
            match self.read_byte()? {
                Some(b',') => continue,
                Some(b'}') => return Ok(false),
                _ => return Err(StreamError::PathNotArray(target.to_string())),
            }
        }
    }

    /// Reads a complete JSON string token (opening quote already peeked, not
    /// consumed) and returns its decoded-as-bytes-UTF8 contents. Only the
    /// escapes needed to find the closing quote are interpreted; the returned
    /// string keeps escape sequences literally except `\"` and `\\`, which is
    /// sufficient for key comparison against unescaped config segments.
    ///
    /// For robust key matching we decode the standard JSON string escapes.
    fn read_string(&mut self) -> Result<String, StreamError> {
        // Consume the opening quote.
        match self.read_byte()? {
            Some(b'"') => {}
            _ => return Err(StreamError::NotJson),
        }
        let mut raw: Vec<u8> = Vec::new();
        // Re-wrap as a JSON string literal so serde_json decodes escapes/UTF-8.
        raw.push(b'"');
        loop {
            match self.read_byte()? {
                None => return Err(StreamError::NotJson),
                Some(b'\\') => {
                    raw.push(b'\\');
                    match self.read_byte()? {
                        None => return Err(StreamError::NotJson),
                        Some(b) => raw.push(b),
                    }
                }
                Some(b'"') => {
                    raw.push(b'"');
                    break;
                }
                Some(b) => raw.push(b),
            }
        }
        serde_json::from_slice::<String>(&raw).map_err(|e| StreamError::Json(e.to_string()))
    }

    /// Structurally skips exactly one JSON value starting at the next
    /// significant byte, materializing nothing. Honors string/escape state and
    /// enforces [`MAX_NESTING_DEPTH`].
    fn skip_value(&mut self) -> Result<(), StreamError> {
        self.skip_ws()?;
        let mut depth: usize = 0;
        let mut in_string = false;
        let mut escaped = false;
        let mut started = false;

        loop {
            let b = match self.peek_byte()? {
                Some(b) => b,
                None => {
                    if depth == 0 && started {
                        return Ok(());
                    }
                    return Err(StreamError::NotJson);
                }
            };

            if in_string {
                let _ = self.read_byte()?;
                if escaped {
                    escaped = false;
                } else if b == b'\\' {
                    escaped = true;
                } else if b == b'"' {
                    in_string = false;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                continue;
            }

            match b {
                b'"' => {
                    let _ = self.read_byte()?;
                    started = true;
                    in_string = true;
                }
                b'{' | b'[' => {
                    let _ = self.read_byte()?;
                    started = true;
                    depth += 1;
                    if depth > MAX_NESTING_DEPTH {
                        return Err(StreamError::DepthExceeded);
                    }
                }
                b'}' | b']' => {
                    if depth == 0 {
                        // This closer terminates the *container* we are inside,
                        // not our value: our scalar (if any) already ended.
                        // Leave it unconsumed for the caller.
                        return Ok(());
                    }
                    let _ = self.read_byte()?;
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                b',' => {
                    if depth == 0 {
                        // End of a scalar value at the top of this skip.
                        return Ok(());
                    }
                    let _ = self.read_byte()?;
                }
                b' ' | b'\t' | b'\n' | b'\r' => {
                    let _ = self.read_byte()?;
                    if depth == 0 && started {
                        // Whitespace after a finished scalar ends the value.
                        return Ok(());
                    }
                }
                _ => {
                    // A scalar literal byte (number / true / false / null).
                    let _ = self.read_byte()?;
                    started = true;
                }
            }
        }
    }

    /// Captures the bytes of the next array element into the reused scratch
    /// buffer, then parses it with `serde_json`. Returns `Ok(None)` at the
    /// closing `]` (which it consumes). Consumes a trailing `,` after an
    /// element.
    ///
    /// Why: this is the bounded-memory heart. It performs the same structural
    /// scan as the internal value-skip but *records* the bytes so the final
    /// parse delegates to `serde_json::from_slice`. The buffer holds exactly
    /// one element, so peak memory is bounded by the largest element — never by
    /// the page. This is precisely where #44 adds a hard `max_item_bytes` cap.
    ///
    /// # Errors
    /// [`StreamError::DepthExceeded`] on over-nesting,
    /// [`StreamError::Json`] if the captured slice is not valid JSON,
    /// [`StreamError::Io`] / [`StreamError::NotJson`] on read/structure faults.
    ///
    /// Memory model: streams element-by-element; reuses one buffer.
    #[must_use = "the next element Result must be handled"]
    pub fn next_element(&mut self) -> Result<Option<Value>, StreamError> {
        self.skip_ws()?;
        match self.peek_byte()? {
            Some(b']') => {
                let _ = self.read_byte()?;
                return Ok(None);
            }
            None => return Err(StreamError::NotJson),
            Some(_) => {}
        }

        // Capture exactly one element's bytes.
        self.scratch.clear();
        self.capture_value()?;

        let value = serde_json::from_slice::<Value>(&self.scratch)
            .map_err(|e| StreamError::Json(e.to_string()))?;

        // Consume a trailing ',' if present; tolerate trailing whitespace.
        self.skip_ws()?;
        if let Some(b',') = self.peek_byte()? {
            let _ = self.read_byte()?;
        }
        Ok(Some(value))
    }

    /// Like [`JsonSkimmer::skip_value`] but appends every consumed byte of the
    /// value to `self.scratch`. Stops at the value's structural end, leaving any
    /// following `,`/`]` unconsumed for the caller.
    fn capture_value(&mut self) -> Result<(), StreamError> {
        self.skip_ws()?;
        let mut depth: usize = 0;
        let mut in_string = false;
        let mut escaped = false;
        let mut started = false;

        loop {
            let b = match self.peek_byte()? {
                Some(b) => b,
                None => {
                    if depth == 0 && started {
                        return Ok(());
                    }
                    return Err(StreamError::NotJson);
                }
            };

            if in_string {
                let _ = self.read_byte()?;
                self.scratch.push(b);
                if escaped {
                    escaped = false;
                } else if b == b'\\' {
                    escaped = true;
                } else if b == b'"' {
                    in_string = false;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                continue;
            }

            match b {
                b'"' => {
                    let _ = self.read_byte()?;
                    self.scratch.push(b);
                    started = true;
                    in_string = true;
                }
                b'{' | b'[' => {
                    let _ = self.read_byte()?;
                    self.scratch.push(b);
                    started = true;
                    depth += 1;
                    if depth > MAX_NESTING_DEPTH {
                        return Err(StreamError::DepthExceeded);
                    }
                }
                b'}' | b']' => {
                    if depth == 0 {
                        // Closer of the enclosing array/object: our value ended.
                        return Ok(());
                    }
                    let _ = self.read_byte()?;
                    self.scratch.push(b);
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                b',' => {
                    if depth == 0 {
                        // Separator after a scalar element: leave it unconsumed.
                        return Ok(());
                    }
                    let _ = self.read_byte()?;
                    self.scratch.push(b);
                }
                b' ' | b'\t' | b'\n' | b'\r' => {
                    let _ = self.read_byte()?;
                    if depth == 0 && started {
                        // Whitespace after a finished scalar ends the value; do
                        // not record trailing whitespace.
                        return Ok(());
                    }
                    self.scratch.push(b);
                }
                _ => {
                    let _ = self.read_byte()?;
                    self.scratch.push(b);
                    started = true;
                }
            }
        }
    }
}

/// The boxed, synchronous fetch closure an automatic [`ItemStream`] calls to
/// retrieve the next page.
///
/// Why a neutral closure: this crate depends on no HTTP client. The caller
/// (the CLI transport, a mock, a test) supplies a `FnMut(&str)` that turns a
/// next-page URL into a [`ResponseStream`]; pagination drives that closure
/// without ever knowing how the bytes are fetched. It is `FnMut` (not `Fn`) so
/// a transport may hold mutable state (a client, a connection) across pages.
pub type PageFetch = Box<dyn FnMut(&str) -> Result<ResponseStream, StreamError>>;

/// Internal seam: where an [`ItemStream`] gets its bytes from.
///
/// Why: #24 shipped single-page streaming only and documented this enum as the
/// seam #27 would extend. #27 adds the [`PageSource::Paginated`] variant
/// *without* rewriting [`ItemStream::single`]; only the `next` match grows a
/// second arm.
enum PageSource {
    /// A single response page: one skimmer over one body, no further pages.
    /// Uses the outer [`ItemStream`] `sought`/`data_path` fields and stays
    /// **strict** on a non-array top level (its #24 contract).
    Single(JsonSkimmer<Box<dyn Read + Send>>),
    /// An automatically-paginated source: a chain of pages followed via
    /// `rel=next`, de-paginated into one lazy item stream. Carries everything
    /// the multi-page state machine needs.
    Paginated(Box<PaginatedSource>),
}

/// State for a [`PageSource::Paginated`] source: the current page's skimmer and
/// headers, the [`Paginator`] that computes the next URL, the fetch closure,
/// the page budget already spent, and per-page seek/leniency state.
///
/// Boxed inside [`PageSource`] because it is much larger than the `Single`
/// variant (a skimmer + headers + paginator + closure), keeping the enum small.
///
/// Memory model: **streams** across pages. Only the current page's skimmer (one
/// reused element buffer) and its small header map are resident; pages are
/// fetched one at a time and never accumulated. The lone exception is a
/// non-array top-level page captured whole into `pending_single` — the same
/// buffering the old `concat_results` did, and where #44 adds the byte guard.
struct PaginatedSource {
    /// Skimmer over the *current* page's body.
    skimmer: JsonSkimmer<Box<dyn Read + Send>>,
    /// The current page's response headers, used to compute the next URL.
    headers: Headers,
    /// The next-URL policy (RFC 5988 `rel=next`) plus the `max_pages` cap.
    paginator: Paginator,
    /// The caller-supplied synchronous page fetcher.
    fetch: PageFetch,
    /// How many pages have been fetched so far (the first page counts as 1).
    page_count: usize,
    /// True once the current page's data path has been navigated. Reset to
    /// `false` each time a fresh page is installed.
    page_sought: bool,
    /// For a lenient top-level non-array page: the captured whole value waiting
    /// to be emitted as the page's single item. Drained on the next `next`.
    pending_single: Option<Value>,
    /// True once the current page is known to be a streaming array (so further
    /// `next` calls pull elements rather than re-running the lenient seek).
    page_is_array: bool,
}

/// Layer 2: a lazy, bounded-memory iterator of response items.
///
/// Why: most callers want "the items", not raw bytes. `ItemStream` drives a
/// [`JsonSkimmer`] to yield one `serde_json::Value` per array element, parsing
/// each only when the caller asks for it. The final per-element parse delegates
/// to `serde_json`.
///
/// Two sources feed it:
///
/// * [`ItemStream::single`] — one response page (#24).
/// * [`ItemStream::paginated`] — a chain of pages followed automatically via
///   the response `Link` `rel=next` header and de-paginated into one stream
///   (#27). The next page is fetched only when the current page drains, so all
///   pages are never resident at once — this replaces the old eager
///   `concat_results`.
///
/// Memory model: **streams**. At most one element is resident at a time (plus
/// the skimmer's reused capture buffer); peak memory is bounded by the largest
/// element, independent of how large the page is, *and* independent of how many
/// pages are followed. `tests/stream_bound.rs` proves the per-page bound. The
/// one buffered case is a non-array top-level page on the paginated path, which
/// is captured whole exactly as `concat_results` did (#44 caps its size).
///
/// Errors are yielded inline as `Err(StreamError)` items; after an error the
/// iterator is fused to `None` so callers cannot loop forever on a broken body
/// or a failing fetch.
pub struct ItemStream {
    source: PageSource,
    /// True once navigation to the data path has been attempted. Used by the
    /// `Single` source only; the `Paginated` source tracks per-page seek state
    /// in [`PaginatedSource::page_sought`].
    sought: bool,
    /// The data path to navigate to on each page.
    data_path: DataPath,
    /// True once the stream has terminated (clean end or error); fuses `next`.
    done: bool,
}

impl ItemStream {
    /// Builds an item stream over a **single** response page.
    ///
    /// Why: this is #24's deliverable — one page, streamed item-by-item. It
    /// takes ownership of the [`ResponseStream`] body and the [`DataPath`] that
    /// locates the array within it. Navigation is deferred to the first
    /// [`Iterator::next`] call so construction never reads from the body.
    ///
    /// Multi-page pagination (the `next`-request machinery) is intentionally
    /// **not** here; #27 adds it by extending the internal page-source seam,
    /// not by rewriting this type.
    ///
    /// Memory model: streams; construction allocates only the skimmer buffer.
    #[must_use]
    pub fn single(resp: ResponseStream, data_path: DataPath) -> ItemStream {
        ItemStream {
            source: PageSource::Single(JsonSkimmer::new(resp.body)),
            sought: false,
            data_path,
            done: false,
        }
    }

    /// Builds an automatically-paginated item stream that de-paginates a chain
    /// of pages into one lazy stream (#27).
    ///
    /// `first` is the already-fetched first page; `data_path` locates the item
    /// array on **every** page; `paginator` computes the next-page URL from each
    /// page's headers (RFC 5988 `rel=next`) and supplies the `max_pages` cap;
    /// `fetch` turns a next-page URL into the next [`ResponseStream`].
    ///
    /// Behavior — the de-paginated contract:
    ///
    /// * Each page is navigated to `data_path` and its elements are streamed in
    ///   order; page envelopes are stripped, so the caller sees a flat item
    ///   stream as if the pages were one array (this is exactly the old
    ///   `concat_results` flatten, but lazy).
    /// * **Laziness:** the next page is fetched only when the current page's
    ///   items are fully drained — `fetch` is never called ahead of need, and
    ///   never at all if the caller stops early. No page is buffered whole
    ///   (except a non-array top-level page; see below).
    /// * The next URL is read from the current page's **headers**, never from
    ///   the already-consumed body, so the forward-only skimmer needs no rewind.
    /// * Following stops when `rel=next` is absent **or** `max_pages` pages have
    ///   been fetched, whichever comes first.
    /// * `concat_results` parity for [`DataPath::TopLevel`]: an array page
    ///   streams its elements; a non-array page yields that whole value as one
    ///   item then ends that page. For [`DataPath::Pointer`] paths a non-array
    ///   at the pointer is a [`StreamError::PathNotArray`] error (strict).
    /// * A fetch error or a navigation error fuses the stream with that `Err`.
    ///
    /// Memory model: **streams** across pages — one page's skimmer at a time;
    /// pages are fetched lazily and never accumulated. A non-array top-level
    /// page is the one buffered case (same as `concat_results`; #44 guards it).
    #[must_use]
    pub fn paginated(
        first: ResponseStream,
        data_path: DataPath,
        paginator: Paginator,
        fetch: PageFetch,
    ) -> ItemStream {
        let source = PaginatedSource {
            skimmer: JsonSkimmer::new(first.body),
            headers: first.headers,
            paginator,
            fetch,
            page_count: 1,
            page_sought: false,
            pending_single: None,
            page_is_array: false,
        };
        ItemStream {
            source: PageSource::Paginated(Box::new(source)),
            // `sought`/`data_path` here: the Paginated source uses `data_path`
            // for every page but tracks seek state per page internally, so the
            // outer `sought` flag is unused on this path.
            sought: false,
            data_path,
            done: false,
        }
    }
}

impl PaginatedSource {
    /// Pulls the next item from the paginated source, following `rel=next`
    /// across page boundaries as needed.
    ///
    /// Returns `Ok(Some(v))` for an item, `Ok(None)` when all reachable pages
    /// are exhausted, and `Err(e)` on a navigation or fetch fault (after which
    /// the caller fuses the stream).
    ///
    /// Uses a `loop` (not recursion) so a run of empty pages cannot grow the
    /// stack: a drained page that links to another simply re-enters the loop.
    fn next_item(&mut self, data_path: &DataPath) -> Result<Option<Value>, StreamError> {
        loop {
            // First touch of a freshly-installed page: navigate the data path.
            if !self.page_sought {
                self.page_sought = true;
                match data_path {
                    DataPath::TopLevel => match self.skimmer.seek_top_level_lenient()? {
                        TopLevelShape::Array => self.page_is_array = true,
                        TopLevelShape::Single(v) => {
                            // Whole-value page: emit it as this page's one item.
                            self.page_is_array = false;
                            self.pending_single = Some(v);
                        }
                    },
                    DataPath::Pointer(_) => {
                        // Pointer paths are strict; a non-array errors here.
                        self.skimmer.seek_to_data_path(data_path)?;
                        self.page_is_array = true;
                    }
                }
            }

            // A buffered non-array top-level page yields exactly one item.
            if let Some(v) = self.pending_single.take() {
                return Ok(Some(v));
            }

            // Stream the current array page's elements until it drains.
            if self.page_is_array {
                match self.skimmer.next_element()? {
                    Some(v) => return Ok(Some(v)),
                    None => { /* page drained; fall through to follow rel=next */ }
                }
            }

            // The current page is exhausted. Compute the next URL from THIS
            // page's headers (never the consumed body) and follow it if the
            // page budget allows; otherwise the stream ends.
            let Some(next_url) = self.paginator.next_url(&self.headers) else {
                return Ok(None);
            };
            if self.page_count >= self.paginator.max_pages {
                return Ok(None);
            }

            let next_page = (self.fetch)(&next_url)?;
            // Install the fresh page: new skimmer + headers, reset per-page
            // state, and loop to navigate and stream it.
            self.skimmer = JsonSkimmer::new(next_page.body);
            self.headers = next_page.headers;
            self.page_count += 1;
            self.page_sought = false;
            self.page_is_array = false;
            self.pending_single = None;
        }
    }
}

impl Iterator for ItemStream {
    type Item = Result<Value, StreamError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match &mut self.source {
            PageSource::Single(skimmer) => {
                // Navigate to the array exactly once, before the first element.
                // Single stays strict: a non-array top level errors (its #24
                // contract); only the Paginated path applies concat_results
                // leniency.
                if !self.sought {
                    self.sought = true;
                    if let Err(e) = skimmer.seek_to_data_path(&self.data_path) {
                        self.done = true;
                        return Some(Err(e));
                    }
                }

                match skimmer.next_element() {
                    Ok(Some(v)) => Some(Ok(v)),
                    Ok(None) => {
                        self.done = true;
                        None
                    }
                    Err(e) => {
                        self.done = true;
                        Some(Err(e))
                    }
                }
            }
            PageSource::Paginated(source) => match source.next_item(&self.data_path) {
                Ok(Some(v)) => Some(Ok(v)),
                Ok(None) => {
                    self.done = true;
                    None
                }
                Err(e) => {
                    self.done = true;
                    Some(Err(e))
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::Status;

    /// Builds a `ResponseStream` from an in-memory byte body for unit testing.
    fn resp_from(bytes: &'static [u8]) -> ResponseStream {
        ResponseStream {
            status: Status(200),
            headers: Default::default(),
            body: Box::new(std::io::Cursor::new(bytes)),
        }
    }

    fn collect(body: &'static [u8], path: DataPath) -> Result<Vec<Value>, StreamError> {
        ItemStream::single(resp_from(body), path).collect()
    }

    #[test]
    fn top_level_array_of_scalars() {
        let items = collect(b"[1, 2, 3]", DataPath::TopLevel).unwrap();
        assert_eq!(items, vec![Value::from(1), Value::from(2), Value::from(3)]);
    }

    #[test]
    fn empty_top_level_array() {
        let items = collect(b"[]", DataPath::TopLevel).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn empty_array_at_pointer() {
        let items = collect(
            br#"{"root": []}"#,
            DataPath::Pointer(vec!["root".to_string()]),
        )
        .unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn element_string_containing_structural_bytes_and_escapes() {
        // A string element that contains ] } , an escaped quote and a backslash.
        let body = br#"[ "a]b}c,d\"e\\f" ]"#;
        let items = collect(body, DataPath::TopLevel).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0], Value::from("a]b}c,d\"e\\f"));
    }

    #[test]
    fn element_nested_object() {
        let body = br#"[ {"a": {"b": [1, 2]}, "c": "x,y]z"} ]"#;
        let items = collect(body, DataPath::TopLevel).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["a"]["b"], Value::from(vec![1, 2]));
        assert_eq!(items[0]["c"], Value::from("x,y]z"));
    }

    #[test]
    fn element_nested_array() {
        let body = br#"[ [1, [2, 3], 4], [5] ]"#;
        let items = collect(body, DataPath::TopLevel).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], serde_json::json!([1, [2, 3], 4]));
        assert_eq!(items[1], serde_json::json!([5]));
    }

    #[test]
    fn pointer_single_segment() {
        let body = br#"{"items": [10, 20], "other": 1}"#;
        let items = collect(body, DataPath::Pointer(vec!["items".to_string()])).unwrap();
        assert_eq!(items, vec![Value::from(10), Value::from(20)]);
    }

    #[test]
    fn pointer_multi_segment_skips_siblings() {
        // The 'a' object has sibling keys before 'b' that must be skipped, and
        // 'result' has sibling keys with nested structures to skip-navigate.
        let body = br#"{
            "meta": {"x": [1, 2, 3], "y": {"z": 9}},
            "a": {"junk": "p]q}r", "b": [100, 200, 300]}
        }"#;
        let items = collect(
            body,
            DataPath::Pointer(vec!["a".to_string(), "b".to_string()]),
        )
        .unwrap();
        assert_eq!(
            items,
            vec![Value::from(100), Value::from(200), Value::from(300)]
        );
    }

    #[test]
    fn pointer_absent_key_errors() {
        let body = br#"{"present": [1, 2]}"#;
        let err = collect(body, DataPath::Pointer(vec!["missing".to_string()])).unwrap_err();
        assert!(matches!(err, StreamError::PathNotArray(_)));
    }

    #[test]
    fn pointer_non_array_value_errors() {
        let body = br#"{"x": {"not": "an array"}}"#;
        let err = collect(body, DataPath::Pointer(vec!["x".to_string()])).unwrap_err();
        assert!(matches!(err, StreamError::PathNotArray(_)));
    }

    #[test]
    fn top_level_non_array_errors() {
        let body = br#"{"a": 1}"#;
        let err = collect(body, DataPath::TopLevel).unwrap_err();
        assert!(matches!(err, StreamError::PathNotArray(_)));
    }

    #[test]
    fn nesting_depth_cap_breach_errors() {
        // Build a deeply nested single element that exceeds MAX_NESTING_DEPTH.
        let depth = MAX_NESTING_DEPTH + 5;
        let mut body = String::from("[");
        for _ in 0..depth {
            body.push('[');
        }
        for _ in 0..depth {
            body.push(']');
        }
        body.push(']');
        let stream = ItemStream::single(
            ResponseStream {
                status: Status(200),
                headers: Default::default(),
                body: Box::new(std::io::Cursor::new(body.into_bytes())),
            },
            DataPath::TopLevel,
        );
        let results: Vec<_> = stream.collect();
        assert!(
            results
                .iter()
                .any(|r| matches!(r, Err(StreamError::DepthExceeded)))
        );
    }

    #[test]
    fn unicode_and_escaped_unicode_elements() {
        let body = r#"[ "héllo é wörld", "🦀 ferris" ]"#.as_bytes();
        // Re-borrow as 'static via Box; build the stream directly.
        let stream = ItemStream::single(
            ResponseStream {
                status: Status(200),
                headers: Default::default(),
                body: Box::new(std::io::Cursor::new(body.to_vec())),
            },
            DataPath::TopLevel,
        );
        let items: Vec<Value> = stream.map(|r| r.unwrap()).collect();
        assert_eq!(items[0], Value::from("héllo é wörld"));
        assert_eq!(items[1], Value::from("🦀 ferris"));
    }

    #[test]
    fn mixed_object_and_scalar_elements() {
        let body = br#"[ {"id": 1}, "two", 3, true, null ]"#;
        let items = collect(body, DataPath::TopLevel).unwrap();
        assert_eq!(items.len(), 5);
        assert_eq!(items[0]["id"], Value::from(1));
        assert_eq!(items[1], Value::from("two"));
        assert_eq!(items[2], Value::from(3));
        assert_eq!(items[3], Value::Bool(true));
        assert_eq!(items[4], Value::Null);
    }

    #[test]
    fn error_fuses_iterator() {
        let body = br#"{"a": 1}"#;
        let mut stream = ItemStream::single(resp_from(body), DataPath::TopLevel);
        let first = stream.next();
        assert!(matches!(first, Some(Err(StreamError::PathNotArray(_)))));
        // After an error the iterator is fused.
        assert!(stream.next().is_none());
    }

    // ----- #27: automatic pagination -----

    use std::cell::RefCell;
    use std::rc::Rc;

    /// Builds a `ResponseStream` carrying an owned body and an owned `Link`
    /// header (so the paginator can compute the next URL from it).
    fn page(body: Vec<u8>, link: Option<&str>) -> ResponseStream {
        let mut headers = Headers::new();
        if let Some(l) = link {
            headers.insert("link".to_string(), l.to_string());
        }
        ResponseStream {
            status: Status(200),
            headers,
            body: Box::new(std::io::Cursor::new(body)),
        }
    }

    #[test]
    fn three_pages_concatenate_in_order() {
        // Pages 2 and 3 are served by the fetch closure keyed on next URL.
        let p2 = br#"[3, 4]"#.to_vec();
        let p3 = br#"[5, 6]"#.to_vec();
        let fetch: PageFetch = Box::new(move |url: &str| match url {
            "https://api/x?page=2" => Ok(page(
                p2.clone(),
                Some(r#"<https://api/x?page=3>; rel="next""#),
            )),
            "https://api/x?page=3" => Ok(page(p3.clone(), None)),
            other => panic!("unexpected fetch url: {other}"),
        });
        let first = page(
            br#"[1, 2]"#.to_vec(),
            Some(r#"<https://api/x?page=2>; rel="next""#),
        );
        let items: Vec<Value> =
            ItemStream::paginated(first, DataPath::TopLevel, Paginator::default(), fetch)
                .map(Result::unwrap)
                .collect();
        assert_eq!(
            items,
            vec![
                Value::from(1),
                Value::from(2),
                Value::from(3),
                Value::from(4),
                Value::from(5),
                Value::from(6),
            ]
        );
    }

    #[test]
    fn fetch_is_lazy_not_called_until_prior_page_drains() {
        // The closure records, in order, the items already pulled when it is
        // invoked, proving page 1 fully drains before page 2 is fetched.
        let pulled: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(Vec::new()));
        let fetch_log = Rc::clone(&pulled);
        let p2 = br#"[3, 4]"#.to_vec();
        let fetch: PageFetch = Box::new(move |url: &str| {
            assert_eq!(url, "https://api/x?page=2");
            // At the moment of fetch, page 1's two items must already be pulled.
            assert_eq!(*fetch_log.borrow(), vec![1, 2]);
            Ok(page(p2.clone(), None))
        });
        let first = page(
            br#"[1, 2]"#.to_vec(),
            Some(r#"<https://api/x?page=2>; rel="next""#),
        );
        let mut stream =
            ItemStream::paginated(first, DataPath::TopLevel, Paginator::default(), fetch);

        // Pull page 1's items one at a time; fetch must NOT have fired yet.
        for expected in [1i64, 2] {
            let v = stream.next().unwrap().unwrap();
            pulled.borrow_mut().push(v.as_i64().unwrap());
            assert_eq!(v, Value::from(expected));
        }
        // Pulling again drains the boundary and triggers exactly one fetch.
        let v = stream.next().unwrap().unwrap();
        assert_eq!(v, Value::from(3));
    }

    #[test]
    fn fetch_not_called_when_caller_stops_early() {
        // If the caller never drains page 1, the next page is never fetched.
        let fetch: PageFetch = Box::new(|_url: &str| panic!("fetch must not be called"));
        let first = page(
            br#"[1, 2, 3]"#.to_vec(),
            Some(r#"<https://api/x?page=2>; rel="next""#),
        );
        let mut stream =
            ItemStream::paginated(first, DataPath::TopLevel, Paginator::default(), fetch);
        // Pull just one item, then drop the stream — no fetch should occur.
        assert_eq!(stream.next().unwrap().unwrap(), Value::from(1));
        drop(stream);
    }

    #[test]
    fn max_pages_cap_stops_following() {
        // Every page links to a next page, but max_pages=2 stops after page 2.
        let fetch_count = Rc::new(RefCell::new(0usize));
        let fc = Rc::clone(&fetch_count);
        let fetch: PageFetch = Box::new(move |_url: &str| {
            *fc.borrow_mut() += 1;
            // Always offers a further next link.
            Ok(page(
                br#"[9]"#.to_vec(),
                Some(r#"<https://api/x?next>; rel="next""#),
            ))
        });
        let first = page(
            br#"[1]"#.to_vec(),
            Some(r#"<https://api/x?next>; rel="next""#),
        );
        let paginator = Paginator { max_pages: 2 };
        let items: Vec<Value> = ItemStream::paginated(first, DataPath::TopLevel, paginator, fetch)
            .map(Result::unwrap)
            .collect();
        // Page 1 (item 1) + page 2 (item 9); page 3 is never fetched.
        assert_eq!(items, vec![Value::from(1), Value::from(9)]);
        assert_eq!(
            *fetch_count.borrow(),
            1,
            "exactly one fetch under max_pages=2"
        );
    }

    #[test]
    fn top_level_non_array_page_yields_single_item() {
        // concat_results parity: a non-array top-level page becomes ONE item.
        let p2 = br#"{"meta": true}"#.to_vec();
        let fetch: PageFetch = Box::new(move |_url: &str| Ok(page(p2.clone(), None)));
        let first = page(
            br#"[1, 2]"#.to_vec(),
            Some(r#"<https://api/x?page=2>; rel="next""#),
        );
        let items: Vec<Value> =
            ItemStream::paginated(first, DataPath::TopLevel, Paginator::default(), fetch)
                .map(Result::unwrap)
                .collect();
        assert_eq!(
            items,
            vec![
                Value::from(1),
                Value::from(2),
                serde_json::json!({"meta": true}),
            ]
        );
    }

    #[test]
    fn single_non_array_top_level_page_is_one_item() {
        // A lone non-array page (no following) is itself a single item.
        let fetch: PageFetch = Box::new(|_url: &str| panic!("no further pages"));
        let first = page(br#"{"only": 1}"#.to_vec(), None);
        let items: Vec<Value> =
            ItemStream::paginated(first, DataPath::TopLevel, Paginator::default(), fetch)
                .map(Result::unwrap)
                .collect();
        assert_eq!(items, vec![serde_json::json!({"only": 1})]);
    }

    #[test]
    fn pointer_non_array_page_errors_path_not_array() {
        // Pointer paths stay strict: a non-array at the pointer is an error,
        // NOT a single item (unlike TopLevel leniency).
        let first = page(br#"{"items": {"not": "array"}}"#.to_vec(), None);
        let fetch: PageFetch = Box::new(|_url: &str| panic!("no further pages"));
        let mut stream = ItemStream::paginated(
            first,
            DataPath::Pointer(vec!["items".to_string()]),
            Paginator::default(),
            fetch,
        );
        assert!(matches!(
            stream.next(),
            Some(Err(StreamError::PathNotArray(_)))
        ));
        // Error fuses the stream.
        assert!(stream.next().is_none());
    }

    #[test]
    fn pointer_paths_concatenate_across_pages() {
        // De-paginate a pointer-located array across two pages.
        let p2 = br#"{"items": [3, 4], "page": 2}"#.to_vec();
        let fetch: PageFetch = Box::new(move |_url: &str| Ok(page(p2.clone(), None)));
        let first = page(
            br#"{"items": [1, 2], "page": 1}"#.to_vec(),
            Some(r#"<https://api/x?page=2>; rel="next""#),
        );
        let items: Vec<Value> = ItemStream::paginated(
            first,
            DataPath::Pointer(vec!["items".to_string()]),
            Paginator::default(),
            fetch,
        )
        .map(Result::unwrap)
        .collect();
        assert_eq!(
            items,
            vec![
                Value::from(1),
                Value::from(2),
                Value::from(3),
                Value::from(4)
            ]
        );
    }

    #[test]
    fn fetch_error_fuses_stream() {
        let fetch: PageFetch =
            Box::new(|_url: &str| Err(StreamError::Io(std::io::Error::other("boom"))));
        let first = page(
            br#"[1]"#.to_vec(),
            Some(r#"<https://api/x?page=2>; rel="next""#),
        );
        let mut stream =
            ItemStream::paginated(first, DataPath::TopLevel, Paginator::default(), fetch);
        assert_eq!(stream.next().unwrap().unwrap(), Value::from(1));
        // Draining page 1 triggers the fetch, which fails and surfaces inline.
        assert!(matches!(stream.next(), Some(Err(StreamError::Io(_)))));
        // Then the stream is fused.
        assert!(stream.next().is_none());
    }

    #[test]
    fn empty_pages_do_not_recurse_and_are_skipped() {
        // A chain of empty array pages must not blow the stack and must reach
        // the final page's items (loop, not recursion).
        let p2 = br#"[]"#.to_vec();
        let p3 = br#"[]"#.to_vec();
        let p4 = br#"[42]"#.to_vec();
        let fetch: PageFetch = Box::new(move |url: &str| match url {
            "https://api/x?p=2" => Ok(page(p2.clone(), Some(r#"<https://api/x?p=3>; rel="next""#))),
            "https://api/x?p=3" => Ok(page(p3.clone(), Some(r#"<https://api/x?p=4>; rel="next""#))),
            "https://api/x?p=4" => Ok(page(p4.clone(), None)),
            other => panic!("unexpected url: {other}"),
        });
        let first = page(
            br#"[]"#.to_vec(),
            Some(r#"<https://api/x?p=2>; rel="next""#),
        );
        let items: Vec<Value> =
            ItemStream::paginated(first, DataPath::TopLevel, Paginator::default(), fetch)
                .map(Result::unwrap)
                .collect();
        assert_eq!(items, vec![Value::from(42)]);
    }

    #[test]
    fn seek_top_level_lenient_distinguishes_array_and_value() {
        let mut arr = JsonSkimmer::new(
            Box::new(std::io::Cursor::new(b"[1, 2]".to_vec())) as Box<dyn Read + Send>
        );
        assert!(matches!(
            arr.seek_top_level_lenient().unwrap(),
            TopLevelShape::Array
        ));

        let mut obj = JsonSkimmer::new(
            Box::new(std::io::Cursor::new(b"{\"a\":1}".to_vec())) as Box<dyn Read + Send>
        );
        match obj.seek_top_level_lenient().unwrap() {
            TopLevelShape::Single(v) => assert_eq!(v, serde_json::json!({"a": 1})),
            TopLevelShape::Array => panic!("object should be a single value"),
        }
    }
}
