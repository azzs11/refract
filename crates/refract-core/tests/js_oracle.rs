//! JavaScript flavour versus the real V8 engine (spec/flavours/javascript.md).
//!
//! Same two checks as for Python: acceptance (per flag set) and semantics on random haystacks.
//! Without `u` the analysis domain is BMP strings, so haystacks are BMP-only there.
//! Needs `node` on PATH; otherwise the tests print a note and pass vacuously.

mod common;

use common::cases;
use proptest::prelude::*;
use refract_core::automata::{alphabet_for, dfa};
use refract_core::equiv::escape;
use refract_core::{Budget, Engine, Flavor, Op, parse, regular::lower};
use refract_oracle::{Engine as Oracle, EngineKind, Mode, Op as OOp, Outcome};
use std::sync::Mutex;

static JS: Mutex<Option<Oracle>> = Mutex::new(None);

fn with_node<T>(f: impl FnOnce(&mut Oracle) -> T) -> Option<T> {
    let mut guard = JS.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        match Oracle::spawn(EngineKind::Node, Mode::from_env()) {
            Ok(e) => *guard = Some(e),
            Err(e) => {
                eprintln!("skipping js oracle tests: {e}");
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
    "😀",
    ".",
    "^",
    "$",
    "|",
    "(",
    ")",
    "(?:",
    "(?i:",
    "(?-i:",
    "(?m:",
    "(?s:",
    "(?<n>",
    "\\k<n>",
    "\\k",
    "(?=",
    "(?!",
    "(?<=",
    "(?<!",
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
    "\\1",
    "\\2",
    "\\8",
    "\\07",
    "\\0",
    "\\x41",
    "\\x4",
    "\\u00e9",
    "\\u{41}",
    "\\uD83D\\uDE00",
    "\\cA",
    "\\c",
    "\\c1",
    "\\n",
    "\\.",
    "\\-",
    "\\/",
    "\\a",
    "\\p{L}",
    "\\u2028",
    " ",
    "\n",
    "\r",
];

const FLAGS: &[&str] = &["", "", "i", "m", "s", "u", "u", "iu", "mu", "su"];

const BMP_CHARS: &[char] = &[
    'a', 'b', 'A', 'k', 'K', '\u{212A}', 'é', 'É', 's', 'S', 'ſ', '1', '\n', '\r', '\u{2028}', ' ',
    '\u{A0}', '\u{FEFF}', '_', '-', 'u', '{', '}', '\x01', '\\', 'c',
];
const ASTRAL: &[char] = &['😀', '𝒜'];

fn case() -> impl Strategy<Value = (String, &'static str)> {
    (
        prop::collection::vec(prop::sample::select(TOKENS), 1..8).prop_map(|v| v.concat()),
        prop::sample::select(FLAGS),
    )
}

fn inputs(unicode: bool) -> BoxedStrategy<Vec<String>> {
    let mut chars: Vec<char> = BMP_CHARS.to_vec();
    if unicode {
        chars.extend_from_slice(ASTRAL);
    }
    prop::collection::vec(
        prop::collection::vec(prop::sample::select(chars), 0..6)
            .prop_map(|v| v.into_iter().collect::<String>()),
        1..16,
    )
    .boxed()
}

/// The pattern as Refract reads it.
fn ours(src: &str, flags: &str) -> String {
    if flags.is_empty() {
        src.to_string()
    } else {
        format!("/{src}/{flags}")
    }
}

fn check_acceptance(src: &str, flags: &str) -> Result<(), TestCaseError> {
    let mine = parse(Flavor::JavaScript, &ours(src, flags));
    let Some(theirs) = with_node(|js| js.run_with_flags(src, flags, OOp::Compile, &[])) else {
        return Ok(());
    };
    let theirs = theirs.map_err(|e| TestCaseError::fail(e.to_string()))?;
    let we = match &mine {
        Ok(_) => true,
        Err(e) => e.kind.engine_accepts(),
    };
    prop_assert_eq!(
        we,
        matches!(theirs, Outcome::Matched(_)),
        "/{}/{}: ours = {:?}, V8 = {:?}",
        src,
        flags,
        mine.as_ref()
            .map(|r| r.root.to_string())
            .map_err(|e| e.message.clone()),
        theirs
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(20_000)))]

    #[test]
    fn acceptance_matches_v8((src, flags) in case()) {
        check_acceptance(&src, flags)?;
    }

    #[test]
    fn prediction_matches_v8(((src, flags), hay) in case().prop_flat_map(|c| {
        let u = c.1.contains('u');
        (Just(c), inputs(u))
    })) {
        let Ok(ir) = parse(Flavor::JavaScript, &ours(&src, flags)) else { return Ok(()) };
        let budget = Budget::small().start();
        let hay_refs: Vec<&str> = hay.iter().map(String::as_str).collect();
        for (op, oop) in [(Op::FullMatch, OOp::Full), (Op::Search, OOp::Search)] {
            let Ok(re) = lower(&ir, op, &budget) else { continue };
            let alpha = alphabet_for(&[&re]);
            let Ok(m) = dfa(&re, &alpha, Engine::Derivative, &budget) else { continue };
            let Some(out) = with_node(|js| js.run_with_flags(&src, flags, oop, &hay_refs)) else {
                return Ok(());
            };
            let Outcome::Matched(real) = out.map_err(|e| TestCaseError::fail(e.to_string()))? else {
                return Err(TestCaseError::fail(format!("we parsed /{src}/{flags} but V8 rejects it")));
            };
            for (h, want) in hay.iter().zip(real) {
                prop_assert_eq!(
                    m.accepts(h), want,
                    "{} of /{}/{} on {}: refract vs V8", op.name(), src, flags, escape(h)
                );
            }
        }
    }
}

