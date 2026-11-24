//! Go flavour versus the real Go `regexp` engine (spec/flavours/go.md), run in its pinned image.
//!
//! 1. Acceptance: we accept (Ok or Unsupported) exactly the patterns `regexp.Compile` accepts.
//! 2. Semantics: our automata agree with `\A(?:p)\z` / `MatchString` on random haystacks.
//!
//! Needs Docker with `refract-oracle-go` built (`docker build -t refract-oracle-go oracle/go`);
//! otherwise the tests print a note and pass vacuously. CI always has it.

mod common;

use common::cases;
use proptest::prelude::*;
use refract_core::automata::{alphabet_for, dfa};
use refract_core::equiv::escape;
use refract_core::{Budget, Engine, Flavor, Op, parse, regular::lower};
use refract_oracle::{Engine as Oracle, EngineKind, Mode, Op as OOp, Outcome};
use std::sync::Mutex;

static GO: Mutex<Option<Oracle>> = Mutex::new(None);

fn with_go<T>(f: impl FnOnce(&mut Oracle) -> T) -> Option<T> {
    let mut guard = GO.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        match Oracle::spawn(EngineKind::Go, Mode::from_env()) {
            Ok(e) => *guard = Some(e),
            Err(e) => {
                eprintln!("skipping go oracle tests: {e}");
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
    "s",
    "ſ",
    "1",
    ".",
    "^",
    "$",
    "|",
    "(",
    ")",
    "(?:",
    "(?i)",
    "(?m)",
    "(?s)",
    "(?U)",
    "(?i:",
    "(?-i:",
    "(?P<n>",
    "(?<n>",
    "(?",
    "*",
    "+",
    "?",
    "*?",
    "{",
    "}",
    "{2}",
    "{,2}",
    "{1,3}",
    "{2,}",
    "{3,1}",
    "{01}",
    ",",
    "[",
    "]",
    "[^",
    "-",
    "\\",
    "\\d",
    "\\D",
    "\\w",
    "\\W",
    "\\s",
    "\\S",
    "\\b",
    "\\B",
    "\\A",
    "\\z",
    "\\Z",
    "\\1",
    "\\12",
    "\\0",
    "\\x41",
    "\\x{e9}",
    "\\Q",
    "\\E",
    "\\pL",
    "\\p{Greek}",
    "\\p{greek}",
    "\\P{Lu}",
    "\\p{^L}",
    "\\p{Nope}",
    "[:alpha:]",
    "[[:^digit:]]",
    "[:foo:]",
    "\\n",
    "\\.",
    "\\-",
    "\\_",
    "\\e",
    " ",
    "\n",
];

const INPUT_CHARS: &[char] = &[
    'a', 'b', 'A', 'k', 'K', '\u{212A}', 'é', 'É', 's', 'S', 'ſ', '1', '\n', '\r', ' ', '\x0B',
    '_', '-', '.', 'λ', 'Ω', '{', '}', '*',
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
    let ours = parse(Flavor::Go, p);
    let Some(theirs) = with_go(|py| py.run(p, OOp::Compile, &[])) else {
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
        "pattern {:?}: ours = {:?}, Go = {:?}",
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
    fn acceptance_matches_go(p in pattern()) {
        check_acceptance(&p)?;
    }

    #[test]
    fn prediction_matches_go(p in pattern(), hay in inputs()) {
        let Ok(ir) = parse(Flavor::Go, &p) else { return Ok(()) };
        let budget = Budget::small().start();
        let hay_refs: Vec<&str> = hay.iter().map(String::as_str).collect();
        for (op, oop) in [(Op::FullMatch, OOp::Full), (Op::Search, OOp::Search)] {
            // The interpreter (ADR-0011) must agree with the engine on every pattern, including
            // the non-regular ones the automata refuse.
            interp_check(&ir, op, oop, &hay)?;
            let Ok(re) = lower(&ir, op, &budget) else { continue };
            let alpha = alphabet_for(&[&re]);
            let Ok(m) = dfa(&re, &alpha, Engine::Derivative, &budget) else { continue };
            let Some(out) = with_go(|py| py.run(&p, oop, &hay_refs)) else { return Ok(()) };
            let Outcome::Matched(real) = out.map_err(|e| TestCaseError::fail(e.to_string()))? else {
                return Err(TestCaseError::fail(format!("we parsed {p:?} but Go rejects it")));
            };
            for (h, want) in hay.iter().zip(real) {
                prop_assert_eq!(
                    m.accepts(h), want,
                    "{} of {:?} on {}: refract vs Go", op.name(), p, escape(h)
                );
            }
        }
    }
}

/// Edge cases probed on Go 1.27.4 (see spec/flavours/go.md).
#[test]
fn known_cases() {
    let cases = [
        "a{",
        "a{,2}",
        "a{2}{3}",
        "a{2}*",
        "a*?",
        "a*??",
        "a**",
        "^*",
        r"\b*",
        "(?i)",
        "(?i)*",
        "a(?i)*",
        "a{1001}",
        "a{1000}",
        "(a{2}){500}",
        "(a{2}){501}",
        r"\1",
        r"\0",
        r"\01",
        r"\123",
        r"\8",
        r"\Q*+\E",
        r"\Qab",
        r"\pL",
        r"\p{Greek}",
        r"\p{greek}",
        r"\p{Script=Greek}",
        r"\p{^Greek}",
        r"\p{Old_Italic}",
        r"\p{Letter}",
        r"\p{Assigned}",
        r"\p{ASCII}",
        r"\p",
        r"\p{",
        "[[:alpha:]]",
        "[[:foo:]]",
        "[a-]",
        "[-a]",
        "[]a]",
        "[^]a]",
        "[]",
        "[a&&b]",
        r"[\d-z]",
        r"[a-\d]",
        "(?P<n>a)",
        "(?<n>a)",
        "(?P<n>a)(?P<n>b)",
        "(?P<1>a)",
        "(?P<>a)",
        "(?=a)",
        "(?<=a)",
        r"\Z",
        r"\z",
        r"\e",
        r"\-",
        r"\ ",
        r"\_",
        "(?x)a b",
        "(?U)a*",
        "(?s-i:a)",
        "(?-)",
        "(?)",
        "(?i-)",
        "(?:",
        ")",
        "x|*",
        "()",
        r"\x{10FFFF}",
        r"\x{110000}",
        r"\xZ",
        r"\C",
        "a{2,1}",
        "a{01}",
        "a{0}",
        "[[:^alpha:]]",
        r"\é",
        r"[\b]",
        "[a-]",
        "[z-a]",
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

/// The interpreter's verdicts versus the real engine on the same haystacks.
fn interp_check(
    ir: &refract_core::Regex,
    op: Op,
    oop: OOp,
    hay: &[String],
) -> Result<(), TestCaseError> {
    let mut mine = Vec::new();
    for h in hay {
        match refract_core::interp::is_match(ir, op, h, 200_000) {
            Ok(b) => mine.push(Some(b)),
            Err(_) => mine.push(None), // step limit: no prediction
        }
    }
    let refs: Vec<&str> = hay.iter().map(String::as_str).collect();
    let Some(out) = with_go(|go| go.run(&ir.pattern, oop, &refs)) else {
        return Ok(());
    };
    let Outcome::Matched(real) = out.map_err(|e| TestCaseError::fail(e.to_string()))? else {
        return Err(TestCaseError::fail(format!(
            "we parsed {:?} but the engine rejects it",
            ir.pattern
        )));
    };
    for ((h, want), got) in hay.iter().zip(real).zip(mine) {
        if let Some(got) = got {
            prop_assert_eq!(
                got,
                want,
                "interpreter {} of {:?} on {} vs engine",
                op.name(),
                ir.pattern,
                escape(h)
            );
        }
    }
    Ok(())
}
