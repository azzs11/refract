//! Command implementations. Exit codes: 0 success, 1 a pattern was rejected or a check failed,
//! 2 usage or internal error.

use anyhow::{Result, anyhow};
use refract_core::automata::{alphabet_for, minimal_dfa};
use refract_core::regular::lower;
use refract_core::{Budget, Engine, Flavor, Op, parse as parse_pattern};
use serde_json::json;
use std::process::ExitCode;

fn budget(max_states: Option<usize>) -> Budget {
    let mut b = Budget::default();
    if let Some(n) = max_states {
        b.max_dfa_states = n;
        b.max_terms = n.saturating_mul(10);
    }
    b.start()
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
