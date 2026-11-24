//! Java flavour versus the real `java.util.regex` (spec/flavours/java.md), run in its pinned image.
//!
//! 1. Acceptance: we accept (Ok or Unsupported) exactly the patterns `Pattern.compile` accepts.
//! 2. Semantics: our automata agree with `matches()` / `find()` on random haystacks.
//!
//! Needs Docker with `refract-oracle-java` built (`docker build -t refract-oracle-java oracle/java`);
//! otherwise the tests print a note and pass vacuously. CI always has it.

mod common;

use common::cases;
use proptest::prelude::*;
use refract_core::automata::{alphabet_for, dfa};
use refract_core::equiv::escape;
use refract_core::{Budget, Engine, Flavor, Op, parse, regular::lower};
use refract_oracle::{Engine as Oracle, EngineKind, Mode, Op as OOp, Outcome};
use std::sync::Mutex;

static JAVA: Mutex<Option<Oracle>> = Mutex::new(None);

fn with_java<T>(f: impl FnOnce(&mut Oracle) -> T) -> Option<T> {
    let mut guard = JAVA.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        match Oracle::spawn(EngineKind::Java, Mode::from_env()) {
            Ok(e) => *guard = Some(e),
            Err(e) => {
                eprintln!("skipping java oracle tests: {e}");
                return None;
            }
        }
    }
    guard.as_mut().map(f)
}

const TOKENS: &[&str] = &[
    "a", "b", "é", "k", "K", "\u{212A}", "s", "ſ", "1", "\u{301}", ".", "^", "$", "|", "(", ")",
    "(?:", "(?i)", "(?iu)", "(?m)", "(?s)", "(?d)", "(?x)", "(?i:", "(?-i:", "(?<n>", "\\k<n>",
    "(?=", "(?!", "(?<=", "(?<!", "(?>", "*", "+", "?", "*?", "*+", "{", "}", "{2}", "{,2}",
    "{1,3}", "{2,}", "{3,1}", ",", "[", "]", "[^", "-", "&&", "\\", "\\d", "\\D", "\\w", "\\W",
    "\\s", "\\S", "\\h", "\\v", "\\R", "\\b", "\\B", "\\A", "\\z", "\\Z", "\\G", "\\1", "\\0",
    "\\01", "\\x41", "\\x{e9}", "\\u00e9", "\\Q", "\\E", "\\cA", "\\e", "\\n", "\\.", "\\-", "\\_",
    "\\q", " ", "#", "\n",
];

const INPUT_CHARS: &[char] = &[
    'a', 'b', 'A', 'k', 'K', '\u{212A}', 'é', 'É', 's', 'S', 'ſ', '1', '\n', '\r', '\u{85}',
    '\u{2028}', ' ', '\t', '\u{A0}', '\x0B', '_', '-', '\u{301}', 'λ', '\x01',
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
    let ours = parse(Flavor::Java, p);
    let Some(theirs) = with_java(|py| py.run(p, OOp::Compile, &[])) else {
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
        "pattern {:?}: ours = {:?}, Java = {:?}",
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
    fn acceptance_matches_java(p in pattern()) {
        check_acceptance(&p)?;
    }

    #[test]
    fn prediction_matches_java(p in pattern(), hay in inputs()) {
        let Ok(ir) = parse(Flavor::Java, &p) else { return Ok(()) };
        let budget = Budget::small().start();
        let hay_refs: Vec<&str> = hay.iter().map(String::as_str).collect();
        for (op, oop) in [(Op::FullMatch, OOp::Full), (Op::Search, OOp::Search)] {
            let Ok(re) = lower(&ir, op, &budget) else { continue };
            let alpha = alphabet_for(&[&re]);
            let Ok(m) = dfa(&re, &alpha, Engine::Derivative, &budget) else { continue };
            let Some(out) = with_java(|py| py.run(&p, oop, &hay_refs)) else { return Ok(()) };
            let Outcome::Matched(real) = out.map_err(|e| TestCaseError::fail(e.to_string()))? else {
                return Err(TestCaseError::fail(format!("we parsed {p:?} but Java rejects it")));
            };
            for (h, want) in hay.iter().zip(real) {
                prop_assert_eq!(
                    m.accepts(h), want,
                    "{} of {:?} on {}: refract vs Java", op.name(), p, escape(h)
                );
            }
        }
    }
}

/// Edge cases probed on JDK 25 (see spec/flavours/java.md).
#[test]
fn known_cases() {
    let cases = [
        "a{",
        "a{,2}",
        "{2}",
        "x{a}",
        "a{2}{3}",
        "a{2}*",
        "a*?",
        "a*??",
        "a*+",
        "a*++",
        "a**",
        "^*",
        r"\b*",
        "(?i)",
        "(?i)*",
        "a(?i)*",
        "]",
        "}",
        "[]a]",
        "[]",
        "[^]a]",
        "[a-]",
        "[-a]",
        "[a&&b]",
        "[a-z&&[^aeiou]]",
        "[a[bc]]",
        r"[\d-z]",
        r"[a-\d]",
        "[z-a]",
        r"\1",
        r"(a)\1",
        r"(a)\2",
        r"\1(a)",
        r"(a\1)",
        r"\0",
        r"\01",
        r"\0400",
        r"\08",
        r"\8",
        r"\x41",
        r"\x4",
        r"\x{1F600}",
        r"\cA",
        r"\c",
        r"\e",
        r"\a",
        r"\Q*+\E",
        r"\Qab",
        r"\E",
        r"\h",
        r"\H",
        r"\v",
        r"\R",
        r"\Z",
        r"\z",
        r"\G",
        r"\A",
        r"\-",
        r"\_",
        r"\ ",
        r"\é",
        "(?<n>a)",
        "(?<n>a)(?<n>b)",
        "(?<1a>a)",
        "(?<a_b>a)",
        r"\k<n>(?<n>a)",
        r"(?<n>a)\k<n>",
        r"\k<x>",
        "(?P<n>a)",
        "(?=a)",
        "(?<=a)",
        "(?<=a+)b",
        "(?<=a{1,3})b",
        "(?>a)",
        "(?x)a b",
        "(?x)[a b]",
        "(?x)a#c",
        "(?d)a",
        "(?u)a",
        "(?s-i:a)",
        "(?-)",
        "(?)",
        "(?i-)",
        "(?:",
        ")",
        "x|*",
        "()",
        "(?#c)",
        "(?<!a)b",
        "a{2,1}",
        "a{99999999999}",
        "a{2147483647}",
        r"[\w&&\d]",
        "[&&a]",
        "[a&&]",
        "[[a]]",
        r"[\Q]\E]",
        r"[\b]",
        r"\Bk",
        r"[a-\\]",
        r"[\\-a]",
        "[a&&&b]",
        "[^a&&b]",
        "[a-z&&]",
        "(?x)a{ 2}",
        "(?x)a {2}",
        r"\N{LATIN SMALL LETTER A}",
        r"[\1]",
        r"[\v-z]",
        "a{2}+{3}",
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
