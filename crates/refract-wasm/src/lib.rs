//! WebAssembly façade over refract-core (spec/web.md). Every function takes plain strings and
//! returns a JSON string, so the JavaScript side needs no generated types. Analyses run with a
//! browser budget; the page runs them in a Web Worker.

#![forbid(unsafe_code)]

use refract_core::equiv::{Side, Verdict, compare, escape};
use refract_core::{Budget, Error, Flavor, Op, parse as parse_pattern};
use serde_json::{Value, json};
use std::time::Duration;
use wasm_bindgen::prelude::*;

/// The flavours with a front-end, in display order.
pub const FLAVORS: [Flavor; 5] = [
    Flavor::JavaScript,
    Flavor::Python,
    Flavor::Go,
    Flavor::Java,
    Flavor::Rust,
];

fn budget() -> Budget {
    let mut b = Budget::small();
    b.max_dfa_states = 5_000;
    b.max_terms = 50_000;
    b.time_limit = Some(Duration::from_secs(3));
    b.start()
}

fn flavor(name: &str) -> Result<Flavor, String> {
    name.parse::<Flavor>()
}

fn op(name: &str) -> Result<Op, String> {
    name.parse::<Op>()
}

fn error_json(e: &Error, pattern: &str) -> Value {
    json!({
        "kind": format!("{:?}", e.kind).to_lowercase(),
        "message": e.message,
        "span": [e.span.start, e.span.end],
        "rendered": e.render(pattern),
    })
}

fn out(v: Value) -> String {
    v.to_string()
}

/// The analysable flavours, as `["js", "python", ...]`.
#[wasm_bindgen]
pub fn flavors() -> String {
    out(json!(FLAVORS.iter().map(|f| f.name()).collect::<Vec<_>>()))
}

/// Parse: `{ok, ir}` or `{ok: false, error}`.
#[wasm_bindgen]
pub fn parse(flavor_name: &str, pattern: &str) -> String {
    let f = match flavor(flavor_name) {
        Ok(f) => f,
        Err(e) => return out(json!({"ok": false, "error": {"message": e}})),
    };
    out(match parse_pattern(f, pattern) {
        Ok(re) => json!({"ok": true, "ir": re.root.to_string()}),
        Err(e) => json!({"ok": false, "error": error_json(&e, pattern)}),
    })
}

/// One pattern, every target: per target a verdict (`same`, `same-bounded`, `differs`,
/// `rejects`, `not-analysed`) with the witness and which side matches it.
#[wasm_bindgen]
pub fn refract(from: &str, pattern: &str, targets: &str, op_name: &str) -> String {
    let (f, o) = match (flavor(from), op(op_name)) {
        (Ok(f), Ok(o)) => (f, o),
        (Err(e), _) | (_, Err(e)) => return out(json!({"ok": false, "error": {"message": e}})),
    };
    let src = match parse_pattern(f, pattern) {
        Ok(r) => r,
        Err(e) => return out(json!({"ok": false, "error": error_json(&e, pattern)})),
    };
    let budget = budget();
    let mut rows = Vec::new();
    for t in targets.split(',').filter(|s| !s.is_empty()) {
        let Ok(tf) = flavor(t) else { continue };
        let row = match parse_pattern(tf, pattern) {
            Err(e) if e.kind == refract_core::ErrorKind::Syntax => {
                json!({"flavor": tf.name(), "verdict": "rejects", "error": error_json(&e, pattern)})
            }
            Err(e) => json!({"flavor": tf.name(), "verdict": "not-analysed", "note": e.message}),
            Ok(ir) if tf == f => {
                let _ = ir;
                json!({"flavor": tf.name(), "verdict": "source"})
            }
            Ok(ir) => match compare(&src, &ir, o, &budget) {
                Err(e) => {
                    json!({"flavor": tf.name(), "verdict": "not-analysed", "note": e.message})
                }
                Ok(rep) => match rep.verdict {
                    Verdict::Equivalent => json!({
                        "flavor": tf.name(), "verdict": "same",
                        "note": format!("proven over {} product states", rep.product_states),
                    }),
                    Verdict::NoDifferenceUpTo { max_len, alphabet } => json!({
                        "flavor": tf.name(), "verdict": "same-bounded", "max_len": max_len,
                        "note": format!("no difference on any haystack of length <= {max_len} over {alphabet} characters (not regular, so not proven)"),
                    }),
                    Verdict::Different(w) => json!({
                        "flavor": tf.name(), "verdict": "differs",
                        "witness": {"text": w.text, "escaped": escape(&w.text)},
                        "source_matches": w.accepted_by == Side::Left,
                    }),
                },
            },
        };
        rows.push(row);
    }
    out(
        json!({"ok": true, "pattern": pattern, "source": f.name(), "operation": o.name(), "engines": rows}),
    )
}

