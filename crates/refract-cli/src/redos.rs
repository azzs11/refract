//! `refract redos`: predict the worst-case growth per engine, then measure it on the real
//! engine (plan §5.7, ADR-0012). A prediction is only "confirmed" when the measured curve has
//! the same class.

use crate::oracle::Oracles;
use anyhow::{Result, anyhow};
use refract_core::equiv::escape;
use refract_core::ir::{Node, NodeKind};
use refract_core::redos::{Attack, Growth, Prediction, analyse};
use refract_core::{Budget, Flavor, Op, parse};
use serde_json::json;
use std::process::ExitCode;
use std::time::Duration;

/// An engine as the ReDoS lab sees it: flavour plus engine options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub flavor: Flavor,
    /// V8 with its linear fallback (`--enable-experimental-regexp-engine-on-excessive-backtracks`).
    pub v8_linear: bool,
}

impl std::str::FromStr for Target {
    type Err = String;
    fn from_str(s: &str) -> Result<Target, String> {
        if s == "js-linear" {
            return Ok(Target {
                flavor: Flavor::JavaScript,
                v8_linear: true,
            });
        }
        Ok(Target {
            flavor: s.parse()?,
            v8_linear: false,
        })
    }
}

impl Target {
    fn name(self) -> String {
        if self.v8_linear {
            "js-linear".into()
        } else {
            self.flavor.name().into()
        }
    }
}

#[derive(Clone, Debug)]
struct Point {
    n: usize,
    len: usize,
    secs: f64,
    timed_out: bool,
}

/// Measured class of a curve.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Measured {
    Linear,
    Polynomial(f64),
    Exponential(f64),
}

impl Measured {
    fn name(self) -> String {
        match self {
            Measured::Linear => "linear".into(),
            Measured::Polynomial(k) if k.is_nan() => "super-linear (timed out)".into(),
            Measured::Polynomial(k) => format!("polynomial ~n^{k:.1}"),
            Measured::Exponential(b) if b.is_nan() => "exponential (timed out)".into(),
            Measured::Exponential(b) => format!("exponential x{b:.2}/pump"),
        }
    }
    fn same_class(self, g: Growth) -> bool {
        matches!(
            (self, g),
            (Measured::Linear, Growth::Linear)
                | (Measured::Polynomial(_), Growth::Polynomial(_))
                | (Measured::Exponential(_), Growth::Exponential)
        ) || matches!((self, g), (Measured::Exponential(_), Growth::Polynomial(d)) if d >= 5)
    }
    fn super_linear(self) -> bool {
        !matches!(self, Measured::Linear)
    }
}

const FLOOR: f64 = 2e-4; // below this, timer and call overhead dominate
const STOP: f64 = 0.25;

/// Engines whose matcher never backtracks.
fn linear_engine(t: Target) -> Option<&'static str> {
    match t.flavor {
        Flavor::Go => Some("RE2-style automaton engine: linear time by design"),
        Flavor::Rust => Some("finite-automaton engine: linear time by design"),
        _ => None,
    }
}

fn any(n: &Node, f: &dyn Fn(&NodeKind) -> bool) -> bool {
    if f(&n.kind) {
        return true;
    }
    match &n.kind {
        NodeKind::Concat(xs) | NodeKind::Alt(xs) => xs.iter().any(|x| any(x, f)),
        NodeKind::Repeat(r) => any(&r.sub, f),
        NodeKind::Group(g) => any(&g.sub, f),
        NodeKind::Look(l) => any(&l.sub, f),
        NodeKind::Conditional(c) => any(&c.yes, f) || c.no.as_ref().is_some_and(|x| any(x, f)),
        _ => false,
    }
}

/// When does V8 14.6 switch to its linear engine? Measured (docs/findings.md): not for
/// back-references, look-around, or the `i`, `u`, `v` flags.
fn v8_fallback_applies(pattern: &str, root: &Node) -> bool {
    let flags = pattern
        .strip_prefix('/')
        .and_then(|r| r.rfind('/').map(|e| &r[e + 1..]))
        .unwrap_or("");
    !flags.contains(['i', 'u', 'v'])
        && !any(root, &|k| {
            matches!(k, NodeKind::Backref(..) | NodeKind::Look(_))
        })
}

