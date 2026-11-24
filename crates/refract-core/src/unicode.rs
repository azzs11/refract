//! Unicode data provider.
//!
//! For the Rust flavour we take Unicode tables from `regex-syntax`, which is the exact data the
//! real `regex` crate uses (ADR-0002). Only the *data* comes from there; parsing is our own. Other
//! flavours will get pinned UCD tables of their own Unicode version later, behind the same API.

use crate::charset::CharSet;
use regex_syntax::hir::{Class, ClassUnicode, ClassUnicodeRange, HirKind};
use std::sync::OnceLock;

/// Identifies where the tables come from, for reports.
pub const UNICODE_SOURCE: &str = "regex-syntax 0.8 tables";

fn class_of(pattern: &str) -> Option<CharSet> {
    let hir = regex_syntax::ParserBuilder::new()
        .build()
        .parse(pattern)
        .ok()?;
    match hir.kind() {
        HirKind::Class(Class::Unicode(cls)) => Some(from_class(cls)),
        // A one-character class can come back as a literal.
        HirKind::Literal(lit) => std::str::from_utf8(&lit.0)
            .ok()
            .map(|s| s.chars().collect()),
        _ => None,
    }
}

fn from_class(cls: &ClassUnicode) -> CharSet {
    CharSet::from_ranges(
        cls.ranges()
            .iter()
            .map(|r| (r.start() as u32, r.end() as u32)),
    )
}

fn to_class(set: &CharSet) -> ClassUnicode {
    ClassUnicode::new(set.ranges().iter().filter_map(|&(lo, hi)| {
        Some(ClassUnicodeRange::new(
            char::from_u32(lo)?,
            char::from_u32(hi)?,
        ))
    }))
}

/// Unicode `\d` (general category Nd).
pub fn perl_digit() -> &'static CharSet {
    static S: OnceLock<CharSet> = OnceLock::new();
    S.get_or_init(|| class_of(r"\d").unwrap_or_else(|| CharSet::range('0', '9')))
}

/// Unicode `\s` (White_Space property).
pub fn perl_space() -> &'static CharSet {
    static S: OnceLock<CharSet> = OnceLock::new();
    S.get_or_init(|| class_of(r"\s").unwrap_or_else(|| posix_class("space").unwrap_or_default()))
}

/// Unicode `\w` (UTS#18 word characters).
pub fn perl_word() -> &'static CharSet {
    static S: OnceLock<CharSet> = OnceLock::new();
    S.get_or_init(|| class_of(r"\w").unwrap_or_else(|| posix_class("word").unwrap_or_default()))
}

/// Characters that read well in a witness string: letters, marks, numbers, punctuation, symbols.
pub fn readable() -> &'static CharSet {
    static S: OnceLock<CharSet> = OnceLock::new();
    S.get_or_init(|| {
        ["L", "N", "P", "S"]
            .iter()
            .filter_map(|p| class_of(&format!(r"\p{{{p}}}")))
            .fold(CharSet::empty(), |acc, s| acc.union(&s))
    })
}

/// Resolves a Unicode property expression as written inside `\p{...}` (or a one-letter `\pL`),
/// e.g. `Greek`, `sc=Greek`, `Script_Extensions:Latin`, `gc!=Lu`. Returns `None` when the name
/// is unknown. The name is validated before it is handed to the table provider.
pub fn property(name: &str) -> Option<CharSet> {
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " _-.:=!&".contains(c));
    if !ok {
        return None;
    }
    class_of(&format!(r"\p{{{name}}}"))
}

/// Characters whose simple case-fold orbit can be non-trivial: `\p{Changes_When_Casemapped}`.
/// Every other character folds only to itself; `tests::cwcm_covers_every_fold_orbit` checks
/// this exhaustively over all scalar values.
fn case_mapped() -> &'static CharSet {
    static S: OnceLock<CharSet> = OnceLock::new();
    S.get_or_init(|| property("CWCM").unwrap_or_else(CharSet::full))
}

/// Simple case folding closure: every character that simple-case-folds to the same thing as a
/// member is added (this is what Rust's `(?i)` does).
///
/// `regex-syntax` walks every code point of a range that overlaps its table, so folding a
/// near-full set (any negated class) costs about a million lookups. Folding only the members
/// that can change keeps it proportional to the ~2,800 case-mapped characters.
pub fn case_fold_simple(set: &CharSet) -> CharSet {
    let relevant = set.intersect(case_mapped());
    if relevant.is_empty() {
        return set.clone();
    }
    let mut cls = to_class(&relevant);
    cls.case_fold_simple();
    set.union(&from_class(&cls))
}

