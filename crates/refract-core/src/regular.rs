//! Pure regular terms, and lowering from the IR for a named match operation.
//!
//! A [`Re`] denotes a set of strings and nothing else: no captures, no greediness, no
//! assertions. Both automaton engines consume `Re`. Lowering is where the *operation* (full
//! match vs search) is fixed, because the same pattern denotes different languages under each
//! (spec/ir.md, "Operations").
//!
//! Assertions are resolved only where their truth value is known statically (ADR-0004): `^` at a
//! position that is certainly the haystack start becomes ε, `^` at a position that certainly is
//! not becomes ∅. Anything else is reported as `Unsupported` with the assertion's span instead of
//! guessing. Full assertion support (derivatives with look-around context) arrives in P4.

use crate::budget::Budget;
use crate::charset::CharSet;
use crate::error::{Error, Result, Span};
use crate::ir::{Assertion, Node, NodeKind, Regex};
use std::fmt;

/// A regular term over Unicode scalar values.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Re {
    /// The empty language ∅.
    Nothing,
    /// The language {ε}.
    Eps,
    /// One character from the set.
    Set(CharSet),
    Concat(Vec<Re>),
    Alt(Vec<Re>),
    /// `sub{min,max}`; `max: None` is unbounded.
    Repeat(Box<Re>, u32, Option<u32>),
    /// Intersection.
    And(Vec<Re>),
    /// Complement relative to all strings.
    Not(Box<Re>),
}

impl Re {
    pub fn lit(c: char) -> Re {
        Re::Set(CharSet::single(c))
    }

    pub fn string(s: &str) -> Re {
        Re::Concat(s.chars().map(Re::lit).collect())
    }

    pub fn star(r: Re) -> Re {
        Re::Repeat(Box::new(r), 0, None)
    }

    /// Σ*, every string.
    pub fn any_star() -> Re {
        Re::star(Re::Set(CharSet::full()))
    }

    /// Every character set mentioned, for building the minterm alphabet.
    pub fn collect_sets(&self, out: &mut Vec<CharSet>) {
        match self {
            Re::Nothing | Re::Eps => {}
            Re::Set(s) => out.push(s.clone()),
            Re::Concat(xs) | Re::Alt(xs) | Re::And(xs) => {
                xs.iter().for_each(|x| x.collect_sets(out));
            }
            Re::Repeat(r, _, _) | Re::Not(r) => r.collect_sets(out),
        }
    }

    /// Direct set-based membership test, written for clarity, not speed: a third, independent
    /// semantics used to check both automaton engines on short strings.
    pub fn matches_naive(&self, s: &str) -> bool {
        let chars: Vec<char> = s.chars().collect();
        self.ends_from(&chars, 0).contains(&chars.len())
    }

    /// All `j` such that `chars[i..j]` is in the language.
    fn ends_from(&self, chars: &[char], i: usize) -> Vec<usize> {
        let n = chars.len();
        let mut out: Vec<usize> = match self {
            Re::Nothing => vec![],
            Re::Eps => vec![i],
            Re::Set(set) => {
                if i < n && set.contains(chars[i]) {
                    vec![i + 1]
                } else {
                    vec![]
                }
            }
            Re::Concat(xs) => {
                let mut cur = vec![i];
                for x in xs {
                    let mut next = Vec::new();
                    for &p in &cur {
                        next.extend(x.ends_from(chars, p));
                    }
                    next.sort_unstable();
                    next.dedup();
                    cur = next;
                }
                cur
            }
            Re::Alt(xs) => xs.iter().flat_map(|x| x.ends_from(chars, i)).collect(),
            Re::And(xs) => {
                let mut cur: Vec<usize> = (i..=n).collect();
                for x in xs {
                    let e = x.ends_from(chars, i);
                    cur.retain(|j| e.contains(j));
                }
                cur
            }
            Re::Not(x) => {
                let e = x.ends_from(chars, i);
                (i..=n).filter(|j| !e.contains(j)).collect()
            }
            Re::Repeat(r, min, max) => {
                let step = |from: &[usize]| {
                    let mut next: Vec<usize> =
                        from.iter().flat_map(|&p| r.ends_from(chars, p)).collect();
                    next.sort_unstable();
                    next.dedup();
                    next
                };
                // Exactly `min` copies first.
                let mut cur = vec![i];
                for _ in 0..*min {
                    cur = step(&cur);
                }
                let mut reached = cur.clone();
                match max {
                    // Up to `max - min` more copies, each optional.
                    Some(max) => {
                        for _ in *min..*max {
                            cur = step(&cur);
                            reached.extend(cur.iter().copied());
                        }
                    }
                    // Any number of further copies: closure until nothing new appears.
                    None => loop {
                        let next: Vec<usize> = step(&cur)
                            .into_iter()
                            .filter(|j| !reached.contains(j))
                            .collect();
                        if next.is_empty() {
                            break;
                        }
                        reached.extend(next.iter().copied());
                        cur = next;
                    },
                }
                reached
            }
        };
        out.sort_unstable();
        out.dedup();
        out
    }
}

