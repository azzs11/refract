//! Real-engine agreement for the Rust flavour (ADR-0005): our predictions versus the actual
//! `regex` crate, and our parse versus `regex-syntax`'s parse at the level of languages.
//!
//! 1. `prediction_matches_real_engine`: for random patterns and haystacks, our DFA for each
//!    operation accepts exactly when the real engine does.
//! 2. `same_language_as_regex_syntax`: our lowered IR and a direct translation of
//!    `regex-syntax`'s HIR are proven equivalent; a witness here would be a parser bug, and the
//!    real engine says which side is right.

mod common;

use common::cases;
use proptest::prelude::*;
use refract_regex::automata::{alphabet_for, dfa};
use refract_regex::equiv::{Verdict, equivalent, escape};
use refract_regex::{Budget, CharSet, Engine, Flavor, Op, Re, parse, regular::lower};
use regex_syntax::hir::{Class, Hir, HirKind};

const TOKENS: &[&str] = &[
    "a",
    "b",
    "é",
    "k",
    "K",
    "\u{212A}",
    "ß",
    "1",
    "٣",
    ".",
    "^",
    "$",
    "|",
    "(",
    ")",
    "(?:",
    "(?i)",
    "(?s)",
    "(?m)",
    "(?U)",
    "(?i:",
    "(?-u:",
    "*",
    "+",
    "?",
    "{2}",
    "{1,3}",
    "{0,}",
    "[",
    "]",
    "[^",
    "-",
    "&&",
    "--",
    "[:alpha:]",
    "\\d",
    "\\D",
    "\\w",
    "\\W",
    "\\s",
    "\\pL",
    "\\p{Greek}",
    "\\x41",
    "\\n",
    "\\.",
    " ",
];

const INPUT_CHARS: &[char] = &[
    'a', 'b', 'A', 'B', 'k', 'K', '\u{212A}', 'é', 'É', 'ß', '1', '٣', '\n', '\r', ' ', '.', 'λ',
    '_',
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

/// The real engine under an operation.
fn real(p: &str, op: Op) -> Option<regex::Regex> {
    match op {
        Op::Search => regex::Regex::new(p).ok(),
        Op::FullMatch => regex::Regex::new(&format!(r"\A(?:{p})\z")).ok(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(20_000)))]

    #[test]
    fn prediction_matches_real_engine(p in pattern(), hay in inputs()) {
        let Ok(ir) = parse(Flavor::Rust, &p) else { return Ok(()) };
        let budget = Budget::small().start();
        for op in [Op::FullMatch, Op::Search] {
            let Ok(re) = lower(&ir, op, &budget) else { continue }; // unsupported assertion
            let alpha = alphabet_for(&[&re]);
            let Ok(m) = dfa(&re, &alpha, Engine::Derivative, &budget) else { continue };
            let Some(engine) = real(&p, op) else {
                return Err(TestCaseError::fail(format!("we parsed {p:?} but the real engine rejects it")));
            };
            for h in &hay {
                prop_assert_eq!(
                    m.accepts(h), engine.is_match(h),
                    "{} of {:?} on {}: refract vs real regex crate", op.name(), p, escape(h)
                );
            }
        }
    }

    #[test]
    fn same_language_as_regex_syntax(p in pattern()) {
        let Ok(ir) = parse(Flavor::Rust, &p) else { return Ok(()) };
        let Ok(hir) = regex_syntax::ParserBuilder::new().build().parse(&p) else {
            return Err(TestCaseError::fail(format!("we accept {p:?}, regex-syntax rejects it")));
        };
        let Some(theirs) = hir_to_re(&hir) else { return Ok(()) }; // has look-around
        let budget = Budget::small().start();
        let Ok(ours) = lower(&ir, Op::FullMatch, &budget) else { return Ok(()) };
        let Ok(rep) = equivalent(&ours, &theirs, Engine::Derivative, &budget) else { return Ok(()) };
        if let Verdict::Different(w) = rep.verdict {
            let truth = real(&p, Op::FullMatch).map(|r| r.is_match(&w.text));
            return Err(TestCaseError::fail(format!(
                "parse of {p:?} differs from regex-syntax on {} (accepted by {:?}; real engine says {:?})",
                escape(&w.text), w.accepted_by, truth
            )));
        }
    }
}

/// Translates assertion-free HIR to a regular term; `None` if it has look-around.
fn hir_to_re(h: &Hir) -> Option<Re> {
    Some(match h.kind() {
        HirKind::Empty => Re::Eps,
        HirKind::Literal(lit) => Re::string(std::str::from_utf8(&lit.0).ok()?),
        HirKind::Class(Class::Unicode(c)) => Re::Set(CharSet::from_ranges(
            c.ranges()
                .iter()
                .map(|r| (r.start() as u32, r.end() as u32)),
        )),
        HirKind::Class(Class::Bytes(c)) => Re::Set(CharSet::from_ranges(
            c.ranges()
                .iter()
                .map(|r| (r.start() as u32, r.end() as u32)),
        )),
        HirKind::Look(_) => return None,
        HirKind::Repetition(r) => Re::Repeat(Box::new(hir_to_re(&r.sub)?), r.min, r.max),
        HirKind::Capture(c) => hir_to_re(&c.sub)?,
        HirKind::Concat(xs) => Re::Concat(xs.iter().map(hir_to_re).collect::<Option<_>>()?),
        HirKind::Alternation(xs) => Re::Alt(xs.iter().map(hir_to_re).collect::<Option<_>>()?),
    })
}