fn measure(
    oracles: &mut Oracles,
    t: Target,
    pattern: &str,
    op: Op,
    attack: &Attack,
    additive: bool,
    timeout: Duration,
) -> Result<Vec<Point>, String> {
    let step = attack.step_len().max(1);
    let ns: Vec<usize> = if additive {
        (1..=40).map(|i| 2 * i).collect()
    } else {
        (4..=17).map(|k| ((1usize << k) / step).max(1)).collect()
    };
    let mut out = Vec::new();
    for n in ns {
        let input = attack.build(n);
        let len = input.chars().count();
        let got = oracles.time(t.flavor, t.v8_linear, pattern, op, &input, timeout, 3)?;
        match got {
            Some(d) => {
                let secs = d.as_secs_f64();
                out.push(Point {
                    n,
                    len,
                    secs,
                    timed_out: false,
                });
                if secs > STOP {
                    break;
                }
            }
            None => {
                out.push(Point {
                    n,
                    len,
                    secs: timeout.as_secs_f64(),
                    timed_out: true,
                });
                break;
            }
        }
    }
    Ok(out)
}

fn slope(xs: &[f64], ys: &[f64]) -> f64 {
    let n = xs.len() as f64;
    let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (x, y) in xs.iter().zip(ys) {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx) * (x - mx);
    }
    if sxx == 0.0 { 0.0 } else { sxy / sxx }
}

fn classify(points: &[Point], additive: bool) -> Measured {
    let informative: Vec<&Point> = points.iter().filter(|p| p.secs >= FLOOR).collect();
    let timed_out = points.last().is_some_and(|p| p.timed_out);
    if informative.len() < 3 {
        if timed_out {
            // Fast, then suddenly over the timeout.
            return if additive || points.last().is_some_and(|p| p.len <= 256) {
                Measured::Exponential(f64::NAN)
            } else {
                Measured::Polynomial(f64::NAN)
            };
        }
        return Measured::Linear;
    }
    let tail = &informative[informative.len().saturating_sub(5)..];
    let ln = |v: f64| v.ln();
    let ys: Vec<f64> = tail.iter().map(|p| ln(p.secs)).collect();
    let log_n: Vec<f64> = tail.iter().map(|p| ln(p.n as f64)).collect();
    let k = slope(&log_n, &ys);
    if additive {
        let lin_n: Vec<f64> = tail.iter().map(|p| p.n as f64).collect();
        let base = slope(&lin_n, &ys).exp();
        // Polynomial growth flattens per added pump as n grows; exponential does not.
        if base >= 1.25 && k >= 3.0 {
            return Measured::Exponential(base);
        }
    } else if k > 6.0 {
        return Measured::Exponential(f64::NAN);
    }
    if k >= 1.5 {
        Measured::Polynomial(k)
    } else {
        Measured::Linear
    }
}

struct Row {
    target: Target,
    engine: String,
    expected: Option<Growth>,
    expected_why: String,
    measured: Option<Measured>,
    points: Vec<Point>,
    status: String,
    analysis: Option<Prediction>,
}

fn fmt_secs(s: f64) -> String {
    if s >= 1.0 {
        format!("{s:.2}s")
    } else if s >= 1e-3 {
        format!("{:.1}ms", s * 1e3)
    } else {
        format!("{:.0}µs", s * 1e6)
    }
}

