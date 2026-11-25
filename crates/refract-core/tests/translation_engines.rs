//! Verified translations on the real engines (ADR-0013): for random patterns translated between
//! Python and JavaScript, every coverage string of the source gets the model's answer from both
//! CPython (source or target) and V8. Needs `python3` and `node`; otherwise passes vacuously.

mod common;

use common::cases;
use proptest::prelude::*;
use refract_core::testgen::cover;
use refract_core::translate::{Outcome, translate};
use refract_core::{Budget, Flavor, Op, parse};
use refract_oracle::{Engine as Oracle, EngineKind, Mode, Op as OOp, Outcome as Got};
use std::sync::Mutex;

static ENGINES: Mutex<Option<(Oracle, Oracle)>> = Mutex::new(None);

fn with_engines<T>(f: impl FnOnce(&mut Oracle, &mut Oracle) -> T) -> Option<T> {
    let mut guard = ENGINES.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        match (
            Oracle::spawn(EngineKind::Python, Mode::from_env()),
            Oracle::spawn(EngineKind::Node, Mode::from_env()),
        ) {
            (Ok(p), Ok(n)) => *guard = Some((p, n)),
            _ => {
                eprintln!("skipping: python3 and node are needed");
                return None;
            }
        }
    }
    guard.as_mut().map(|(p, n)| f(p, n))
}

const TOKENS: &[&str] = &[
    "a", "b", "é", "K", "1", "😀", " ", ".", "^", "$", "|", "(", ")", "(?:", "(?i)", "(?m)",
    "(?s)", "(?=", "(?!", "(?<=", "(?<!", "*", "+", "?", "{2}", "{1,3}", "[a-c]", "[^a]", "\\d",
    "\\w", "\\W", "\\s", "\\b", "\\B", "\\n", "\\.",
];

fn pattern() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(TOKENS), 1..7).prop_map(|v| v.concat())
}

fn run(e: &mut Oracle, js: bool, p: &str, op: Op, inputs: &[&str]) -> Vec<bool> {
    let op = if op == Op::FullMatch {
        OOp::Full
    } else {
        OOp::Search
    };
    let out = if js {
        let (src, flags) = p
            .strip_prefix('/')
            .and_then(|r| r.rfind('/').map(|i| (&r[..i], &r[i + 1..])))
            .unwrap();
        e.run_with_flags(src, flags, op, inputs)
    } else {
        e.run(p, op, inputs)
    };
    match out.expect("engine io") {
        Got::Matched(v) => v,
        Got::Rejected(m) => panic!("engine rejected {p:?}: {m}"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(1_500)))]

    #[test]
    fn verified_translations_hold_on_real_engines(p in pattern(), from_js in any::<bool>()) {
        let (from, to, src_text) = if from_js {
            (Flavor::JavaScript, Flavor::Python, format!("/{p}/u"))
        } else {
            (Flavor::Python, Flavor::JavaScript, p.clone())
        };
        let Ok(src) = parse(from, &src_text) else { return Ok(()) };
        let budget = Budget::small().start();
        let Ok(out) = translate(&src, to, &budget) else { return Ok(()) };
        if !out.verified() {
            return Ok(());
        }
        let Outcome::Printed { text, .. } = out else { unreachable!() };
        for op in [Op::FullMatch, Op::Search] {
            let Ok(cs) = cover(&src, op, &budget) else { continue };
            let inputs: Vec<&str> = cs.iter().map(|c| c.text.as_str()).take(100).collect();
            let got = with_engines(|py, node| {
                let (s_eng, t_eng) = if from_js { (node, py) } else { (py, node) };
                (run(s_eng, from_js, &src_text, op, &inputs), run(t_eng, !from_js, &text, op, &inputs))
            });
            let Some((a, b)) = got else { return Ok(()) };
            for ((c, x), y) in cs.iter().zip(a).zip(b) {
                prop_assert!(x == c.matches && y == c.matches,
                    "{from}:{src_text} -> {to}:{text} {} on {:?}: model {}, {from} {x}, {to} {y}",
                    op.name(), c.text, c.matches);
            }
        }
    }
}