/// Minimal DFA: states, accepting, start, edges labelled by character classes, and the state
/// path of `input` (the witness, lit in the automaton lens).
#[wasm_bindgen]
pub fn dfa(flavor_name: &str, pattern: &str, op_name: &str, input: &str) -> String {
    let (f, o) = match (flavor(flavor_name), op(op_name)) {
        (Ok(f), Ok(o)) => (f, o),
        (Err(e), _) | (_, Err(e)) => return out(json!({"ok": false, "error": {"message": e}})),
    };
    let budget = budget();
    let built = parse_pattern(f, pattern).and_then(|re| {
        refract_core::automata::regex_dfa(&re, o, &budget)
            .map(|d| refract_core::minimize::hopcroft(&d).trim())
    });
    let d = match built {
        Ok(d) => d,
        Err(e) => return out(json!({"ok": false, "error": error_json(&e, pattern)})),
    };
    // Merge parallel edges: one label per (from, to).
    let mut edges: std::collections::BTreeMap<(usize, usize), Vec<String>> = Default::default();
    for q in 0..d.len() {
        for c in 0..d.k() {
            let to = d.next(q, c);
            edges
                .entry((q, to))
                .or_default()
                .push(d.alphabet.class(c).to_string());
        }
    }
    let dead: Vec<bool> = (0..d.len())
        .map(|q| !d.accepting[q] && (0..d.k()).all(|c| d.next(q, c) == q))
        .collect();
    let mut path = vec![d.start];
    for c in input.chars() {
        let q = *path.last().unwrap_or(&d.start);
        path.push(d.next(q, d.alphabet.class_of(c)));
    }
    out(json!({
        "ok": true,
        "path": path,
        "start": d.start,
        "accepting": d.accepting,
        "dead": dead,
        "edges": edges.into_iter().map(|((a, b), l)| json!({"from": a, "to": b, "label": l.join(" ∪ ")})).collect::<Vec<_>>(),
    }))
}

/// Static ReDoS prediction (timing needs the engines; the page times JavaScript itself).
#[wasm_bindgen]
pub fn redos(flavor_name: &str, pattern: &str, op_name: &str) -> String {
    let (f, o) = match (flavor(flavor_name), op(op_name)) {
        (Ok(f), Ok(o)) => (f, o),
        (Err(e), _) | (_, Err(e)) => return out(json!({"ok": false, "error": {"message": e}})),
    };
    let budget = budget();
    let p = parse_pattern(f, pattern).and_then(|re| refract_core::redos::analyse(&re, o, &budget));
    out(match p {
        Err(e) => json!({"ok": false, "error": error_json(&e, pattern)}),
        Ok(p) => json!({
            "ok": true,
            "growth": p.growth.name(),
            "reason": p.reason,
            "span": p.span.map(|s| [s.start, s.end]),
            "attack": p.attack.map(|a| json!({"prefix": a.prefix, "pumps": a.pumps, "suffix": a.suffix})),
        }),
    })
}

