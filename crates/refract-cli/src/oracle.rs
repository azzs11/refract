//! Real-engine replay (doc §5.10). Every "different" verdict is replayed on the real engines
//! before it is shown as confirmed; without an oracle for a flavour it is labelled "predicted".
//!
//! P2 ships the in-process oracle for the Rust flavour (the `regex` crate itself, ADR-0005).
//! The Docker oracle for the other engines arrives in P3 behind the same function.

use refract_core::{Flavor, Op};

/// Engine name and version reported next to every replay.
pub fn engine_label(flavor: Flavor) -> Option<&'static str> {
    match flavor {
        Flavor::Rust => Some("rust regex 1.13 (in-process)"),
        _ => None,
    }
}

/// Runs `pattern` on `input` under `op` on the real engine. `None` when no oracle exists for
/// the flavour yet; `Some(Err)` when the real engine rejects the pattern.
pub fn run(flavor: Flavor, pattern: &str, op: Op, input: &str) -> Option<Result<bool, String>> {
    match flavor {
        Flavor::Rust => {
            let compiled = match op {
                Op::Search => regex::Regex::new(pattern),
                // \A and \z: Rust's `$` would also be end-of-text, but be explicit. The newline
                // before `)` would change the pattern, so a trailing `x`-mode comment is not
                // protected; patterns like `(?x)a#c` are rare in practice and are reported as
                // engine errors rather than silently mis-replayed.
                Op::FullMatch => regex::Regex::new(&format!(r"\A(?:{pattern})\z")),
            };
            Some(
                compiled
                    .map(|r| r.is_match(input))
                    .map_err(|e| e.to_string()),
            )
        }
        _ => None,
    }
}
