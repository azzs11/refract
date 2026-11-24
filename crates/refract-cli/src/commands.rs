//! Command implementations. Exit codes: 0 success, 1 a pattern was rejected or a check failed,
//! 2 usage or internal error.

use crate::oracle;
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

fn replay(sides: [(&Spec, bool); 2], op: Op, input: &str) -> Replay {
    let mut notes = Vec::new();
    let mut predicted = false;
    let mut contradicted = false;
    for (name, (spec, expect)) in ["a", "b"].iter().zip(sides) {
        match oracle::run(spec.flavor, &spec.pattern, op, input) {
            Some(Ok(got)) => {
                let label = oracle::engine_label(spec.flavor).unwrap_or("?");
                let word = |m: bool| if m { "match" } else { "no match" };
                if got == expect {
                    notes.push(format!("{name}: {} on {label}", word(got)));
                } else {
                    contradicted = true;
                    notes.push(format!(
                        "{name}: model predicted {} but {label} returned {}",
                        word(expect),
                        word(got)
                    ));
                }
            }
            Some(Err(e)) => {
                predicted = true;
                notes.push(format!(
                    "{name}: real engine could not run the pattern: {e}"
                ));
            }
            None => {
                predicted = true;
                notes.push(format!("{name}: no oracle for {} yet (P3)", spec.flavor));
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
    let report = equivalent(&x, &y, engine, &budget).map_err(|e| anyhow!("{}", e.message))?;

    let (code, replayed) = match &report.verdict {
        Verdict::Equivalent => (ExitCode::SUCCESS, None),
        Verdict::Different(w) => {
            let left = w.accepted_by == Side::Left;
            let r = replay([(a, left), (b, !left)], op, &w.text);
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
