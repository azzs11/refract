//! Equivalence checking with shortest readable witnesses (doc §5.4, `spec/witness.md`).
//!
//! Breadth-first search over the product of the two automata. Edges are visited in the
//! alphabet's readability order, so the first distinguishing state found gives a witness that
//! is (1) as short as possible and (2) lexicographically least by readability among the
//! shortest: lower-case ASCII before upper-case, digits, punctuation, space, readable Unicode,
//! newline, control characters.
//!
//! The derivative engine explores the product lazily and stops at the first witness; the
//! Thompson engine builds both DFAs first. Either way the verdict names its operation.

use crate::alphabet::Alphabet;
use crate::automata::{Engine, alphabet_for};
use crate::budget::Budget;
use crate::deriv::{NO_CTX, Store};
use crate::dfa::Dfa;
use crate::error::Result;
use crate::ir::Regex;
use crate::regular::{Op, Re, lower};
use crate::thompson;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Which side of the comparison accepts the witness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// A string accepted by exactly one side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Witness {
    pub text: String,
    pub accepted_by: Side,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Proven: no string distinguishes the two (the whole product was explored).
    Equivalent,
    Different(Witness),
    /// Bounded (ADR-0011): no string up to `max_len` over the representative alphabet
    /// distinguishes them. Never shown as "proven".
    NoDifferenceUpTo {
        max_len: usize,
        alphabet: usize,
    },
}

/// The result of one comparison, with the facts a report needs.
#[derive(Clone, Debug)]
pub struct Report {
    pub verdict: Verdict,
    /// Product states explored (all of them when equivalent).
    pub product_states: usize,
    /// Minterms in the shared alphabet.
    pub minterms: usize,
    pub engine: Engine,
}

/// Decides whether two regular terms denote the same language.
pub fn equivalent(a: &Re, b: &Re, engine: Engine, budget: &Budget) -> Result<Report> {
    let alphabet = alphabet_for(&[a, b]);
    match engine {
        Engine::Derivative => lazy_derivative(a, b, &alphabet, budget),
        Engine::Thompson => {
            let x = thompson::to_dfa(a, &alphabet, budget)?;
            let y = thompson::to_dfa(b, &alphabet, budget)?;
            dfa_product(&x, &y, budget)
        }
    }
}

/// Decides whether two parsed regexes behave the same under `op` (they may be in different
/// flavours: each was already lowered with its own flavour's semantics).
pub fn equivalent_regex(a: &Regex, b: &Regex, op: Op, budget: &Budget) -> Result<Report> {
    let x = lower(a, op, budget)?;
    let y = lower(b, op, budget)?;
    use crate::ir::Domain;
    if a.domain == Domain::Scalars && b.domain == Domain::Scalars {
        return equivalent(&x, &y, Engine::Derivative, budget);
    }
    // Code-unit semantics on at least one side: lift both to one scalar alphabet.
    let (_, dfas) = crate::automata::shared_dfas(&[(&x, a.domain), (&y, b.domain)], budget)?;
    dfa_product(&dfas[0], &dfas[1], budget)
}

/// Compares two parsed regexes: exactly when both lower to automata, otherwise by the bounded
/// check (ADR-0011). The verdict says which it was.
pub fn compare(a: &Regex, b: &Regex, op: Op, budget: &Budget) -> Result<Report> {
    match equivalent_regex(a, b, op, budget) {
        Err(e) if e.kind == crate::error::ErrorKind::Unsupported => bounded(a, b, op, budget),
        other => other,
    }
}

