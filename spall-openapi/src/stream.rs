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

/// Internal seam: where a [`ItemStream`] gets its bytes from.
///
/// Why: #24 ships single-page streaming only, but #27 must add multi-page
/// pagination *without* rewriting `ItemStream`. Modeling the byte source as an
/// enum lets #27 add a `Paginated { .. }` variant later while `next` stays the
/// same shape. We add no multi-page variant now (no dead code); the enum simply
/// names the single seam #27 will extend.
enum PageSource {
    /// A single response page: one skimmer over one body, no further pages.
    Single(JsonSkimmer<Box<dyn Read + Send>>),
}

/// Layer 2: a lazy, bounded-memory iterator of response items.
///
/// Why: most callers want "the items", not raw bytes. `ItemStream` drives a
/// [`JsonSkimmer`] to yield one `serde_json::Value` per array element, parsing
/// each only when the caller asks for it. The final per-element parse delegates
/// to `serde_json`.
///
/// Memory model: **streams**. At most one element is resident at a time (plus
/// the skimmer's reused capture buffer); peak memory is bounded by the largest
/// element, independent of how large the page is. `tests/stream_bound.rs`
/// proves this against a multi-hundred-megabyte body.
///
/// Errors are yielded inline as `Err(StreamError)` items; after an error the
/// iterator is fused to `None` so callers cannot loop forever on a broken body.
pub struct ItemStream {
    source: PageSource,
    /// True once navigation to the data path has been attempted.
    sought: bool,
    /// The data path to navigate to on first `next`.
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
}

impl Iterator for ItemStream {
    type Item = Result<Value, StreamError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let PageSource::Single(skimmer) = &mut self.source;

        // Navigate to the array exactly once, before the first element.
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
}
