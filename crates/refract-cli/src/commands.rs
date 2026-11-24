//! Command implementations. Exit codes: 0 success, 1 a pattern was rejected or a check failed,
//! 2 usage or internal error.

use crate::oracle::Oracles;
use anyhow::{Result, anyhow};
use refract_core::automata::{alphabet_for, minimal_dfa};
use refract_core::equiv::{Side, Verdict, equivalent, escape};
use refract_core::regular::lower;
use refract_core::{Budget, Engine, Flavor, Op, parse as parse_pattern};
use serde_json::json;
use std::process::ExitCode;

/// A pattern with its flavour.
pub struct Spec {
    pub flavor: Flavor,
    pub pattern: String,
}

/// `rust:a+` → (rust, `a+`). A prefix that is not a flavour name is part of the pattern, so
/// `(?:a)` or `a:b` are read whole.
pub fn split_flavor(arg: &str, default: Flavor) -> Spec {
    if let Some((head, rest)) = arg.split_once(':')
        && let Ok(flavor) = head.parse::<Flavor>()
    {
        return Spec {
            flavor,
            pattern: rest.to_string(),
        };
    }
    Spec {
        flavor: default,
        pattern: arg.to_string(),
    }
}

fn budget(max_states: Option<usize>) -> Budget {
    let mut b = Budget::default();
    if let Some(n) = max_states {
        b.max_dfa_states = n;
        b.max_terms = n.saturating_mul(10);
    }
    b.start()
}

/// Outcome of replaying a witness on the real engines.
enum Replay {
    /// Both real engines did what the model predicted.
    Confirmed(Vec<String>),
    /// No oracle yet for at least one side: the verdict is a prediction.
    Predicted(Vec<String>),
    /// A real engine contradicted the model. Never hidden.
    Contradicted(Vec<String>),
}

fn replay(oracles: &mut Oracles, sides: [(&Spec, bool); 2], op: Op, input: &str) -> Replay {
    let mut notes = Vec::new();
    let mut predicted = false;
    let mut contradicted = false;
    for (name, (spec, expect)) in ["a", "b"].iter().zip(sides) {
        let label = oracles.label(spec.flavor);
        let word = |m: bool| if m { "match" } else { "no match" };
        match oracles.run(spec.flavor, &spec.pattern, op, input) {
            Ok(got) if got == expect => notes.push(format!("{name}: {} on {label}", word(got))),
            Ok(got) => {
                contradicted = true;
                notes.push(format!(
                    "{name}: model predicted {} but {label} returned {}",
                    word(expect),
                    word(got)
                ));
            }
            Err(e) => {
                predicted = true;
                notes.push(format!("{name}: not replayed: {e}"));
            }
        }
    }
    if contradicted {
        Replay::Contradicted(notes)
    } else if predicted {
        Replay::Predicted(notes)
    } else {
        Replay::Confirmed(notes)
    }
}