/// ASCII-only case folding (`(?i-u)` in Rust).
pub fn case_fold_ascii(set: &CharSet) -> CharSet {
    let upper = set.intersect(&CharSet::range('A', 'Z'));
    let lower = set.intersect(&CharSet::range('a', 'z'));
    let shift = |s: &CharSet, up: bool| {
        CharSet::from_ranges(s.ranges().iter().map(|&(lo, hi)| {
            if up {
                (lo - 32, hi - 32)
            } else {
                (lo + 32, hi + 32)
            }
        }))
    };
    set.union(&shift(&upper, false)).union(&shift(&lower, true))
}

/// POSIX bracket classes (`[[:alpha:]]`), which are ASCII-only by definition in RE2-family
/// engines.
pub fn posix_class(name: &str) -> Option<CharSet> {
    let r = |pairs: &[(char, char)]| {
        Some(CharSet::from_ranges(
            pairs.iter().map(|&(a, b)| (a as u32, b as u32)),
        ))
    };
    match name {
        "alnum" => r(&[('0', '9'), ('A', 'Z'), ('a', 'z')]),
        "alpha" => r(&[('A', 'Z'), ('a', 'z')]),
        "ascii" => r(&[('\0', '\x7F')]),
        "blank" => r(&[('\t', '\t'), (' ', ' ')]),
        "cntrl" => r(&[('\0', '\x1F'), ('\x7F', '\x7F')]),
        "digit" => r(&[('0', '9')]),
        "graph" => r(&[('!', '~')]),
        "lower" => r(&[('a', 'z')]),
        "print" => r(&[(' ', '~')]),
        "punct" => r(&[('!', '/'), (':', '@'), ('[', '`'), ('{', '~')]),
        "space" => r(&[('\t', '\r'), (' ', ' ')]),
        "upper" => r(&[('A', 'Z')]),
        "word" => r(&[('0', '9'), ('A', 'Z'), ('a', 'z'), ('_', '_')]),
        "xdigit" => r(&[('0', '9'), ('A', 'F'), ('a', 'f')]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perl_classes_are_unicode() {
        assert!(perl_digit().contains('٣')); // ARABIC-INDIC DIGIT THREE
        assert!(perl_word().contains('é'));
        assert!(perl_word().contains('_'));
        assert!(perl_space().contains('\u{2003}'));
        assert!(!perl_word().contains('-'));
    }

    #[test]
    fn properties() {
        assert!(property("Greek").unwrap().contains('λ'));
        assert!(property("sc=Greek").unwrap().contains('λ'));
        assert!(property("L").unwrap().contains('a'));
        assert!(property("NotAThing").is_none());
        assert!(property("x}|.*").is_none());
    }

    /// The fast path in `case_fold_simple` is only sound if no character outside CWCM has a
    /// non-trivial orbit. Check every scalar value against the unoptimised fold.
    #[test]
    fn cwcm_covers_every_fold_orbit() {
        let mapped = case_mapped();
        // Guard against a vacuous test: CWCM must have resolved (not the full-set fallback).
        assert!(
            (1_000..20_000).contains(&mapped.len()),
            "CWCM has {} chars",
            mapped.len()
        );
        let outside = mapped.complement();
        assert!(outside.len() > 1_000_000);
        let mut bad = Vec::new();
        for c in outside.chars() {
            let mut cls = to_class(&CharSet::single(c));
            cls.case_fold_simple();
            if from_class(&cls) != CharSet::single(c) {
                bad.push(c);
            }
        }
        assert!(
            bad.is_empty(),
            "chars outside CWCM with non-trivial folds: {bad:?}"
        );
        // And the fast path equals the slow path on a few large sets.
        for set in [
            CharSet::full(),
            perl_word().complement(),
            CharSet::range('A', 'z'),
        ] {
            let mut slow = to_class(&set);
            slow.case_fold_simple();
            assert_eq!(case_fold_simple(&set), from_class(&slow));
        }
    }

    #[test]
    fn folding() {
        let k = case_fold_simple(&CharSet::single('k'));
        assert!(k.contains('K') && k.contains('\u{212A}')); // KELVIN SIGN
        let k = case_fold_ascii(&CharSet::single('k'));
        assert!(k.contains('K') && !k.contains('\u{212A}'));
        assert_eq!(
            case_fold_ascii(&CharSet::range('Y', 'b')),
            CharSet::from_ranges([
                ('A' as u32, 'B' as u32),
                ('Y' as u32, 'b' as u32),
                ('y' as u32, 'z' as u32)
            ])
        );
    }
}
