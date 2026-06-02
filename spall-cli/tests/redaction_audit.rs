//! Issue #13 Phase 4: redaction discipline audit.
//!
//! Scans every `.rs` file under `spall-cli/src/auth/` and asserts no
//! `expose_secret(` call is within `K` source lines of any logging
//! macro (`eprintln!`, `println!`, `tracing::`, `log::`, `dbg!`). The
//! intent is to catch a refactor that accidentally hoists an
//! `expose_secret()` call into stderr / structured-log proximity.
//!
//! Scope and limits (do not overstate what this catches):
//! - Grep-based proximity heuristic, NOT semantic data-flow analysis.
//!   Will NOT catch `format!("{}", token.expose_secret())` whose
//!   result later flows into `eprintln!` 10 lines away.
//! - Does NOT catch `serde::Serialize` adjacency, `Display` impls, or
//!   `Debug` derives — the in-source `// SECURITY:` comments name those
//!   sites aspirationally; this test enforces only the log-proximity
//!   subset.
//! - Non-recursive: only scans top-level `.rs` files under `auth/`,
//!   not subdirectories. A future `auth/providers/` submodule must
//!   either be flat or this test must be extended.
//!
//! The matcher uses the literal substring `expose_secret(` (with open
//! paren) so the `// SECURITY: ... `expose_secret` ...` comments don't
//! trigger false positives — comment text uses backticks without a
//! paren after the identifier.

use std::fs;
use std::path::Path;

/// Proximity window in source lines. K=3 catches the immediate
/// `let v = format!(...); eprintln!(v);` hoist pattern. The current
/// `mod.rs` geometry (eprintln at line 40, nearest `expose_secret(`
/// at line 48, distance 8) sits well outside the window; a refactor
/// that closes that gap by 5+ lines trips this test.
const K: usize = 3;
const LOG_MARKERS: &[&str] = &[
    "eprintln!",
    "println!",
    "tracing::",
    "log::",
    "dbg!",
];

#[test]
fn expose_secret_never_within_k_lines_of_log_macro() {
    let auth_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("auth");
    let mut violations: Vec<String> = Vec::new();
    for entry in fs::read_dir(&auth_dir).expect("read auth dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let src = fs::read_to_string(&path).expect("read source");
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !line.contains("expose_secret(") {
                continue;
            }
            let lo = i.saturating_sub(K);
            let hi = (i + K + 1).min(lines.len());
            for (j, neighbor) in lines.iter().enumerate().take(hi).skip(lo) {
                if j == i {
                    continue;
                }
                if LOG_MARKERS.iter().any(|m| neighbor.contains(m)) {
                    violations.push(format!(
                        "{}:{}: `expose_secret(` within {} lines of a log macro at line {}",
                        path.display(),
                        i + 1,
                        K,
                        j + 1,
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "redaction discipline violations:\n{}",
        violations.join("\n"),
    );
}