/// Verified translation (ADR-0013) into every target: per target `verified` (with the scope),
/// `not-verified` (with each check), `untranslatable` (the construct and its span) or
/// `printer-bug`. Coverage cases of the source come along so the page can replay the
/// JavaScript side live.
#[wasm_bindgen]
pub fn translate(from: &str, pattern: &str, targets: &str) -> String {
    use refract_core::translate::{Outcome, Scope, translate_for};
    let f = match flavor(from) {
        Ok(f) => f,
        Err(e) => return out(json!({"ok": false, "error": {"message": e}})),
    };
    let src = match parse_pattern(f, pattern) {
        Ok(r) => r,
        Err(e) => return out(json!({"ok": false, "error": error_json(&e, pattern)})),
    };
    let budget = budget();
    let cases = |op: Op| -> Vec<Value> {
        refract_core::testgen::cover(&src, op, &budget)
            .or_else(|_| refract_core::testgen::sample(&src, op, 15, &budget))
            .unwrap_or_default()
            .into_iter()
            .take(60)
            .map(|c| json!({"text": c.text, "matches": c.matches}))
            .collect()
    };
    let mut rows = Vec::new();
    for t in targets.split(',').filter(|s| !s.is_empty()) {
        let Ok(to) = flavor(t) else { continue };
        if to == f {
            continue;
        }
        let mut scope = Scope::Both;
        let mut res = translate_for(&src, to, scope, &budget);
        if matches!(res, Ok(Outcome::Unsupported { .. })) {
            let full = translate_for(&src, to, Scope::Only(Op::FullMatch), &budget);
            if full.as_ref().is_ok_and(Outcome::verified) {
                scope = Scope::Only(Op::FullMatch);
                res = full;
            }
        }
        let row = match res {
            Err(e) => json!({"flavor": to.name(), "status": "not-analysed", "note": e.message}),
            Ok(Outcome::Unsupported { construct, span }) => json!({
                "flavor": to.name(), "status": "untranslatable", "construct": construct,
                "span": [span.start, span.end],
            }),
            Ok(Outcome::PrinterBug { text, error }) => {
                json!({"flavor": to.name(), "status": "printer-bug", "text": text, "note": error})
            }
            Ok(o) => {
                let verified = o.verified();
                let Outcome::Printed { text, checks } = o else {
                    unreachable!()
                };
                json!({
                    "flavor": to.name(),
                    "status": if verified { "verified" } else { "not-verified" },
                    "scope": match scope { Scope::Both => "full match and search".to_string(), Scope::Only(op) => format!("{} only", op.name()) },
                    "text": text,
                    "checks": checks.iter().map(|c| {
                        let (verdict, witness) = match &c.verdict {
                            Verdict::Equivalent => ("proven".to_string(), None),
                            Verdict::NoDifferenceUpTo { max_len, .. } => (format!("no difference up to length {max_len}"), None),
                            Verdict::Different(w) => ("differs".to_string(), Some(json!({"text": w.text, "escaped": escape(&w.text)}))),
                        };
                        json!({"op": c.op.name(), "verdict": verdict, "witness": witness, "cases": cases(c.op)})
                    }).collect::<Vec<_>>(),
                })
            }
        };
        rows.push(row);
    }
    out(json!({"ok": true, "pattern": pattern, "source": f.name(), "targets": rows}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn j(s: String) -> Value {
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn demo_refracts() {
        let v = j(refract(
            "python",
            r"^\w+@\w+\.com$",
            "js,python,go,java,rust",
            "full",
        ));
        let rows = v["engines"].as_array().unwrap();
        let js = rows.iter().find(|r| r["flavor"] == "js").unwrap();
        assert_eq!(js["verdict"], "differs", "{v}");
        assert!(rows.iter().any(|r| r["verdict"] == "source"));
    }

    #[test]
    fn errors_carry_spans() {
        let v = j(parse("python", "a(b"));
        assert_eq!(v["ok"], false);
        assert!(v["error"]["span"].is_array());
    }

    #[test]
    fn translate_demo() {
        let v = j(translate("python", r"^\w+@\w+\.com$", "js,go"));
        let t = v["targets"].as_array().unwrap();
        assert_eq!(t[0]["status"], "verified", "{v}");
        assert_eq!(t[1]["scope"], "full-match only", "{v}");
        assert!(!t[0]["checks"][0]["cases"].as_array().unwrap().is_empty());
    }

    #[test]
    fn dfa_and_redos() {
        let d = j(dfa("rust", "a(b|c)*", "full", "ab"));
        assert_eq!(d["path"].as_array().unwrap().len(), 3);
        assert_eq!(d["ok"], true);
        assert!(!d["edges"].as_array().unwrap().is_empty());
        let r = j(redos("python", "^(a+)+$", "full"));
        assert_eq!(r["growth"], "exponential");
    }
}