impl fmt::Display for Re {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |f: &mut fmt::Formatter<'_>, head: &str, xs: &[Re]| {
            write!(f, "({head}")?;
            for x in xs {
                write!(f, " {x}")?;
            }
            f.write_str(")")
        };
        match self {
            Re::Nothing => f.write_str("∅"),
            Re::Eps => f.write_str("ε"),
            Re::Set(s) => write!(f, "{s}"),
            Re::Concat(xs) => list(f, "cat", xs),
            Re::Alt(xs) => list(f, "alt", xs),
            Re::And(xs) => list(f, "and", xs),
            Re::Not(x) => write!(f, "(not {x})"),
            Re::Repeat(r, min, max) => {
                let max = max.map_or(String::new(), |m| m.to_string());
                write!(f, "(rep{{{min},{max}}} {r})")
            }
        }
    }
}

/// The match operation a verdict is about. Every verdict names one (doc review item 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    /// The whole haystack must match (Python `re.fullmatch`, Java `matches()`).
    FullMatch,
    /// A match exists anywhere (JS `test()`, Python `re.search`, Rust `is_match`).
    Search,
}

impl Op {
    pub fn name(self) -> &'static str {
        match self {
            Op::FullMatch => "full-match",
            Op::Search => "search",
        }
    }
}

impl std::str::FromStr for Op {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s {
            "full" | "full-match" | "fullmatch" => Ok(Op::FullMatch),
            "search" | "find" | "test" | "is-match" => Ok(Op::Search),
            other => Err(format!("unknown operation `{other}` (use full or search)")),
        }
    }
}

/// Three-valued static knowledge of a position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tri {
    Yes,
    No,
    Unknown,
}

#[derive(Clone, Copy, Debug)]
struct Ctx {
    at_start: Tri,
    at_end: Tri,
}

/// Lowers a parsed regex to the language of haystacks accepted by `op`.
pub fn lower(re: &Regex, op: Op, budget: &Budget) -> Result<Re> {
    match op {
        Op::FullMatch => lower_node(
            &re.root,
            Ctx {
                at_start: Tri::Yes,
                at_end: Tri::Yes,
            },
            budget,
        ),
        Op::Search => lower_search(&re.root, budget),
    }
}

/// Search: the haystack contains a match, i.e. Σ* r Σ*, with leading/trailing anchors of each
/// top-level alternative absorbed into the prefix/suffix.
fn lower_search(root: &Node, budget: &Budget) -> Result<Re> {
    let alts: Vec<&Node> = match &unwrap_groups(root).kind {
        NodeKind::Alt(xs) => xs.iter().collect(),
        _ => vec![root],
    };
    let mut out = Vec::with_capacity(alts.len());
    for alt in alts {
        let mut items: Vec<&Node> = match &unwrap_groups(alt).kind {
            NodeKind::Concat(xs) => xs.iter().collect(),
            _ => vec![alt],
        };
        let mut prefix = Re::any_star();
        let mut at_start = Tri::Unknown;
        if let Some(NodeKind::Assert(a)) = items.first().map(|n| &unwrap_groups(n).kind) {
            match a {
                Assertion::TextStart => {
                    prefix = Re::Eps;
                    at_start = Tri::Yes;
                    items.remove(0);
                }
                Assertion::LineStart { crlf: false } => {
                    prefix = Re::Alt(vec![
                        Re::Eps,
                        Re::Concat(vec![Re::any_star(), Re::lit('\n')]),
                    ]);
                    items.remove(0);
                }
                _ => {}
            }
        }
        let mut suffix = Re::any_star();
        let mut at_end = Tri::Unknown;
        if let Some(NodeKind::Assert(a)) = items.last().map(|n| &unwrap_groups(n).kind) {
            match a {
                Assertion::TextEnd => {
                    suffix = Re::Eps;
                    at_end = Tri::Yes;
                    items.pop();
                }
                Assertion::TextEndOrFinalNewline => {
                    suffix = Re::Alt(vec![Re::Eps, Re::lit('\n')]);
                    items.pop();
                }
                Assertion::LineEnd { crlf: false } => {
                    suffix = Re::Alt(vec![
                        Re::Eps,
                        Re::Concat(vec![Re::lit('\n'), Re::any_star()]),
                    ]);
                    items.pop();
                }
                _ => {}
            }
        }
        let ctx = Ctx { at_start, at_end };
        let core = lower_seq(&items, ctx, budget)?;
        out.push(Re::Concat(vec![prefix, core, suffix]));
    }
    Ok(if out.len() == 1 {
        out.pop().unwrap_or(Re::Nothing)
    } else {
        Re::Alt(out)
    })
}

