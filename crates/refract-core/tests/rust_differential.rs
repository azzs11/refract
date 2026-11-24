//! Differential tests: our Rust-flavour front-end versus `regex-syntax`, the parser the real
//! `regex` crate uses. Spec: `spec/flavours/rust.md` ("How this spec is checked").
//!
//! 1. Acceptance: we accept a pattern exactly when `regex-syntax` does.
//! 2. Class contents: when both sides parse a pattern to a single character class, the sets agree.
//!
//! Language-level agreement for whole patterns is checked in `equiv_oracle.rs` (P2).

use proptest::prelude::*;
use refract_core::{CharSet, Flavor, ir::NodeKind, parse};
use regex_syntax::hir::{Class, HirKind};

const TOKENS: &[&str] = &[
    "a",
    "b",
    "é",
    "k",
    "K",
    "\u{212A}",
    "ß",
    ".",
    "^",
    "$",
    "|",
    "(",
    ")",
    "(?:",
    "(?i)",
    "(?-u)",
    "(?x)",
    "(?s)",
    "(?m)",
    "(?U)",
    "(?R)",
    "(?i-u:",
    "(?P<n>",
    "(?<m>",
    "(?",
    "*",
    "+",
    "?",
    "{",
    "}",
    "{2}",
    "{1,3}",
    "{,2}",
    "{2,}",
    "{3,1}",
    ",",
    "[",
    "]",
    "[^",
    "-",
    "&&",
    "--",
    "~~",
    "[:alpha:]",
    "[[:^digit:]]",
    "[:foo:]",
    "\\",
    "\\d",
    "\\D",
    "\\W",
    "\\s",
    "\\b",
    "\\B",
    "\\A",
    "\\z",
    "\\Z",
    "\\pL",
    "\\p{Greek}",
    "\\P{Lu}",
    "\\p{sc=Greek}",
    "\\p{Nope}",
    "\\x41",
    "\\x{e9}",
    "\\xFF",
    "\\x{110000}",
    "\\u00e9",
    "\\u{100}",
    "\\n",
    "\\1",
    "\\<",
    "\\>",
    "\\b{start}",
    "\\b{end-half}",
    "\\b{bogus}",
    "\\ ",
    "\\#",
    "\\-",
    "\\&",
    "\\~",
    "\\e",
    "#",
    " ",
    "\n",
    "0",
    "9",
    "i",
    "u",
    "x",
    "<",
    ">",
    ":",
    "=",
    "!",
    "_",
];

fn token_pattern() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(TOKENS), 0..10).prop_map(|v| v.concat())
}

fn char_pattern() -> impl Strategy<Value = String> {
    let alphabet: Vec<char> = "ab()[]{}|*+?^$\\-&~:,0123i9uxsmURP<>!=# \n.é_"
        .chars()
        .collect();
    prop::collection::vec(prop::sample::select(alphabet), 0..12)
        .prop_map(|v| v.into_iter().collect())
}

fn theirs(p: &str) -> Result<regex_syntax::hir::Hir, String> {
    regex_syntax::ParserBuilder::new()
        .build()
        .parse(p)
        .map_err(|e| e.to_string())
}

fn check_acceptance(p: &str) -> Result<(), TestCaseError> {
    let ours = parse(Flavor::Rust, p);
    let theirs = theirs(p);
    prop_assert_eq!(
        ours.is_ok(),
        theirs.is_ok(),
        "pattern {:?}: ours = {:?}, regex-syntax = {:?}",
        p,
        ours.as_ref().map(|r| r.root.to_string()),
        theirs.as_ref().map(|_| ()),
    );
    // The reference must itself agree with the real engine's constructor (which also applies
    // compile-size limits; the short generated patterns never reach them).
    let real = match regex::Regex::new(p) {
        Ok(_) => true,
        // Size limits are not syntax: the pattern is valid, just too big to compile.
        Err(regex::Error::CompiledTooBig(_)) => true,
        Err(_) => false,
    };
    prop_assert_eq!(
        theirs.is_ok(),
        real,
        "regex-syntax and regex::Regex::new disagree on {:?}",
        p
    );
    // Same class contents when both produce a single class.
    if let (Ok(ours), Ok(hir)) = (ours, theirs) {
        let our_set = match &ours.root.kind {
            NodeKind::Class(s) => Some(s.clone()),
            NodeKind::Literal(c) => Some(CharSet::single(*c)),
            _ => None,
        };
        let their_set = match hir.kind() {
            HirKind::Class(Class::Unicode(c)) => Some(CharSet::from_ranges(
                c.ranges()
                    .iter()
                    .map(|r| (r.start() as u32, r.end() as u32)),
            )),
            _ => None,
        };
        if let (Some(a), Some(b)) = (our_set, their_set) {
            prop_assert_eq!(a, b, "class contents differ for {:?}", p);
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(20_000)))]

    #[test]
    fn acceptance_matches_regex_syntax_tokens(p in token_pattern()) {
        check_acceptance(&p)?;
    }

    #[test]
    fn acceptance_matches_regex_syntax_chars(p in char_pattern()) {
        check_acceptance(&p)?;
    }
}