/// Parses `--attack '{"prefix": "", "pumps": ["a"], "suffix": "!", "leads": [""]}'`.
pub fn parse_attack(s: &str) -> Result<Attack, String> {
    let v: serde_json::Value = serde_json::from_str(s).map_err(|e| e.to_string())?;
    let text = |k: &str| v[k].as_str().unwrap_or("").to_string();
    let pumps: Vec<String> = v["pumps"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if pumps.is_empty() || pumps.iter().any(String::is_empty) {
        return Err("an attack needs at least one non-empty pump".into());
    }
    let leads: Vec<String> = v["leads"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Ok(Attack {
        prefix: text("prefix"),
        pumps,
        suffix: text("suffix"),
        leads,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn redos(
    from: Flavor,
    pattern: &str,
    targets: &[Target],
    op: Op,
    timeout: Duration,
    no_measure: bool,
    given: Option<Attack>,
    as_json: bool,
) -> Result<ExitCode> {
    // The pattern must be valid where it was written.
    parse(from, pattern).map_err(|e| anyhow!("{from} pattern:\n{}", e.render(pattern)))?;
    let mut oracles = Oracles::default();
    let mut rows = Vec::new();
    for &t in targets {
        // Labels need a running engine: only when measuring.
        let engine = if no_measure {
            "(not measured)".to_string()
        } else {
            oracles.label_with(t.flavor, t.v8_linear)
        };
        let mut row = Row {
            target: t,
            engine,
            expected: None,
            expected_why: String::new(),
            measured: None,
            points: Vec::new(),
            status: String::new(),
            analysis: None,
        };
        let ir = match parse(t.flavor, pattern) {
            Ok(ir) => ir,
            Err(e) => {
                row.status = format!("not analysed: {}", e.message);
                rows.push(row);
                continue;
            }
        };
        let budget = Budget::default().start();
        let pred = match analyse(&ir, op, &budget) {
            Ok(p) => p,
            Err(e) => {
                row.status = format!("not analysed: {}", e.message);
                rows.push(row);
                continue;
            }
        };
        let (expected, why) = if let Some(why) = linear_engine(t) {
            (Growth::Linear, why.to_string())
        } else if t.v8_linear
            && pred.growth != Growth::Linear
            && v8_fallback_applies(pattern, &ir.root)
        {
            (
                Growth::Linear,
                "V8 switches to its linear engine after excessive backtracking".into(),
            )
        } else {
            (
                pred.growth,
                "backtracking engine: follows the NFA's ambiguity".into(),
            )
        };
        row.expected = Some(expected);
        row.expected_why = why;
        row.analysis = Some(pred);
        rows.push(row);
    }
    // An engine predicted linear is still measured, with the attack found for the others: that
    // is the evidence it stays linear.
    let shared: Option<Attack> = rows
        .iter()
        .find_map(|r| r.analysis.as_ref().and_then(|a| a.attack.clone()));
    for row in rows.iter_mut() {
        let (Some(pred), Some(expected)) = (row.analysis.as_ref(), row.expected) else {
            continue;
        };
        let t = row.target;
        let attack = given
            .clone()
            .or_else(|| pred.attack.clone())
            .or_else(|| shared.clone());
        match (&attack, no_measure) {
            (None, _) => row.status = "predicted (no attack string makes it backtrack)".into(),
            (Some(_), true) => row.status = "predicted (not measured)".into(),
            (Some(attack), false) => {
                let mut additive = expected == Growth::Exponential || given.is_some();
                let mut run = measure(&mut oracles, t, pattern, op, attack, additive, timeout);
                // A given attack of unknown growth: small steps first, then doubling.
                if given.is_some()
                    && let Ok(points) = &run
                    && !matches!(classify(points, true), Measured::Exponential(_))
                {
                    additive = false;
                    run = measure(&mut oracles, t, pattern, op, attack, false, timeout);
                }
                match run {
                    Err(e) => row.status = format!("predicted ({e})"),
                    Ok(points) => {
                        let m = classify(&points, additive);
                        row.status = if m.same_class(expected) {
                            "confirmed".into()
                        } else if m.super_linear() {
                            "CONTRADICTED: slower than predicted".into()
                        } else {
                            "not confirmed: the engine stays linear (it optimises this case)".into()
                        };
                        row.measured = Some(m);
                        row.points = points;
                    }
                }
            }
        }
    }

    let vulnerable = rows
        .iter()
        .any(|r| r.status == "confirmed" && r.measured.is_some_and(|m| m.super_linear()));
    let contradicted = rows.iter().any(|r| r.status.starts_with("CONTRADICTED"));

    if as_json {
        let engines: Vec<_> = rows
            .iter()
            .map(|r| {
                let a = r.analysis.as_ref();
                json!({
                    "engine": r.target.name(),
                    "engine_version": r.engine,
                    "ambiguity": a.map(|p| p.growth.name()),
                    "reason": a.map(|p| p.reason.clone()),
                    "span": a.and_then(|p| p.span).map(|s| [s.start, s.end]),
                    "attack": a.and_then(|p| p.attack.as_ref()).map(|at| json!({
                        "prefix": at.prefix, "pumps": at.pumps, "suffix": at.suffix,
                    })),
                    "predicted": r.expected.map(|g| g.name()),
                    "predicted_why": r.expected_why,
                    "measured": r.measured.map(|m| m.name()),
                    "points": r.points.iter().map(|p| json!({
                        "n": p.n, "length": p.len, "seconds": p.secs, "timed_out": p.timed_out,
                    })).collect::<Vec<_>>(),
                    "status": r.status,
                })
            })
            .collect();
        let out = json!({
            "pattern": pattern,
            "written": from.name(),
            "operation": op.name(),
            "timeout_seconds": timeout.as_secs_f64(),
            "attack_given": given.as_ref().map(|a| json!({"prefix": a.prefix, "pumps": a.pumps, "suffix": a.suffix})),
            "engines": engines,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("pattern    {pattern}");
        println!("written    {}", from.name());
        println!("operation  {}", op.name());
        let own = rows
            .iter()
            .find(|r| r.target.flavor == from && !r.target.v8_linear)
            .or(rows.first())
            .and_then(|r| r.analysis.as_ref());
        if let Some(a) = own {
            let at = a
                .span
                .map(|s| format!(" (at {}..{})", s.start, s.end))
                .unwrap_or_default();
            println!("ambiguity  {}{at}: {}", a.growth.name(), a.reason);
            if let Some(at) = &a.attack {
                let mut pumps: Vec<String> = at
                    .pumps
                    .iter()
                    .take(3)
                    .map(|p| format!("{}×n", quote(p)))
                    .collect();
                if at.pumps.len() > 3 {
                    pumps.push(format!("… ({} pumps)", at.pumps.len()));
                }
                println!(
                    "attack     {} + {} + {}",
                    quote(&at.prefix),
                    pumps.join(" + "),
                    quote(&at.suffix)
                );
            }
        }
        println!();
        println!(
            "{:<10} {:<22} {:<26} status",
            "engine", "predicted", "measured"
        );
        for r in &rows {
            println!(
                "{:<10} {:<22} {:<26} {}",
                r.target.name(),
                r.expected.map(|g| g.name()).unwrap_or_else(|| "-".into()),
                r.measured.map(|m| m.name()).unwrap_or_else(|| "-".into()),
                r.status
            );
            if !no_measure {
                println!("{:<10} {}", "", r.engine);
            }
            if !r.points.is_empty() {
                let pts: Vec<String> = r
                    .points
                    .iter()
                    .map(|p| {
                        format!(
                            "n={} {}{}",
                            p.n,
                            if p.timed_out { ">" } else { "" },
                            fmt_secs(p.secs)
                        )
                    })
                    .collect();
                println!("{:<10} {}", "", pts.join("  "));
            }
        }
        println!();
        if no_measure {
            println!("Static prediction only; run without --no-measure to time it on the engines.");
        } else {
            println!(
                "\"confirmed\" = the measured curve has the predicted class; times are wall clock inside each engine's runner (timeout {}).",
                fmt_secs(timeout.as_secs_f64())
            );
        }
    }
    Ok(ExitCode::from(if contradicted {
        3
    } else if vulnerable {
        1
    } else {
        0
    }))
}

fn quote(s: &str) -> String {
    escape(s)
}