pub fn equiv(
    a: &Spec,
    b: &Spec,
    op: Op,
    engine: Engine,
    max_states: Option<usize>,
    as_json: bool,
) -> Result<ExitCode> {
    let budget = budget(max_states);
    let lowered = |s: &Spec| -> Result<refract_core::Re> {
        let re = parse_pattern(s.flavor, &s.pattern)
            .map_err(|e| anyhow!("{} pattern:\n{}", s.flavor, e.render(&s.pattern)))?;
        lower(&re, op, &budget).map_err(|e| anyhow!("{}", e.render(&s.pattern)))
    };
    let (x, y) = (lowered(a)?, lowered(b)?);
    let parse_dom = |s: &Spec| parse_pattern(s.flavor, &s.pattern).map(|r| r.domain).ok();
    let d = match (parse_dom(a), parse_dom(b)) {
        (Some(refract_core::ir::Domain::Bmp), _) | (_, Some(refract_core::ir::Domain::Bmp)) => {
            refract_core::ir::Domain::Bmp
        }
        _ => refract_core::ir::Domain::Scalars,
    };
    let (x, y) = (
        refract_core::regular::restrict(x, d),
        refract_core::regular::restrict(y, d),
    );
    let report = equivalent(&x, &y, engine, &budget).map_err(|e| anyhow!("{}", e.message))?;

    let (code, replayed) = match &report.verdict {
        Verdict::Equivalent => (ExitCode::SUCCESS, None),
        Verdict::Different(w) => {
            let left = w.accepted_by == Side::Left;
            let mut oracles = Oracles::default();
            let r = replay(&mut oracles, [(a, left), (b, !left)], op, &w.text);
            let code = match r {
                Replay::Contradicted(_) => ExitCode::from(3),
                _ => ExitCode::from(1),
            };
            (code, Some(r))
        }
    };

    if as_json {
        let (status, notes) = match &replayed {
            None => ("proven", vec![]),
            Some(Replay::Confirmed(n)) => ("confirmed", n.clone()),
            Some(Replay::Predicted(n)) => ("predicted", n.clone()),
            Some(Replay::Contradicted(n)) => ("contradicted", n.clone()),
        };
        let witness = match &report.verdict {
            Verdict::Different(w) => json!({
                "text": w.text,
                "escaped": escape(&w.text),
                "length": w.text.chars().count(),
                "accepted_by": if w.accepted_by == Side::Left { "a" } else { "b" },
            }),
            Verdict::Equivalent => serde_json::Value::Null,
        };
        let out = json!({
            "operation": op.name(),
            "a": { "flavor": a.flavor.name(), "pattern": a.pattern },
            "b": { "flavor": b.flavor.name(), "pattern": b.pattern },
            "equivalent": matches!(report.verdict, Verdict::Equivalent),
            "status": status,
            "witness": witness,
            "replay": notes,
            "product_states": report.product_states,
            "minterms": report.minterms,
            "engine": format!("{:?}", report.engine),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(code);
    }

    println!("operation  {}", op.name());
    println!("a          {:<6} {}", a.flavor.name(), a.pattern);
    println!("b          {:<6} {}", b.flavor.name(), b.pattern);
    match &report.verdict {
        Verdict::Equivalent => {
            println!(
                "verdict    EQUIVALENT (proven: no distinguishing string; explored {} product \
                 states over {} minterms)",
                report.product_states, report.minterms
            );
        }
        Verdict::Different(w) => {
            let (yes, no) = if w.accepted_by == Side::Left {
                ("a", "b")
            } else {
                ("b", "a")
            };
            println!("verdict    DIFFERENT");
            println!(
                "witness    {}  (length {}, shortest possible)",
                escape(&w.text),
                w.text.chars().count()
            );
            println!("           {yes} matches it, {no} does not");
            match replayed {
                Some(Replay::Confirmed(notes)) => {
                    println!("status     confirmed on the real engines");
                    notes.iter().for_each(|n| println!("           {n}"));
                }
                Some(Replay::Predicted(notes)) => {
                    println!("status     predicted (not replayed on every real engine)");
                    notes.iter().for_each(|n| println!("           {n}"));
                }
                Some(Replay::Contradicted(notes)) => {
                    println!("status     CONTRADICTED by a real engine: this is a Refract bug");
                    notes.iter().for_each(|n| println!("           {n}"));
                }
                None => {}
            }
        }
    }
    Ok(code)
}

pub fn dfa(
    flavor: Flavor,
    pattern: &str,
    op: Op,
    engine: Engine,
    dot: bool,
    max_states: Option<usize>,
) -> Result<ExitCode> {
    let budget = budget(max_states);
    let re = parse_pattern(flavor, pattern).map_err(|e| anyhow!("{}", e.render(pattern)))?;
    let term = lower(&re, op, &budget).map_err(|e| anyhow!("{}", e.render(pattern)))?;
    let alphabet = alphabet_for(&[&term]);
    let m = minimal_dfa(&term, &alphabet, engine, &budget).map_err(|e| anyhow!("{}", e.message))?;
    if dot {
        print!("{}", m.to_dot());
        return Ok(ExitCode::SUCCESS);
    }
    println!("operation  {}", op.name());
    println!("states     {} (minimal, complete)", m.len());
    println!("minterms   {}", alphabet.len());
    for i in 0..alphabet.len() {
        println!("  c{i:<3} {}", alphabet.class(i));
    }
    println!("transitions (state: class -> target; * = accepting, > = start)");
    for q in 0..m.len() {
        let mark = format!(
            "{}{}",
            if q == m.start { ">" } else { " " },
            if m.accepting[q] { "*" } else { " " }
        );
        let row: Vec<String> = (0..m.k())
            .map(|c| format!("c{c}->{}", m.next(q, c)))
            .collect();
        println!("  {mark}{q:<3} {}", row.join("  "));
    }
    Ok(ExitCode::SUCCESS)
}

pub fn parse(flavor: Flavor, pattern: &str, compact: bool, as_json: bool) -> Result<ExitCode> {
    match parse_pattern(flavor, pattern) {
        Ok(re) => {
            if as_json {
                let out = json!({
                    "flavor": flavor.name(),
                    "pattern": pattern,
                    "ir": re.root.to_string(),
                    "captures": re.captures,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else if compact {
                println!("{}", re.root);
            } else {
                print!("{}", re.root.pretty());
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            if as_json {
                let out = json!({
                    "flavor": flavor.name(),
                    "pattern": pattern,
                    "error": {
                        "kind": format!("{:?}", e.kind),
                        "message": e.message,
                        "span": [e.span.start, e.span.end],
                    }
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                eprintln!("{}", e.render(pattern));
            }
            Ok(ExitCode::from(1))
        }
    }
}

pub fn flavors() -> Result<ExitCode> {
    for f in Flavor::ALL {
        match f.profile() {
            Some(p) => {
                println!("{:<7} implemented  {}", f.name(), p.engine);
                println!("        .  excludes {}", p.dot_excludes);
                println!("        $  {}", p.dollar);
                println!(
                    "        \\w \\d \\s Unicode by default: {}",
                    p.unicode_classes
                );
                println!("        (?i) {}", p.case_folding);
                println!("        spec: {}", p.spec);
            }
            None => println!("{:<7} planned", f.name()),
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// One row of a cross-engine report.
struct Row {
    flavor: Flavor,
    verdict: &'static str,
    witness: Option<String>,
    /// (source matches, target matches) on the witness, as predicted.
    predicted: Option<(bool, bool)>,
    status: String,
    note: String,
}

/// `refract refract`: one pattern, every engine, the shortest disagreement each, replayed.
pub fn refract(
    from: Flavor,
    pattern: &str,
    targets: &[Flavor],
    op: Op,
    as_json: bool,
) -> Result<ExitCode> {
    let budget = budget(None);
    let src_ir = parse_pattern(from, pattern)
        .map_err(|e| anyhow!("{from} pattern:\n{}", e.render(pattern)))?;
    let src = lower(&src_ir, op, &budget).map_err(|e| anyhow!("{}", e.render(pattern)))?;
    let mut oracles = Oracles::default();
    let word = |m: bool| if m { "match" } else { "no match" };
    let mut rows = Vec::new();
    let mut worst = 0u8; // 0 same, 1 different/rejected, 3 contradicted
    for &t in targets {
        if t == from {
            continue;
        }
        let row = match parse_pattern(t, pattern) {
            Err(e) if e.kind == refract_core::ErrorKind::Syntax => {
                // Confirm the rejection on the real engine.
                let status = match oracles.run(t, pattern, op, "") {
                    Err(m) if m.starts_with("rejected") => "confirmed".to_string(),
                    Err(m) => format!("predicted ({m})"),
                    Ok(_) => {
                        worst = 3;
                        "CONTRADICTED: the real engine accepts it".to_string()
                    }
                };
                worst = worst.max(1);
                Row {
                    flavor: t,
                    verdict: "REJECTS",
                    witness: None,
                    predicted: None,
                    status,
                    note: e.message,
                }
            }
            Err(e) => Row {
                flavor: t,
                verdict: "NOT ANALYSED",
                witness: None,
                predicted: None,
                status: "-".into(),
                note: e.message,
            },
            Ok(ir) => match lower(&ir, op, &budget).and_then(|re| {
                // Compare over the common domain so the witness is valid for both engines.
                let d = refract_core::regular::common_domain(&src_ir, &ir);
                let x = refract_core::regular::restrict(src.clone(), d);
                let y = refract_core::regular::restrict(re, d);
                equivalent(&x, &y, Engine::Derivative, &budget)
            }) {
                Err(e) => Row {
                    flavor: t,
                    verdict: "NOT ANALYSED",
                    witness: None,
                    predicted: None,
                    status: "-".into(),
                    note: e.message,
                },
                Ok(rep) => match rep.verdict {
                    Verdict::Equivalent => {
                        let mut note = format!("proven over {} product states", rep.product_states);
                        if refract_core::regular::common_domain(&src_ir, &ir)
                            == refract_core::ir::Domain::Bmp
                        {
                            note.push_str("; JS without /u: BMP haystacks only");
                        }
                        Row {
                            flavor: t,
                            verdict: "SAME",
                            witness: None,
                            predicted: None,
                            status: "proven".into(),
                            note,
                        }
                    }
                    Verdict::Different(w) => {
                        worst = worst.max(1);
                        let src_m = w.accepted_by == Side::Left;
                        let (a, b) = (
                            oracles.run(from, pattern, op, &w.text),
                            oracles.run(t, pattern, op, &w.text),
                        );
                        let status = match (a, b) {
                            (Ok(x), Ok(y)) if x == src_m && y == !src_m => "confirmed".to_string(),
                            (Ok(x), Ok(y)) => {
                                worst = 3;
                                format!(
                                    "CONTRADICTED: engines gave {from} {}, {t} {}",
                                    word(x),
                                    word(y)
                                )
                            }
                            (Err(e), _) | (_, Err(e)) => format!("predicted ({e})"),
                        };
                        Row {
                            flavor: t,
                            verdict: "DIFFERS",
                            witness: Some(w.text),
                            predicted: Some((src_m, !src_m)),
                            status,
                            note: String::new(),
                        }
                    }
                },
            },
        };
        rows.push(row);
    }
    if as_json {
        let engines: Vec<_> = rows
            .iter()
            .map(|r| {
                json!({
                    "flavor": r.flavor.name(),
                    "engine": oracles.label(r.flavor),
                    "verdict": r.verdict,
                    "witness": r.witness.as_ref().map(|w| json!({"text": w, "escaped": escape(w)})),
                    "source_matches": r.predicted.map(|p| p.0),
                    "target_matches": r.predicted.map(|p| p.1),
                    "status": r.status,
                    "note": r.note,
                })
            })
            .collect();
        let out = json!({
            "pattern": pattern,
            "source": from.name(),
            "source_engine": oracles.label(from),
            "operation": op.name(),
            "engines": engines,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("pattern    {pattern}");
        println!("written    {}  ({})", from.name(), oracles.label(from));
        println!("operation  {}", op.name());
        println!();
        println!(
            "{:<8} {:<13} {:<22} {:<22} status",
            "engine",
            "verdict",
            "shortest witness",
            format!("{} / engine", from.name())
        );
        for r in &rows {
            let w = r.witness.as_deref().map(escape).unwrap_or_default();
            let p = r
                .predicted
                .map(|(a, b)| format!("{} / {}", word(a), word(b)))
                .unwrap_or_default();
            println!(
                "{:<8} {:<13} {:<22} {:<22} {}",
                r.flavor.name(),
                r.verdict,
                w,
                p,
                r.status
            );
            if !r.note.is_empty() {
                println!("{:<8} {}", "", r.note);
            }
        }
    }
    Ok(ExitCode::from(worst))
}