fn unwrap_groups(mut n: &Node) -> &Node {
    while let NodeKind::Group(g) = &n.kind {
        n = &g.sub;
    }
    n
}

fn lower_node(node: &Node, ctx: Ctx, budget: &Budget) -> Result<Re> {
    Ok(match &node.kind {
        NodeKind::Empty => Re::Eps,
        NodeKind::Literal(c) => Re::lit(*c),
        NodeKind::Class(s) => Re::Set(s.clone()),
        NodeKind::Group(g) if g.atomic => {
            return Err(Error::unsupported(
                node.span,
                "atomic groups change what matches; they are analysed from phase P4",
            ));
        }
        NodeKind::Group(g) => lower_node(&g.sub, ctx, budget)?,
        NodeKind::Look(_) => {
            return Err(Error::unsupported(
                node.span,
                "look-around is analysed from phase P4",
            ));
        }
        NodeKind::Backref(_) => {
            return Err(Error::unsupported(
                node.span,
                "backreferences are not regular; bounded analysis arrives in phase P4",
            ));
        }
        NodeKind::Conditional(_) => {
            return Err(Error::unsupported(
                node.span,
                "conditional groups are analysed from phase P4",
            ));
        }
        NodeKind::Repeat(r) if r.possessive => {
            return Err(Error::unsupported(
                node.span,
                "possessive quantifiers change what matches; they are analysed from phase P4",
            ));
        }
        NodeKind::Alt(xs) => Re::Alt(
            xs.iter()
                .map(|x| lower_node(x, ctx, budget))
                .collect::<Result<_>>()?,
        ),
        NodeKind::Concat(xs) => {
            let items: Vec<&Node> = xs.iter().collect();
            lower_seq(&items, ctx, budget)?
        }
        NodeKind::Repeat(r) => {
            for n in [Some(r.min), r.max].into_iter().flatten() {
                if n > budget.max_repeat {
                    return Err(Error::budget(format!(
                        "counted repetition {{{n}}} at {} exceeds the limit of {}",
                        node.span, budget.max_repeat
                    )));
                }
            }
            let sub_ctx = if r.max.is_some_and(|m| m <= 1) {
                ctx
            } else {
                // Later iterations sit elsewhere: only "No" survives repetition.
                Ctx {
                    at_start: if ctx.at_start == Tri::No {
                        Tri::No
                    } else {
                        Tri::Unknown
                    },
                    at_end: if ctx.at_end == Tri::No {
                        Tri::No
                    } else {
                        Tri::Unknown
                    },
                }
            };
            Re::Repeat(Box::new(lower_node(&r.sub, sub_ctx, budget)?), r.min, r.max)
        }
        NodeKind::Assert(a) => lower_assertion(*a, node.span, ctx)?,
    })
}

/// Lowers a sequence, computing for each item whether it certainly starts at the haystack start
/// (all previous items are zero-width) or certainly does not (some previous item consumes).
fn lower_seq(items: &[&Node], ctx: Ctx, budget: &Budget) -> Result<Re> {
    let lens: Vec<(usize, Option<usize>)> = items.iter().map(|n| width(n)).collect();
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let before = &lens[..i];
        let after = &lens[i + 1..];
        let at_start = position(ctx.at_start, before);
        let at_end = position(ctx.at_end, after);
        out.push(lower_node(item, Ctx { at_start, at_end }, budget)?);
    }
    Ok(match out.len() {
        0 => Re::Eps,
        1 => out.pop().unwrap_or(Re::Eps),
        _ => Re::Concat(out),
    })
}

fn position(edge: Tri, others: &[(usize, Option<usize>)]) -> Tri {
    if others.iter().any(|&(min, _)| min > 0) {
        Tri::No
    } else if edge == Tri::Yes && others.iter().all(|&(_, max)| max == Some(0)) {
        Tri::Yes
    } else if edge == Tri::No {
        Tri::No
    } else {
        Tri::Unknown
    }
}