/// Characters worth trying: one representative of every minterm of the sets the two patterns
/// mention, plus a second, distinct character of the same minterm (preferring a case partner)
/// so back-references can tell "same character" from "different character".
pub fn bounded_alphabet(a: &Regex, b: &Regex) -> Vec<char> {
    use crate::ir::{Assertion, NodeKind, WordSet};
    let mut sets: Vec<crate::CharSet> = Vec::new();
    for re in [a, b] {
        re.root.walk(&mut |n| match &n.kind {
            NodeKind::Literal(c) => sets.push(crate::CharSet::single(*c)),
            NodeKind::Class(s) => sets.push(s.clone()),
            NodeKind::Assert(
                Assertion::WordBoundary { word }
                | Assertion::NotWordBoundary { word }
                | Assertion::WordStart { word }
                | Assertion::WordEnd { word }
                | Assertion::WordStartHalf { word }
                | Assertion::WordEndHalf { word },
            ) => {
                sets.push(word.chars());
                if *word == WordSet::Java {
                    let (mn, ld) = crate::tables::java_marks();
                    sets.extend([mn.clone(), ld.clone()]);
                }
            }
            NodeKind::Assert(_) => {
                sets.push(
                    ['\n', '\r', '\u{85}', '\u{2028}', '\u{2029}']
                        .into_iter()
                        .collect(),
                );
            }
            _ => {}
        });
    }
    let alpha = Alphabet::new(&sets);
    // The interpreter works on scalar values; for JS keep the bounded check in the BMP, where
    // code units and code points coincide (the exact automata handle astral haystacks).
    let js = a.flavor == crate::Flavor::JavaScript || b.flavor == crate::Flavor::JavaScript;
    let ok = |c: char| !js || (c as u32) <= 0xFFFF;
    let mut first = Vec::new();
    let mut second = Vec::new();
    for &i in alpha.readable_order() {
        let class = alpha.class(i);
        let rep = alpha.representative(i);
        if !ok(rep) {
            continue;
        }
        first.push(rep);
        let partner = rep
            .to_uppercase()
            .chain(rep.to_lowercase())
            .find(|&p| p != rep && class.contains(p) && ok(p));
        let other = partner.or_else(|| class.chars().take(64).find(|&p| p != rep && ok(p)));
        if let Some(o) = other {
            second.push(o);
        }
    }
    first.extend(second);
    first
}

/// Bounded comparison: all haystacks up to a length that keeps the search under budget,
/// shortest and most readable first.
fn bounded(a: &Regex, b: &Regex, op: Op, budget: &Budget) -> Result<Report> {
    const MAX_STRINGS: u64 = 150_000;
    const STEP_LIMIT: u64 = 200_000;
    let alphabet = bounded_alphabet(a, b);
    let k = alphabet.len().max(1) as u64;
    let mut max_len = 0usize;
    let (mut total, mut level) = (1u64, 1u64);
    while max_len < 12 {
        level = level.saturating_mul(k);
        if total.saturating_add(level) > MAX_STRINGS {
            break;
        }
        total += level;
        max_len += 1;
    }
    // Exact automata where a side lowers; the interpreter otherwise.
    type Membership = Box<dyn Fn(&str) -> Result<bool>>;
    let side = |re: &Regex| -> Membership {
        if let Ok(d) = crate::automata::regex_dfa(re, op, budget) {
            return Box::new(move |s: &str| Ok(d.accepts(s)));
        }
        let re = re.clone();
        Box::new(move |s: &str| crate::interp::is_match(&re, op, s, STEP_LIMIT))
    };
    let (fa, fb) = (side(a), side(b));
    let mut checked = 0usize;
    let mut current: Vec<String> = vec![String::new()];
    for len in 0..=max_len {
        for s in &current {
            budget.check_time()?;
            checked += 1;
            let (x, y) = (fa(s)?, fb(s)?);
            if x != y {
                return Ok(Report {
                    verdict: Verdict::Different(Witness {
                        text: s.clone(),
                        accepted_by: if x { Side::Left } else { Side::Right },
                    }),
                    product_states: checked,
                    minterms: alphabet.len(),
                    engine: Engine::Derivative,
                });
            }
        }
        if len == max_len {
            break;
        }
        current = current
            .iter()
            .flat_map(|s| alphabet.iter().map(move |&c| format!("{s}{c}")))
            .collect();
    }
    Ok(Report {
        verdict: Verdict::NoDifferenceUpTo {
            max_len,
            alphabet: alphabet.len(),
        },
        product_states: checked,
        minterms: alphabet.len(),
        engine: Engine::Derivative,
    })
}