/// Edge cases probed by hand on V8 14.6 (see spec/flavours/javascript.md).
#[test]
fn known_cases() {
    let cases: &[(&str, &str)] = &[
        ("(?i:a)b", ""),
        ("(?-i:a)", "i"),
        ("(?ims-:a)", ""),
        ("(?i-i:a)", ""),
        ("(?-:a)", ""),
        ("(?<n>a)|(?<n>b)", ""),
        ("(?<n>a)(?<n>b)", ""),
        ("((?<n>a)|(?<n>b))", ""),
        ("(?<n>a)|((?<n>b))", ""),
        ("[\\d-z]", ""),
        ("[\\d-z]", "u"),
        ("\\k", ""),
        ("\\k<n>(?<n>a)", ""),
        ("\\k(?<n>a)", ""),
        ("\\k<x>", ""),
        ("]", ""),
        ("]", "u"),
        ("{", ""),
        ("a{", ""),
        ("a{2,1}", ""),
        ("{2}", ""),
        ("a{,2}", ""),
        ("\\1(a)", ""),
        ("(?=a)*", ""),
        ("(?=a)*", "u"),
        ("(?<=a)*", ""),
        ("\\c", ""),
        ("\\cA", ""),
        ("[\\c_]", ""),
        ("[\\c1]", ""),
        ("[\\c]", ""),
        ("\\8", ""),
        ("\\8", "u"),
        ("\\07", ""),
        ("\\1", ""),
        ("(?<$a>x)", ""),
        ("(?<a\\u0062>x)", ""),
        ("(?<é>x)", ""),
        ("(?<1a>x)", ""),
        ("[]", ""),
        ("[^]", ""),
        ("\\p{L}", ""),
        ("\\p{L}", "u"),
        ("a**", ""),
        ("^*", ""),
        ("\\b+", ""),
        ("x|*", ""),
        ("(?:)", ""),
        ("()", ""),
        ("\\u{41}", ""),
        ("\\u{41}", "u"),
        ("[a-\\d]", ""),
        ("[\\w-]", "u"),
        ("\\-", "u"),
        ("[\\-]", "u"),
        ("\\/", "u"),
        ("\\a", "u"),
        ("\\a", ""),
        ("[z-a]", ""),
        ("(?<a>", ""),
        ("(?", ""),
        ("(?i)", ""),
        ("a{99999999999}", ""),
        ("\\u{110000}", "u"),
        ("[\\b]", ""),
        ("[\\B]", ""),
        ("[\\B]", "u"),
        ("[😀-😂]", ""),
        ("[😀-😂]", "u"),
        ("😀+", ""),
        ("a)", ""),
        ("\\", ""),
        ("[\\k]", ""),
        ("[\\k](?<n>a)", ""),
        ("\\400", ""),
        ("\\777", ""),
        ("(a)\\2", ""),
        ("(a)\\2", "u"),
    ];
    let mut failures = Vec::new();
    for (src, flags) in cases {
        if let Err(e) = check_acceptance(src, flags) {
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