/// Regression cases found by the generators above, plus hand-picked edge cases.
#[test]
fn known_cases() {
    let cases = [
        "",
        "a",
        "a|",
        "|",
        "()",
        "(|a)",
        "a**",
        "a*?",
        "a*??",
        "^*",
        "$+",
        "a{,3}",
        "a{2,}",
        "a{ 2}",
        "(?x)a{ 2 , 3 }",
        "[]]",
        "[^]]",
        "[]a]",
        "[-a]",
        "[a-]",
        "[--a]",
        "[a--b]",
        "[a-b-c]",
        "[&&a]",
        "[a&&]",
        "[a&b]",
        "[[:alpha:]]",
        "[[:^alpha:]]",
        "[:alpha:]",
        "[[:foo:]]",
        "[a[bc]]",
        "\\b{3}",
        "\\b{start}",
        "(?i)\\W",
        "(?i)[\\W]",
        "(?i)\\P{Lu}",
        "(?i)[^k]",
        "(?-u)\\w",
        "(?-u)\\W",
        "(?-u)[^a]",
        "(?-u)é",
        "(?i-u)é",
        "(?-u)[é]",
        "(?-u)\\xFF",
        "(?-u)\\x{e9}",
        "(?-u)\\u00e9",
        "(?-u)[\\x00-\\xFF]",
        "(?-u)\\B",
        "(?-u)\\b",
        "(?P<a.b[c]>x)",
        "(?P<1a>x)",
        "(?<é>x)",
        "(?P<a>x)(?<a>y)",
        "(?i)",
        "a(?i)*",
        "(?i:)",
        "(?-:a)",
        "(?--i)",
        "(?i-i)",
        "(?#c)",
        "\\Q",
        "\\e",
        "\\ ",
        "\\\n",
        "(?x)\\ ",
        "(?x)[ a ]",
        "(?x)# only a comment",
        "(?x)a#b\nc",
        "\\x{ 41 }",
        "(?x)\\x{ 41 }",
        "(?x)\\x4 1",
        "\\p{ Greek }",
        "\\p{greek}",
        "\\p{^Greek}",
        "\\p{scx=Grek}",
        "\\pZ",
        "\\p",
        "\\p{",
        "[\\b]",
        "[\\A]",
        "[a-\\d]",
        "[\\x41-\\x5A]",
        "[z-a]",
        "a{4294967295}",
        "a{4294967296}",
        "x{2}{3}",
        // Found by rust_oracle::same_language_as_regex_syntax:
        "[^]--]",
        "[]--]",
        "[]-]",
        "[]--a]",
        "[--]",
        "[---]",
        "[--a]",
        "[^--a]",
        "[a--]",
        "[]-a]",
        "[--b-c]",
        // -u: every intermediate class set must be ASCII (1M-case run found the first one).
        "(?-u)[^\\D]",
        "(?-u)[[^a]&&b]",
        "(?-u)[^[:^alpha:]]",
        "(?-u)[\\x80&&a]",
        "(?-u)[^\\x00-\\xFF]",
        "(?-u)[a&&[^\\x00-\\xFF]]",
        "(?-u)[\\x00-\\x7F]",
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

/// Case count: `PROPTEST_CASES` when set, else `default`.
fn cases(default: u32) -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}