/// Generic shortest-witness BFS over a product whose states are pairs `(p, q)`.
fn bfs<S: Copy + Eq + std::hash::Hash>(
    start: S,
    alphabet: &Alphabet,
    budget: &Budget,
    mut accepts: impl FnMut(S) -> (bool, bool),
    mut step: impl FnMut(S, usize) -> Result<S>,
) -> Result<(Verdict, usize)> {
    // parent[i] = (parent index, minterm) for reconstructing the path.
    let mut nodes: Vec<(S, usize, usize)> = vec![(start, usize::MAX, usize::MAX)];
    let mut seen: HashMap<S, ()> = HashMap::from([(start, ())]);
    let mut queue = VecDeque::from([0usize]);
    let check = |i: usize, nodes: &Vec<(S, usize, usize)>, acc: (bool, bool)| {
        (acc.0 != acc.1).then(|| {
            let mut text = Vec::new();
            let mut j = i;
            while nodes[j].1 != usize::MAX {
                text.push(alphabet.representative(nodes[j].2));
                j = nodes[j].1;
            }
            text.reverse();
            Witness {
                text: text.into_iter().collect(),
                accepted_by: if acc.0 { Side::Left } else { Side::Right },
            }
        })
    };
    if let Some(w) = check(0, &nodes, accepts(start)) {
        return Ok((Verdict::Different(w), 1));
    }
    while let Some(i) = queue.pop_front() {
        budget.check_time()?;
        let s = nodes[i].0;
        for &c in alphabet.readable_order() {
            let t = step(s, c)?;
            if seen.insert(t, ()).is_some() {
                continue;
            }
            nodes.push((t, i, c));
            let j = nodes.len() - 1;
            if let Some(w) = check(j, &nodes, accepts(t)) {
                return Ok((Verdict::Different(w), nodes.len()));
            }
            queue.push_back(j);
            budget.check("product states", nodes.len(), budget.max_dfa_states)?;
        }
    }
    Ok((Verdict::Equivalent, nodes.len()))
}

fn lazy_derivative(a: &Re, b: &Re, alphabet: &Arc<Alphabet>, budget: &Budget) -> Result<Report> {
    let mut store = Store::new(alphabet.clone(), budget);
    let x = store.from_re(a, budget)?;
    let y = store.from_re(b, budget)?;
    // Both sides read the same input, so they share one look-behind context.
    let ctx0 = store.initial_ctx();
    let norm = |s: &Store, p: u32, q: u32, ctx: u32| {
        if s.has_behind(p) || s.has_behind(q) {
            ctx
        } else {
            NO_CTX
        }
    };
    let start = (x, y, norm(&store, x, y, ctx0));
    let store = std::cell::RefCell::new(store);
    let (verdict, n) = bfs(
        start,
        alphabet,
        budget,
        |(p, q, ctx)| {
            let mut s = store.borrow_mut();
            (s.nullable(p, ctx), s.nullable(q, ctx))
        },
        |(p, q, ctx), c| {
            let mut s = store.borrow_mut();
            let dp = s.derive(p, ctx, c as u32, budget)?;
            let dq = s.derive(q, ctx, c as u32, budget)?;
            let next = s.step_ctx(ctx, c as u32);
            let next = norm(&s, dp, dq, next);
            Ok((dp, dq, next))
        },
    )?;
    Ok(Report {
        verdict,
        product_states: n,
        minterms: alphabet.len(),
        engine: Engine::Derivative,
    })
}

/// Equivalence of two complete DFAs over the same alphabet.
pub fn dfa_product(x: &Dfa, y: &Dfa, budget: &Budget) -> Result<Report> {
    let alphabet = x.alphabet.clone();
    let (verdict, n) = bfs(
        (x.start, y.start),
        &alphabet,
        budget,
        |(p, q)| (x.accepting[p], y.accepting[q]),
        |(p, q), c| Ok((x.next(p, c), y.next(q, c))),
    )?;
    Ok(Report {
        verdict,
        product_states: n,
        minterms: alphabet.len(),
        engine: Engine::Thompson,
    })
}

