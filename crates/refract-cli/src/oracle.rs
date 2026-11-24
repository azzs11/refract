//! Real-engine replay (plan §5.10). Every "different" verdict is replayed on the real engines
//! before it is shown as confirmed; when an engine is not available the verdict is labelled
//! "predicted" and the report says why.
//!
//! - Rust: in-process (the `regex` crate itself, ADR-0005).
//! - Python, JavaScript: native `python3` / `node` when installed, else the pinned Docker image.
//! - Go, Java: pinned Docker images (`docker build -t refract-oracle-go oracle/go`, same for java).

use refract_core::{Flavor, Op};
use refract_oracle::{Engine, EngineKind, Mode, Op as OOp, Outcome, Timing};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// V8's opt-in linear engine, used after too many backtracks (ReDoS lab).
pub const V8_LINEAR: &str = "--enable-experimental-regexp-engine-on-excessive-backtracks";

/// Long-lived engine processes, started on first use. The key's flag selects V8's linear
/// fallback.
#[derive(Default)]
pub struct Oracles {
    engines: HashMap<(EngineKind, bool), Result<Engine, String>>,
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
        self.engine_with(k, false)
    }

    fn engine_with(&mut self, k: EngineKind, v8_linear: bool) -> Result<&mut Engine, String> {
        // A runner killed after a timeout is replaced.
        if let Some(Ok(e)) = self.engines.get(&(k, v8_linear))
            && !e.is_alive()
        {
            self.engines.remove(&(k, v8_linear));
        }
        let args: &[&str] = if v8_linear { &[V8_LINEAR] } else { &[] };
        let slot = self.engines.entry((k, v8_linear)).or_insert_with(|| {
            Engine::spawn_with(k, Mode::from_env(), args)
                .or_else(|_| Engine::spawn_with(k, Mode::Docker, args))
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

    /// Engine label for the ReDoS lab, naming the option when V8's fallback is on.
    pub fn label_with(&mut self, flavor: Flavor, v8_linear: bool) -> String {
        if !v8_linear {
            return self.label(flavor);
        }
        match self.engine_with(EngineKind::Node, true) {
            Ok(e) => format!("node {} {V8_LINEAR}", e.version()),
            Err(e) => e,
        }
    }

    /// Times one match (min of `reps` runs). `Ok(None)` = timed out.
    #[allow(clippy::too_many_arguments)]
    pub fn time(
        &mut self,
        flavor: Flavor,
        v8_linear: bool,
        pattern: &str,
        op: Op,
        input: &str,
        timeout: Duration,
        reps: usize,
    ) -> Result<Option<Duration>, String> {
        let full = op == Op::FullMatch;
        if flavor == Flavor::Rust {
            let src = if full {
                format!(r"\A(?:{pattern})\z")
            } else {
                pattern.to_string()
            };
            let r = regex::Regex::new(&src).map_err(|e| format!("rejected the pattern: {e}"))?;
            let mut best = Duration::MAX;
            for _ in 0..reps.max(1) {
                let t0 = Instant::now();
                std::hint::black_box(r.is_match(input));
                best = best.min(t0.elapsed());
            }
            return Ok(Some(best));
        }
        let k = kind(flavor).ok_or_else(|| format!("no oracle for {flavor}"))?;
        let (src, flags) = if flavor == Flavor::JavaScript {
            js_literal(pattern)
        } else {
            (pattern, "")
        };
        let mut best: Option<Duration> = None;
        for _ in 0..reps.max(1) {
            let engine = self.engine_with(k, v8_linear)?;
            match engine
                .time(src, flags, full, input, timeout)
                .map_err(|e| e.to_string())?
            {
                Timing::Took(t) => best = Some(best.map_or(t, |b| b.min(t))),
                Timing::TimedOut => return Ok(None),
                Timing::Rejected(e) => return Err(format!("rejected the pattern: {e}")),
            }
        }
        Ok(best)
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
