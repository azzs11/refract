//! Python flavour versus the real CPython `re` engine (spec/flavours/python.md).
//!
//! 1. Acceptance: we accept (Ok or Unsupported) exactly the patterns CPython compiles.
//! 2. Semantics: wherever we can build an automaton, it agrees with `re.fullmatch` / `re.search`
//!    on random haystacks.
//!
//! Needs `python3` on PATH (or `REFRACT_ORACLE=docker` with the image built); otherwise the
//! tests print a note and pass vacuously, so `cargo test` works on any machine. CI always has it.

mod common;

use common::cases;
use proptest::prelude::*;
use refract_core::automata::{alphabet_for, dfa};
use refract_core::equiv::escape;
use refract_core::{Budget, Engine, Flavor, Op, parse, regular::lower};
use refract_oracle::{Engine as Oracle, EngineKind, Mode, Op as OOp, Outcome};
use std::sync::Mutex;

static PY: Mutex<Option<Oracle>> = Mutex::new(None);

fn with_python<T>(f: impl FnOnce(&mut Oracle) -> T) -> Option<T> {
    let mut guard = PY.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        match Oracle::spawn(EngineKind::Python, Mode::from_env()) {
            Ok(e) => *guard = Some(e),
            Err(e) => {
                eprintln!("skipping python oracle tests: {e}");
                return None;
            }
        }
    }
    guard.as_mut().map(f)
}

const TOKENS: &[&str] = &[
    "a",
    "b",
    "é",
    "k",
    "K",
    "\u{212A}",
    "ß",
    "ſ",
    "s",
    "1",
    "²",
    "٣",
    ".",
    "^",
    "$",
    "|",
    "(",
    ")",
    "(?:",
    "(?i)",
    "(?a)",
    "(?s)",
    "(?m)",
    "(?x)",
    "(?i:",
    "(?-i:",
    "(?a:",
    "(?P<n>",
    "(?P=n)",
    "(?#c)",
    "(?=",
    "(?!",
    "(?<=",
    "(?<!",
    "(?>",
    "(?(1)",
    "*",
    "+",
    "?",
    "*?",
    "*+",
    "{",
    "}",
    "{2}",
    "{,2}",
    "{1,3}",
    "{2,}",
    "{3,1}",
    ",",
    "[",
    "]",
    "[^",
    "-",
    "&&",
    "\\",
    "\\d",
    "\\D",
    "\\w",
    "\\W",
    "\\s",
    "\\b",
    "\\B",
    "\\A",
    "\\Z",
    "\\z",
    "\\1",
    "\\x41",
    "\\u00e9",
    "\\0",
    "\\08",
    "\\101",
    "\\n",
    "\\.",
    "\\-",
    "\\e",
    "\\N{EM DASH}",
    " ",
    "#",
    "\n",
];

const INPUT_CHARS: &[char] = &[
    'a', 'b', 'A', 'k', 'K', '\u{212A}', 'é', 'É', 'ß', 's', 'S', 'ſ', '1', '²', '٣', '\n', ' ',
    '\x1c', '_', '-', '.', 'λ',
];

fn pattern() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(TOKENS), 1..8).prop_map(|v| v.concat())
}

fn inputs() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(
        prop::collection::vec(prop::sample::select(INPUT_CHARS), 0..6)
            .prop_map(|v| v.into_iter().collect::<String>()),
        1..16,
    )
}

fn check_acceptance(p: &str) -> Result<(), TestCaseError> {
    let ours = parse(Flavor::Python, p);
    let Some(theirs) = with_python(|py| py.run(p, OOp::Compile, &[])) else {
        return Ok(());
    };
    let theirs = theirs.map_err(|e| TestCaseError::fail(e.to_string()))?;
    let we_accept = match &ours {
        Ok(_) => true,
        Err(e) => e.kind.engine_accepts(),
    };
    let they_accept = matches!(theirs, Outcome::Matched(_));
    prop_assert_eq!(
        we_accept,
        they_accept,
        "pattern {:?}: ours = {:?}, CPython = {:?}",
        p,
        ours.as_ref()
            .map(|r| r.root.to_string())
            .map_err(|e| e.message.clone()),
        theirs
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(20_000)))]

    #[test]
    fn acceptance_matches_cpython(p in pattern()) {
        check_acceptance(&p)?;
    }

    #[test]
    fn prediction_matches_cpython(p in pattern(), hay in inputs()) {
        let Ok(ir) = parse(Flavor::Python, &p) else { return Ok(()) };
        let budget = Budget::small().start();
        let hay_refs: Vec<&str> = hay.iter().map(String::as_str).collect();
        for (op, oop) in [(Op::FullMatch, OOp::Full), (Op::Search, OOp::Search)] {
            let Ok(re) = lower(&ir, op, &budget) else { continue };
            let alpha = alphabet_for(&[&re]);
            let Ok(m) = dfa(&re, &alpha, Engine::Derivative, &budget) else { continue };
            let Some(out) = with_python(|py| py.run(&p, oop, &hay_refs)) else { return Ok(()) };
            let Outcome::Matched(real) = out.map_err(|e| TestCaseError::fail(e.to_string()))? else {
                return Err(TestCaseError::fail(format!("we parsed {p:?} but CPython rejects it")));
            };
            for (h, want) in hay.iter().zip(real) {
                prop_assert_eq!(
                    m.accepts(h), want,
                    "{} of {:?} on {}: refract vs CPython", op.name(), p, escape(h)
                );
            }
        }
    }
}

/// Edge cases probed by hand on CPython 3.15 (see spec/flavours/python.md).
#[test]
fn known_cases() {
    let cases = [
        "",
        "a{",
        "a{}",
        "a{,3}",
        "a{,}",
        "a{ 2}",
        "a**",
        "x*{2}",
        "^*",
        r"\b*",
        "[]",
        "[]a]",
        "[^]a]",
        "[a-]",
        r"[\d-z]",
        "[z-a]",
        "[]-a]",
        "a(?#x)*",
        "(?#x)*",
        "a(?i)",
        "(?i)(?m)a",
        "(?x) (?i)a",
        r"(a)\1",
        r"\1(a)",
        r"(a\1)",
        r"\8",
        r"\08",
        r"\400",
        r"[\400]",
        r"[\A]",
        r"\N{EM DASH}",
        "(?u)a",
        r"(?a:\w)",
        r"(?-a:\w)",
        "(?i-i:a)",
        "(?-:a)",
        "(?<=a+)b",
        "(?<=a|bc)d",
        "(?<=ab|cd)e",
        "(?(1)a|b)",
        "(?(1)a|b)(c)",
        "(a)(?(1)a|b|c)",
        "(?>a)",
        "a*+",
        "a{2}+",
        "(?=a)*",
        "(?P<é>x)",
        "(?P<1a>x)",
        "(?P<a>x)(?P<a>y)",
        "(?P=a)",
        r"\ud800",
        "\\",
        "a|*",
        "()",
        "a*?+",
        r"\Q",
        r"\e",
        "(?L)a",
        "(?au)a",
        "(?a)(?u)a",
        "(?P>x)",
        "(?<x>a)",
        "a{4294967295}",
        "a{4294967294}",
        "(?t)a",
        "(?x)[ a]",
        "(?x)a{ 2}",
    ];
    let mut failures = Vec::new();
    for p in cases {
        if let Err(e) = check_acceptance(p) {
            failures.push(e.to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