/// Spells a witness so invisible characters are visible: a Rust-style escaped, quoted string.
pub fn escape(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() || c == '\u{A0}' || c == '\u{FEFF}' => {
                out.push_str(&format!("\\u{{{:X}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CharSet, Flavor, parse};

    fn eq(a: &str, b: &str, op: Op) -> Verdict {
        let budget = Budget::default().start();
        let a = parse(Flavor::Rust, a).unwrap();
        let b = parse(Flavor::Rust, b).unwrap();
        equivalent_regex(&a, &b, op, &budget).unwrap().verdict
    }

    fn witness(v: Verdict) -> (String, Side) {
        match v {
            Verdict::Different(w) => (w.text, w.accepted_by),
            other => panic!("expected a witness, got {other:?}"),
        }
    }

    /// Bounded comparison (ADR-0011): real differences found exactly, no-difference labelled.
    #[test]
    fn bounded_backreferences() {
        let budget = Budget::default().start();
        let js = parse(Flavor::JavaScript, r"(a)|\1b").unwrap();
        let py = parse(Flavor::Python, r"(a)|\1b").unwrap();
        // JS: a back-reference to an unset group matches empty; Python: it fails.
        let rep = compare(&js, &py, Op::Search, &budget).unwrap();
        assert_eq!(witness(rep.verdict), ("b".into(), Side::Left));
        let a = parse(Flavor::Python, r"(a+)\1").unwrap();
        let b = parse(Flavor::Python, "(aa)+").unwrap();
        let rep = compare(&a, &b, Op::FullMatch, &budget).unwrap();
        assert!(matches!(rep.verdict, Verdict::NoDifferenceUpTo { max_len, .. } if max_len >= 8));
        let a = parse(Flavor::Python, r"(.)\1").unwrap();
        let b = parse(Flavor::Python, "..").unwrap();
        assert_eq!(
            witness(compare(&a, &b, Op::FullMatch, &budget).unwrap().verdict).1,
            Side::Right
        );
    }

    #[test]
    fn equivalent_pairs() {
        assert_eq!(eq("a(b|c)*", "a(c|b)*", Op::FullMatch), Verdict::Equivalent);
        assert_eq!(eq("(a|b)*", "(a*b*)*", Op::FullMatch), Verdict::Equivalent);
        assert_eq!(eq("[a-c]", "a|b|c", Op::FullMatch), Verdict::Equivalent);
        assert_eq!(eq("a+", "aa*", Op::FullMatch), Verdict::Equivalent);
        assert_eq!(eq("a", "^a$", Op::FullMatch), Verdict::Equivalent);
        // Search: "a" and ".*a.*" find the same haystacks.
        assert_eq!(eq("a", ".*a.*", Op::Search), Verdict::Equivalent);
    }

    #[test]
    fn shortest_readable_witnesses() {
        assert_eq!(
            witness(eq("a*", "a+", Op::FullMatch)),
            ("".into(), Side::Left)
        );
        assert_eq!(
            witness(eq("ab|ba", "ab", Op::FullMatch)),
            ("ba".into(), Side::Left)
        );
        // Operation matters: under search "a" also matches "ba".
        assert_eq!(
            witness(eq("a", "^a", Op::Search)),
            ("ba".into(), Side::Left)
        );
        // Unicode \d vs ASCII: the witness is a readable non-ASCII digit.
        let (w, side) = witness(eq(r"\d", "[0-9]", Op::FullMatch));
        assert_eq!(side, Side::Left);
        assert!(w.chars().count() == 1 && !w.is_ascii(), "{w:?}");
        // . excludes \n: the witness is the newline itself.
        assert_eq!(
            witness(eq("(?s).", ".", Op::FullMatch)),
            ("\n".into(), Side::Left)
        );
    }

    #[test]
    fn engines_agree_on_witness() {
        let budget = Budget::default().start();
        let a = Re::Concat(vec![Re::any_star(), Re::string("ab")]);
        let b = Re::Concat(vec![
            Re::star(Re::Set(CharSet::range('a', 'b'))),
            Re::string("ab"),
        ]);
        let d = equivalent(&a, &b, Engine::Derivative, &budget).unwrap();
        let t = equivalent(&a, &b, Engine::Thompson, &budget).unwrap();
        assert_eq!(d.verdict, t.verdict);
        assert_eq!(witness(d.verdict), ("cab".into(), Side::Left));
    }

    #[test]
    fn escaping() {
        assert_eq!(escape("abc\n"), "\"abc\\n\"");
        assert_eq!(escape("\u{A0}"), "\"\\u{A0}\"");
    }
}
