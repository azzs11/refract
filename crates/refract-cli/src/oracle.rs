//! Real-engine replay (plan §5.10). Every "different" verdict is replayed on the real engines
//! before it is shown as confirmed; when an engine is not available the verdict is labelled
//! "predicted" and the report says why.
//!
//! - Rust: in-process (the `regex` crate itself, ADR-0005).
//! - Python, JavaScript: native `python3` / `node` when installed, else the pinned Docker image.
//! - Go, Java: pinned Docker images (`docker build -t refract-oracle-go oracle/go`, same for java).

use refract_core::{Flavor, Op};
use refract_oracle::{Engine, EngineKind, Mode, Op as OOp, Outcome};
use std::collections::HashMap;

/// Long-lived engine processes, started on first use.
#[derive(Default)]
pub struct Oracles {
    engines: HashMap<EngineKind, Result<Engine, String>>,
}

fn kind(flavor: Flavor) -> Option<EngineKind> {
    match flavor {
        Flavor::Python => Some(EngineKind::Python),
        Flavor::JavaScript => Some(EngineKind::Node),
        Flavor::Go => Some(EngineKind::Go),
        Flavor::Java => Some(EngineKind::Java),
        _ => None,
    }
}

/// `/src/flags` → (src, flags); anything else is a bare source with no flags.
fn js_literal(p: &str) -> (&str, &str) {
    if let Some(rest) = p.strip_prefix('/')
        && let Some(end) = rest.rfind('/')
        && rest[end + 1..].chars().all(|c| c.is_ascii_alphabetic())
    {
        return (&rest[..end], &rest[end + 1..]);
    }
    (p, "")
}

impl Oracles {
    fn engine(&mut self, k: EngineKind) -> Result<&mut Engine, String> {
        let slot = self.engines.entry(k).or_insert_with(|| {
            Engine::spawn(k, Mode::from_env())
                .or_else(|_| Engine::spawn(k, Mode::Docker))
                .map_err(|e| {
                    format!(
                        "{} engine unavailable ({e}); build it with `docker build -t {} oracle/{}`",
                        k.name(),
                        k.image(),
                        k.name()
                    )
                })
        });
        slot.as_mut().map_err(|e| e.clone())
    }

    /// Engine name and version, e.g. `python 3.15.2`.
    pub fn label(&mut self, flavor: Flavor) -> String {
        match flavor {
            Flavor::Rust => "rust regex 1.13 (in-process)".to_string(),
            f => match kind(f).map(|k| self.engine(k)) {
                Some(Ok(e)) => format!("{} {}", e.kind().name(), e.version()),
                Some(Err(e)) => e,
                None => format!("no oracle for {f}"),
            },
        }
    }

    /// Runs `pattern` on `input` under `op`. `Err` when the engine is unavailable or rejects
    /// the pattern.
    pub fn run(
        &mut self,
        flavor: Flavor,
        pattern: &str,
        op: Op,
        input: &str,
    ) -> Result<bool, String> {
        if flavor == Flavor::Rust {
            let compiled = match op {
                Op::Search => regex::Regex::new(pattern),
                Op::FullMatch => regex::Regex::new(&format!(r"\A(?:{pattern})\z")),
            };
            return compiled.map(|r| r.is_match(input)).map_err(|e| {
                format!(
                    "rejected the pattern: {}",
                    e.to_string().lines().last().unwrap_or("")
                )
            });
        }
        let k = kind(flavor).ok_or_else(|| format!("no oracle for {flavor}"))?;
        let oop = match op {
            Op::FullMatch => OOp::Full,
            Op::Search => OOp::Search,
        };
        let engine = self.engine(k)?;
        let out = if flavor == Flavor::JavaScript {
            let (src, flags) = js_literal(pattern);
            engine.run_with_flags(src, flags, oop, &[input])
        } else {
            engine.run(pattern, oop, &[input])
        };
        match out.map_err(|e| e.to_string())? {
            Outcome::Matched(v) => v.first().copied().ok_or_else(|| "no result".to_string()),
            Outcome::Rejected(e) => Err(format!("rejected the pattern: {e}")),
        }
    }
}