/// (minimum, maximum) number of characters a node can consume; `None` = unbounded.
fn width(n: &Node) -> (usize, Option<usize>) {
    match &n.kind {
        NodeKind::Empty | NodeKind::Assert(_) => (0, Some(0)),
        NodeKind::Literal(_) => (1, Some(1)),
        NodeKind::Class(s) if s.is_empty() => (1, Some(0)), // never matches; treat as consuming
        NodeKind::Class(_) => (1, Some(1)),
        NodeKind::Group(g) => width(&g.sub),
        NodeKind::Look(_) => (0, Some(0)),
        // Unknown statically: the referenced group can match anything.
        NodeKind::Backref(_) => (0, None),
        NodeKind::Conditional(c) => {
            let y = width(&c.yes);
            let n = c.no.as_ref().map_or((0, Some(0)), |n| width(n));
            (y.0.min(n.0), y.1.zip(n.1).map(|(a, b)| a.max(b)))
        }
        NodeKind::Concat(xs) => xs.iter().fold((0, Some(0)), |(a, b), x| {
            let (c, d) = width(x);
            (
                a.saturating_add(c),
                b.zip(d).map(|(b, d)| b.saturating_add(d)),
            )
        }),
        NodeKind::Alt(xs) => {
            let ws: Vec<_> = xs.iter().map(width).collect();
            let min = ws.iter().map(|w| w.0).min().unwrap_or(0);
            let max = ws.iter().try_fold(0usize, |acc, w| w.1.map(|m| acc.max(m)));
            (min, max)
        }
        NodeKind::Repeat(r) => {
            let (a, b) = width(&r.sub);
            let min = a.saturating_mul(r.min as usize);
            let max = match (b, r.max) {
                (Some(0), _) => Some(0),
                (Some(b), Some(m)) => Some(b.saturating_mul(m as usize)),
                _ => None,
            };
            (min, max)
        }
    }
}

fn lower_assertion(a: Assertion, span: Span, ctx: Ctx) -> Result<Re> {
    let unknown = |what: &str| {
        Err(Error::unsupported(
            span,
            format!(
                "{what} here depends on the surrounding text; assertions inside the pattern \
                 are analysed from phase P4 (only leading/trailing anchors are handled now)"
            ),
        ))
    };
    match a {
        Assertion::TextStart => match ctx.at_start {
            Tri::Yes => Ok(Re::Eps),
            Tri::No => Ok(Re::Nothing),
            Tri::Unknown => unknown("start-of-text anchor"),
        },
        Assertion::TextEnd => match ctx.at_end {
            Tri::Yes => Ok(Re::Eps),
            Tri::No => Ok(Re::Nothing),
            Tri::Unknown => unknown("end-of-text anchor"),
        },
        Assertion::TextEndOrFinalNewline => match ctx.at_end {
            Tri::Yes => Ok(Re::Eps),
            _ => unknown("end-or-final-newline anchor"),
        },
        Assertion::LineStart { .. } => match ctx.at_start {
            Tri::Yes => Ok(Re::Eps),
            _ => unknown("start-of-line anchor"),
        },
        Assertion::LineEnd { .. } => match ctx.at_end {
            Tri::Yes => Ok(Re::Eps),
            _ => unknown("end-of-line anchor"),
        },
        _ => unknown("word-boundary assertion"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Flavor, parse};

    fn low(p: &str, op: Op) -> Result<Re> {
        lower(&parse(Flavor::Rust, p).unwrap(), op, &Budget::default())
    }

    #[test]
    fn edge_anchors() {
        assert_eq!(
            low("^a$", Op::FullMatch).unwrap().to_string(),
            "(cat ε [a] ε)"
        );
        assert_eq!(
            low("a^b", Op::FullMatch).unwrap().to_string(),
            "(cat [a] ∅ [b])"
        );
        assert_eq!(
            low("^a", Op::Search).unwrap().to_string(),
            "(cat ε [a] (rep{0,} [ANY]))"
        );
        assert!(low("(^a)*", Op::FullMatch).is_err());
        assert!(low(r"\ba", Op::FullMatch).is_err());
    }

    #[test]
    fn naive_matcher() {
        let r = low("a(b|c)*d", Op::FullMatch).unwrap();
        assert!(r.matches_naive("ad"));
        assert!(r.matches_naive("abcbd"));
        assert!(!r.matches_naive("abc"));
        let r = Re::Not(Box::new(Re::string("ab")));
        assert!(r.matches_naive("") && r.matches_naive("abc") && !r.matches_naive("ab"));
        let r = Re::Repeat(Box::new(Re::Alt(vec![Re::Eps, Re::lit('a')])), 2, Some(3));
        assert!(r.matches_naive("") && r.matches_naive("aaa") && !r.matches_naive("aaaa"));
    }
}
