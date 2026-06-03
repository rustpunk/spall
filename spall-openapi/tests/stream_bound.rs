//! Gating bounded-memory test (verdict adjustment #3).
//!
//! This is a *separate* integration-test binary so it can install its own
//! global allocator: a counting allocator that wraps `std::alloc::System` and
//! tracks current + peak resident bytes. We drive [`ItemStream`] over a body
//! that lazily emits a multi-hundred-megabyte JSON document **without ever
//! holding the whole body in memory**, then assert that peak heap stayed far
//! below the body size. That proves the parser streams element-by-element and
//! never buffers the page — the load-bearing property of the whole crate.

use spall_openapi::{DataPath, ItemStream, ResponseStream, Status};
use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// Counting global allocator
// ---------------------------------------------------------------------------

/// Currently-live heap bytes allocated through this allocator.
static CURRENT: AtomicUsize = AtomicUsize::new(0);
/// High-water mark of `CURRENT` since the last reset.
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// An allocator that forwards to `System` while tracking current and peak
/// live bytes. Installed as the `#[global_allocator]` for this test binary so
/// every allocation in the process is counted.
struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            // Bump PEAK up to `now` if it is higher (CAS loop avoids races).
            let mut peak = PEAK.load(Ordering::Relaxed);
            while now > peak {
                match PEAK.compare_exchange_weak(peak, now, Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(observed) => peak = observed,
                }
            }
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            // Adjust the live counter by the delta, then re-check the peak.
            if new_size >= layout.size() {
                let delta = new_size - layout.size();
                let now = CURRENT.fetch_add(delta, Ordering::Relaxed) + delta;
                let mut peak = PEAK.load(Ordering::Relaxed);
                while now > peak {
                    match PEAK.compare_exchange_weak(
                        peak,
                        now,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(observed) => peak = observed,
                    }
                }
            } else {
                CURRENT.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new_ptr
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Resets the peak high-water mark to the current live byte count.
fn reset_peak() {
    PEAK.store(CURRENT.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// Returns the peak-minus-baseline bytes observed since the last `reset_peak`,
/// i.e. how much *additional* heap the measured work caused at its worst.
fn peak_over_baseline(baseline: usize) -> usize {
    PEAK.load(Ordering::Relaxed).saturating_sub(baseline)
}

// ---------------------------------------------------------------------------
// Lazy multi-hundred-MB body generators (never hold the whole body in memory)
// ---------------------------------------------------------------------------

/// One serialized element, repeated N times to build a large array. Chosen to
/// contain structural bytes inside a string so the test also exercises the
/// skimmer's string/escape handling at scale.
const ELEM: &[u8] =
    br#"{"id":1234567,"name":"item ],},\" x","tags":["a","b","c"],"nested":{"k":[1,2,3]}}"#;

/// Number of elements; sized so the *total emitted body* exceeds 256 MB while
/// the generator only ever holds a few bytes of state.
fn element_count() -> usize {
    // ELEM is ~80 bytes; 4_000_000 elements => ~320 MB of body.
    4_000_000
}

/// A `Read` that lazily emits `{"root":[ ELEM, ELEM, ... ]}` for `n` elements.
///
/// It fills the caller's buffer on demand from a tiny rolling state machine and
/// never materializes the full body — proving the *producer* side is also
/// bounded, so any buffering observed in the peak must come from the parser.
struct PointerBodyGen {
    n: usize,
    emitted: usize,
    // The bytes we still owe the reader from the current chunk.
    pending: Vec<u8>,
    pending_pos: usize,
    // Phase of the document we are emitting.
    phase: Phase,
}

/// A `Read` that lazily emits a top-level array `[ ELEM, ELEM, ... ]`.
struct TopLevelBodyGen {
    n: usize,
    emitted: usize,
    pending: Vec<u8>,
    pending_pos: usize,
    phase: Phase,
}

#[derive(PartialEq)]
enum Phase {
    Prefix,
    Elements,
    Suffix,
    Done,
}

impl PointerBodyGen {
    fn new(n: usize) -> Self {
        PointerBodyGen {
            n,
            emitted: 0,
            pending: b"{\"root\":[".to_vec(),
            pending_pos: 0,
            phase: Phase::Prefix,
        }
    }

    /// Refills `pending` with the next logical chunk when the current one is
    /// drained. Returns false when the document is complete.
    fn refill(&mut self) -> bool {
        self.pending_pos = 0;
        self.pending.clear();
        match self.phase {
            Phase::Prefix => {
                self.phase = Phase::Elements;
                self.refill()
            }
            Phase::Elements => {
                if self.emitted >= self.n {
                    self.phase = Phase::Suffix;
                    return self.refill();
                }
                if self.emitted > 0 {
                    self.pending.push(b',');
                }
                self.pending.extend_from_slice(ELEM);
                self.emitted += 1;
                true
            }
            Phase::Suffix => {
                self.pending.extend_from_slice(b"]}");
                self.phase = Phase::Done;
                true
            }
            Phase::Done => false,
        }
    }
}

impl Read for PointerBodyGen {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pending_pos >= self.pending.len() && !self.refill() {
            return Ok(0);
        }
        let avail = &self.pending[self.pending_pos..];
        let n = avail.len().min(buf.len());
        buf[..n].copy_from_slice(&avail[..n]);
        self.pending_pos += n;
        Ok(n)
    }
}

impl TopLevelBodyGen {
    fn new(n: usize) -> Self {
        TopLevelBodyGen {
            n,
            emitted: 0,
            pending: b"[".to_vec(),
            pending_pos: 0,
            phase: Phase::Prefix,
        }
    }

    fn refill(&mut self) -> bool {
        self.pending_pos = 0;
        self.pending.clear();
        match self.phase {
            Phase::Prefix => {
                self.phase = Phase::Elements;
                self.refill()
            }
            Phase::Elements => {
                if self.emitted >= self.n {
                    self.phase = Phase::Suffix;
                    return self.refill();
                }
                if self.emitted > 0 {
                    self.pending.push(b',');
                }
                self.pending.extend_from_slice(ELEM);
                self.emitted += 1;
                true
            }
            Phase::Suffix => {
                self.pending.extend_from_slice(b"]");
                self.phase = Phase::Done;
                true
            }
            Phase::Done => false,
        }
    }
}

impl Read for TopLevelBodyGen {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pending_pos >= self.pending.len() && !self.refill() {
            return Ok(0);
        }
        let avail = &self.pending[self.pending_pos..];
        let n = avail.len().min(buf.len());
        buf[..n].copy_from_slice(&avail[..n]);
        self.pending_pos += n;
        Ok(n)
    }
}

// ---------------------------------------------------------------------------
// The gating tests
// ---------------------------------------------------------------------------

/// Peak heap ceiling. The body is >= 256 MB; the parser must stay well under
/// 16 MB, i.e. << the body, proving element-by-element streaming.
const PEAK_CEILING_BYTES: usize = 16 * 1024 * 1024;

#[test]
fn pointer_array_streams_bounded_memory() {
    let n = element_count();
    let body_estimate = ELEM.len() * n;
    assert!(
        body_estimate >= 256 * 1024 * 1024,
        "body must be >= 256 MB to be a meaningful bound; got {body_estimate} bytes"
    );

    let resp = ResponseStream {
        status: Status(200),
        headers: Default::default(),
        body: Box::new(PointerBodyGen::new(n)),
    };

    let baseline = CURRENT.load(Ordering::Relaxed);
    reset_peak();

    let stream = ItemStream::single(resp, DataPath::Pointer(vec!["root".to_string()]));

    let mut count = 0usize;
    let mut first_seen = None;
    for item in stream {
        let v = item.expect("element must parse");
        if count == 0 {
            first_seen = Some(v.clone());
        }
        // Spot-check a sampled element parses to the expected structure.
        if count == n / 2 {
            assert_eq!(v["id"], serde_json::json!(1234567));
            assert_eq!(v["name"], serde_json::json!("item ],},\" x"));
            assert_eq!(v["tags"], serde_json::json!(["a", "b", "c"]));
            assert_eq!(v["nested"]["k"], serde_json::json!([1, 2, 3]));
        }
        count += 1;
    }

    let peak = peak_over_baseline(baseline);

    // (a) Every element was produced and a spot element parsed correctly.
    assert_eq!(count, n, "must stream exactly N elements");
    let first = first_seen.expect("at least one element");
    assert_eq!(first["id"], serde_json::json!(1234567));

    // (b) Peak heap stayed bounded — << the 256+ MB body.
    assert!(
        peak < PEAK_CEILING_BYTES,
        "peak heap {peak} bytes exceeded ceiling {PEAK_CEILING_BYTES}; \
         parser is buffering the page instead of streaming"
    );
}

#[test]
fn top_level_array_streams_bounded_memory() {
    let n = element_count();
    let body_estimate = ELEM.len() * n;
    assert!(body_estimate >= 256 * 1024 * 1024);

    let resp = ResponseStream {
        status: Status(200),
        headers: Default::default(),
        body: Box::new(TopLevelBodyGen::new(n)),
    };

    let baseline = CURRENT.load(Ordering::Relaxed);
    reset_peak();

    let stream = ItemStream::single(resp, DataPath::TopLevel);

    let mut count = 0usize;
    for item in stream {
        let v = item.expect("element must parse");
        if count == n - 1 {
            assert_eq!(v["id"], serde_json::json!(1234567));
        }
        count += 1;
    }

    let peak = peak_over_baseline(baseline);
    assert_eq!(count, n, "must stream exactly N elements");
    assert!(
        peak < PEAK_CEILING_BYTES,
        "peak heap {peak} bytes exceeded ceiling {PEAK_CEILING_BYTES}"
    );
}
