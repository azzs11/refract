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
use crate::ir::{Assertion, Node, NodeKind, Regex, WordSet};
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
    /// Zero-width look-behind: true at a position when the whole input before it is in
    /// `Σ* · sub` (or is not, when `negated`). `sub` must not contain `Behind` itself.
    Behind {
        sub: Box<Re>,
        negated: bool,
    },
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

    /// True when the term contains a look-behind (only the derivative engine handles those).
    pub fn has_behind(&self) -> bool {
        match self {
            Re::Behind { .. } => true,
            Re::Nothing | Re::Eps | Re::Set(_) => false,
            Re::Concat(xs) | Re::Alt(xs) | Re::And(xs) => xs.iter().any(Re::has_behind),
            Re::Repeat(r, _, _) | Re::Not(r) => r.has_behind(),
        }
    }

    /// Number of nodes, for the lowering size budget.
    pub fn size(&self) -> usize {
        1 + match self {
            Re::Nothing | Re::Eps | Re::Set(_) => 0,
            Re::Concat(xs) | Re::Alt(xs) | Re::And(xs) => xs.iter().map(Re::size).sum(),
            Re::Repeat(r, _, _) | Re::Not(r) => r.size(),
            Re::Behind { sub, .. } => sub.size(),
        }
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
            Re::Behind { sub, .. } => sub.collect_sets(out),
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
            Re::Behind { sub, negated } => {
                // The prefix chars[..i] ends with a match of `sub`.
                let holds = (0..=i).any(|j| sub.ends_from(&chars[..i], j).contains(&i));
                if holds != *negated { vec![i] } else { vec![] }
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
            Re::Behind { sub, negated } => {
                write!(f, "({}behind {sub})", if *negated { "not-" } else { "" })
            }
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

/// Lowers a parsed regex to the language of haystacks accepted by `op` (ADR-0008).
///
/// Continuation-passing: `node(n, k)` is the language of "n, then whatever `k` describes, up to
/// the end of the haystack". That makes every zero-width assertion exact:
///
/// - look-ahead `(?=s)` before continuation `k` is `(s·Σ*) ∩ k` (`(?!s)`: `¬(s·Σ*) ∩ k`);
/// - look-behind is a [`Re::Behind`] atom, evaluated by the derivative engine on the input read
///   so far;
/// - `^ $ \A \z \Z \b \B` and multi-line anchors are those two built from character sets
///   (`\b` = `(?<=w)(?!w) | (?<!w)(?=w)`).
///
/// Sub-patterns without look-ahead are lowered directly and simply concatenated with `k`, so the
/// continuation is only threaded where it is needed.
pub fn lower(re: &Regex, op: Op, budget: &Budget) -> Result<Re> {
    let lw = Lowerer { budget };
    let lang = match op {
        Op::FullMatch => lw.node(&re.root, Re::Eps)?,
        Op::Search => {
            let normal = Re::Concat(vec![Re::any_star(), lw.node(&re.root, Re::any_star())?]);
            if re.quirks.v8_mid_surrogate_search && matches_empty_mid_pair(&re.root)? {
                // Any haystack with an astral char offers such a position.
                let astral = Re::Set(CharSet::from_ranges([(0x1_0000, crate::charset::MAX_CHAR)]));
                Re::Alt(vec![
                    normal,
                    Re::Concat(vec![Re::any_star(), astral, Re::any_star()]),
                ])
            } else {
                normal
            }
        }
    };
    let size = lang.size();
    if size > MAX_LOWERED_SIZE {
        return Err(Error::budget(format!(
            "lowered term has {size} nodes, over the limit of {MAX_LOWERED_SIZE} (many look-aheads \
             after alternations); split the regex"
        )));
    }
    Ok(match re.domain {
        crate::ir::Domain::Scalars => lang,
        crate::ir::Domain::Bmp => Re::And(vec![
            lang,
            Re::star(Re::Set(CharSet::from_ranges([(0, 0xFFFF)]))),
        ]),
    })
}

/// Java's `\b` (Pattern.Bound): a side is "word" when its char is an ASCII word char, or a
/// non-spacing mark whose run of marks follows a Unicode letter or digit (`hasBaseCharacter`).
fn java_word_boundary(boundary: bool, k: Re) -> Re {
    let (mn, ld) = crate::tables::java_marks();
    let w = Re::Set(crate::unicode::posix_class("word").unwrap_or_default());
    let mn = Re::Set(mn.clone());
    let ld = Re::Set(ld.clone());
    // Left char is word: prefix ends in W, or in LD·Mn+.
    let left_word = Re::Alt(vec![
        w.clone(),
        Re::Concat(vec![ld.clone(), Re::Repeat(Box::new(mn.clone()), 1, None)]),
    ]);
    let lw = behind(left_word.clone(), false);
    let not_lw = behind(left_word, true);
    // Right char is word: next is W, or next is Mn and the prefix ends in LD·Mn*.
    let base = Re::Concat(vec![ld, Re::star(mn.clone())]);
    let rw = |k: Re| {
        Re::Alt(vec![
            and(starts_with(w.clone()), k.clone()),
            cat(behind(base.clone(), false), and(starts_with(mn.clone()), k)),
        ])
    };
    let not_rw = |k: Re| {
        let not_w = Re::Not(Box::new(starts_with(w.clone())));
        Re::Alt(vec![
            and(
                Re::And(vec![
                    not_w.clone(),
                    Re::Not(Box::new(starts_with(mn.clone()))),
                ]),
                k.clone(),
            ),
            cat(behind(base.clone(), true), and(not_w, k)),
        ])
    };
    if boundary {
        Re::Alt(vec![cat(lw, not_rw(k.clone())), cat(not_lw, rw(k))])
    } else {
        Re::Alt(vec![cat(lw, rw(k.clone())), cat(not_lw, not_rw(k))])
    }
}

/// Can the pattern match the empty string at a V8 mid-surrogate-pair position (see
/// [`crate::ir::Quirks`])? There no character is visible on either side and the text-edge
/// anchors are false, so only empty matches are possible.
fn matches_empty_mid_pair(n: &Node) -> Result<bool> {
    Ok(match &n.kind {
        NodeKind::Empty => true,
        NodeKind::Literal(_) | NodeKind::Class(_) => false,
        NodeKind::Concat(xs) => {
            for x in xs {
                if !matches_empty_mid_pair(x)? {
                    return Ok(false);
                }
            }
            true
        }
        NodeKind::Alt(xs) => {
            for x in xs {
                if matches_empty_mid_pair(x)? {
                    return Ok(true);
                }
            }
            false
        }
        NodeKind::Group(g) => matches_empty_mid_pair(&g.sub)?,
        NodeKind::Repeat(r) => r.min == 0 || matches_empty_mid_pair(&r.sub)?,
        NodeKind::Look(l) => matches_empty_mid_pair(&l.sub)? != l.negated,
        NodeKind::Assert(a) => matches!(a, Assertion::NotWordBoundary { .. }),
        NodeKind::Backref(_) | NodeKind::Conditional(_) => {
            return Err(Error::unsupported(n.span, "construct not analysed yet"));
        }
    })
}

/// Restricts a lowered language to a domain. Two verdicts are only comparable over a common
/// domain: comparing JS-without-`u` (BMP haystacks) with another engine restricts both sides to
/// BMP strings, so a witness is meaningful for both (ADR-0007).
pub fn restrict(re: Re, domain: crate::ir::Domain) -> Re {
    match domain {
        crate::ir::Domain::Scalars => re,
        crate::ir::Domain::Bmp => Re::And(vec![
            re,
            Re::star(Re::Set(CharSet::from_ranges([(0, 0xFFFF)]))),
        ]),
    }
}

/// The common domain of two regexes.
pub fn common_domain(a: &Regex, b: &Regex) -> crate::ir::Domain {
    use crate::ir::Domain;
    if a.domain == Domain::Bmp || b.domain == Domain::Bmp {
        Domain::Bmp
    } else {
        Domain::Scalars
    }
}

/// Continuation threading copies `k` into every alternative that contains a look-ahead; this
/// bounds the result.
const MAX_LOWERED_SIZE: usize = 200_000;
/// Bounded repetitions containing a look-ahead are unrolled up to this many copies.
const MAX_CPS_UNROLL: u32 = 16;

struct Lowerer<'a> {
    budget: &'a Budget,
}

fn any() -> Re {
    Re::Set(CharSet::full())
}

fn behind(sub: Re, negated: bool) -> Re {
    Re::Behind {
        sub: Box::new(sub),
        negated,
    }
}

/// `s · Σ*`: "the rest of the input starts with s".
fn starts_with(s: Re) -> Re {
    Re::Concat(vec![s, Re::any_star()])
}

fn and(a: Re, k: Re) -> Re {
    Re::And(vec![a, k])
}

fn cat(a: Re, k: Re) -> Re {
    match k {
        Re::Eps => a,
        k => Re::Concat(vec![a, k]),
    }
}

/// Does the node contain anything that looks to the right of the current position?
fn needs_continuation(n: &Node) -> bool {
    let mut found = false;
    n.walk(&mut |m| {
        found |= match &m.kind {
            NodeKind::Look(l) => !l.behind,
            NodeKind::Assert(a) => !matches!(
                a,
                Assertion::TextStart
                    | Assertion::WordStartHalf { .. }
                    | Assertion::LineStart {
                        terms: crate::ir::LineTerms::Lf | crate::ir::LineTerms::Js
                    }
            ),
            _ => false,
        }
    });
    found
}

impl Lowerer<'_> {
    fn unsupported(span: Span, what: &str) -> Error {
        Error::unsupported(span, what.to_string())
    }

    /// `n` followed by `k`.
    fn node(&self, n: &Node, k: Re) -> Result<Re> {
        self.budget.check_time()?;
        if !needs_continuation(n) {
            return Ok(cat(self.local(n)?, k));
        }
        Ok(match &n.kind {
            NodeKind::Concat(xs) => {
                let mut acc = k;
                for x in xs.iter().rev() {
                    acc = self.node(x, acc)?;
                }
                acc
            }
            NodeKind::Alt(xs) => Re::Alt(
                xs.iter()
                    .map(|x| self.node(x, k.clone()))
                    .collect::<Result<_>>()?,
            ),
            NodeKind::Group(g) if g.atomic => {
                return Err(Self::unsupported(
                    n.span,
                    "atomic groups are analysed from phase P4",
                ));
            }
            NodeKind::Group(g) => self.node(&g.sub, k)?,
            NodeKind::Repeat(r) if r.possessive => {
                return Err(Self::unsupported(
                    n.span,
                    "possessive quantifiers are analysed from phase P4",
                ));
            }
            NodeKind::Repeat(r) => {
                let Some(max) = r.max.filter(|&m| m <= MAX_CPS_UNROLL) else {
                    return Err(Self::unsupported(
                        n.span,
                        "a look-ahead inside an unbounded (or very large) repetition is not \
                         analysed yet",
                    ));
                };
                // Optional copies first (innermost), then the mandatory ones.
                let mut acc = k.clone();
                for _ in r.min..max {
                    acc = Re::Alt(vec![k.clone(), self.node(&r.sub, acc)?]);
                }
                for _ in 0..r.min {
                    acc = self.node(&r.sub, acc)?;
                }
                acc
            }
            NodeKind::Look(l) if l.behind => {
                // Reaches here only because its body needs a continuation.
                return Err(Self::unsupported(
                    n.span,
                    "a look-ahead (or \\b, $) inside a look-behind is not analysed yet",
                ));
            }
            NodeKind::Look(l) => {
                let inner = self.node(&l.sub, Re::any_star())?;
                and(
                    if l.negated {
                        Re::Not(Box::new(inner))
                    } else {
                        inner
                    },
                    k,
                )
            }
            NodeKind::Assert(a) => self.assertion(*a, k),
            _ => return Err(Self::unsupported(n.span, "construct not analysed yet")),
        })
    }

    /// A node with no look-ahead inside: its own language.
    fn local(&self, n: &Node) -> Result<Re> {
        Ok(match &n.kind {
            NodeKind::Empty => Re::Eps,
            NodeKind::Literal(c) => Re::lit(*c),
            NodeKind::Class(s) => Re::Set(s.clone()),
            NodeKind::Concat(xs) => {
                Re::Concat(xs.iter().map(|x| self.local(x)).collect::<Result<_>>()?)
            }
            NodeKind::Alt(xs) => Re::Alt(xs.iter().map(|x| self.local(x)).collect::<Result<_>>()?),
            NodeKind::Group(g) if g.atomic => {
                return Err(Self::unsupported(
                    n.span,
                    "atomic groups change what matches; they are analysed from phase P4",
                ));
            }
            NodeKind::Group(g) => self.local(&g.sub)?,
            NodeKind::Repeat(r) if r.possessive => {
                return Err(Self::unsupported(
                    n.span,
                    "possessive quantifiers change what matches; they are analysed from phase P4",
                ));
            }
            NodeKind::Repeat(r) => {
                for c in [Some(r.min), r.max].into_iter().flatten() {
                    if c > self.budget.max_repeat {
                        return Err(Error::budget(format!(
                            "counted repetition {{{c}}} at {} exceeds the limit of {}",
                            n.span, self.budget.max_repeat
                        )));
                    }
                }
                Re::Repeat(Box::new(self.local(&r.sub)?), r.min, r.max)
            }
            NodeKind::Look(l) => {
                // Only look-behind reaches here (look-ahead needs the continuation).
                if needs_continuation(&l.sub) {
                    return Err(Self::unsupported(
                        n.span,
                        "a look-ahead inside a look-behind is not analysed yet",
                    ));
                }
                let body = self.local(&l.sub)?;
                let body = if l.window_empty {
                    Re::Nothing
                } else if let Some((min, max)) = l.window {
                    Re::And(vec![body, Re::Repeat(Box::new(any()), min, max)])
                } else {
                    body
                };
                behind(body, l.negated)
            }
            NodeKind::Assert(a) => match a {
                // (?<![\s\S]): nothing before.
                Assertion::TextStart => behind(any(), true),
                // (?<![^\n]) (resp. JS LineTerminators): start, or after a terminator.
                Assertion::LineStart { terms } => {
                    let t: CharSet = terms.chars().unwrap_or_default().iter().copied().collect();
                    behind(Re::Set(t.complement()), true)
                }
                Assertion::WordStartHalf { word } => behind(Re::Set(word.chars()), true),
                other => {
                    return Err(Self::unsupported(
                        n.span,
                        &format!("internal: {other:?} needs a continuation"),
                    ));
                }
            },
            NodeKind::Backref(_) => {
                return Err(Self::unsupported(
                    n.span,
                    "backreferences are not regular; bounded analysis arrives in phase P4",
                ));
            }
            NodeKind::Conditional(_) => {
                return Err(Self::unsupported(
                    n.span,
                    "conditional groups are analysed from phase P4",
                ));
            }
        })
    }

    /// An assertion that looks right, before continuation `k`.
    fn assertion(&self, a: Assertion, k: Re) -> Re {
        use crate::ir::LineTerms;
        let nl = Re::lit('\n');
        let cr = Re::lit('\r');
        match a {
            Assertion::TextEnd => and(Re::Eps, k),
            Assertion::TextEndOrFinalNewline => and(Re::Alt(vec![Re::Eps, nl]), k),
            Assertion::LineEnd {
                terms: terms @ (LineTerms::Lf | LineTerms::Js),
            } => {
                let t: CharSet = terms.chars().unwrap_or_default().iter().copied().collect();
                and(Re::Alt(vec![Re::Eps, starts_with(Re::Set(t))]), k)
            }
            // Before \r or \n or at the end, but never between \r and \n.
            Assertion::LineEnd {
                terms: LineTerms::CrLf,
            } => Re::Alt(vec![
                and(Re::Eps, k.clone()),
                and(starts_with(cr.clone()), k.clone()),
                cat(behind(cr, true), and(starts_with(nl), k)),
            ]),
            // At the start, after \n, or after \r unless \n follows.
            Assertion::LineStart {
                terms: LineTerms::CrLf,
            } => Re::Alt(vec![
                cat(behind(any(), true), k.clone()),
                cat(behind(nl.clone(), false), k.clone()),
                cat(
                    behind(cr, false),
                    and(Re::Not(Box::new(starts_with(nl))), k),
                ),
            ]),
            Assertion::TextStart
            | Assertion::LineStart { .. }
            | Assertion::WordStartHalf { .. } => {
                // Look-behind only; handled by `local`, kept total here.
                let local = match a {
                    Assertion::TextStart => behind(any(), true),
                    Assertion::WordStartHalf { word } => behind(Re::Set(word.chars()), true),
                    Assertion::LineStart { terms } => {
                        let t: CharSet =
                            terms.chars().unwrap_or_default().iter().copied().collect();
                        behind(Re::Set(t.complement()), true)
                    }
                    _ => Re::Eps,
                };
                cat(local, k)
            }
            Assertion::JavaDollar { unix: true } => and(Re::Alt(vec![Re::Eps, nl]), k),
            Assertion::JavaDollar { unix: false } => {
                let single = Re::Set(CharSet::from_iter(['\r', '\u{85}', '\u{2028}', '\u{2029}']));
                Re::Alt(vec![
                    and(
                        Re::Alt(vec![Re::Eps, Re::string("\r\n"), single]),
                        k.clone(),
                    ),
                    cat(behind(cr, true), and(nl, k)),
                ])
            }
            Assertion::JavaLineEnd { unix: true } => {
                and(Re::Alt(vec![Re::Eps, starts_with(nl)]), k)
            }
            Assertion::JavaLineEnd { unix: false } => {
                let single = Re::Set(CharSet::from_iter(['\r', '\u{85}', '\u{2028}', '\u{2029}']));
                Re::Alt(vec![
                    and(Re::Alt(vec![Re::Eps, starts_with(single)]), k.clone()),
                    cat(behind(cr, true), and(starts_with(nl), k)),
                ])
            }
            Assertion::JavaLineStart { unix } => {
                let nonempty = starts_with(any());
                let mut alts = vec![cat(behind(any(), true), and(nonempty.clone(), k.clone()))];
                if unix {
                    alts.push(cat(behind(nl, false), and(nonempty, k)));
                } else {
                    let other =
                        Re::Set(CharSet::from_iter(['\n', '\u{85}', '\u{2028}', '\u{2029}']));
                    alts.push(cat(behind(other, false), and(nonempty.clone(), k.clone())));
                    alts.push(cat(
                        behind(cr, false),
                        and(
                            Re::And(vec![nonempty, Re::Not(Box::new(starts_with(nl)))]),
                            k,
                        ),
                    ));
                }
                Re::Alt(alts)
            }
            Assertion::WordBoundary {
                word: WordSet::Java,
            }
            | Assertion::NotWordBoundary {
                word: WordSet::Java,
            } => java_word_boundary(matches!(a, Assertion::WordBoundary { .. }), k),
            Assertion::WordBoundary { word }
            | Assertion::NotWordBoundary { word }
            | Assertion::WordStart { word }
            | Assertion::WordEnd { word }
            | Assertion::WordEndHalf { word } => {
                let w = Re::Set(word.chars());
                let ahead = starts_with(w.clone());
                let not_ahead = Re::Not(Box::new(ahead.clone()));
                let after_word = |r: Re| cat(behind(w.clone(), false), r);
                let after_non = |r: Re| cat(behind(w.clone(), true), r);
                match a {
                    Assertion::WordBoundary { .. } => Re::Alt(vec![
                        after_word(and(not_ahead, k.clone())),
                        after_non(and(ahead, k)),
                    ]),
                    Assertion::NotWordBoundary { .. } => Re::Alt(vec![
                        after_word(and(ahead, k.clone())),
                        after_non(and(not_ahead, k)),
                    ]),
                    Assertion::WordStart { .. } => after_non(and(ahead, k)),
                    Assertion::WordEnd { .. } => after_word(and(not_ahead, k)),
                    _ => and(not_ahead, k),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Flavor, parse};

    fn low(p: &str, op: Op) -> Result<Re> {
        lower(&parse(Flavor::Rust, p).unwrap(), op, &Budget::default())
    }

    /// Assertions anywhere, checked against hand-computed membership.
    #[test]
    fn assertions_anywhere() {
        type Case<'a> = (&'a str, Op, &'a [(&'a str, bool)]);
        let cases: &[Case] = &[
            ("^a$", Op::FullMatch, &[("a", true), ("aa", false)]),
            ("a^b", Op::FullMatch, &[("ab", false)]),
            ("^a", Op::Search, &[("ab", true), ("ba", false)]),
            (
                "(^a)*",
                Op::FullMatch,
                &[("", true), ("a", true), ("aa", false)],
            ),
            (
                r"\ba",
                Op::Search,
                &[("a", true), ("ba", false), (" a", true)],
            ),
            (r"a\b", Op::FullMatch, &[("a", true)]),
            (r"\Bb", Op::Search, &[("ab", true), ("b", false)]),
            ("(?m)^b", Op::Search, &[("a\nb", true), ("ab", false)]),
            ("(?m)a$", Op::Search, &[("a\nb", true), ("ab", false)]),
            (
                "a$|b",
                Op::Search,
                &[("xa", true), ("ax", false), ("bx", true)],
            ),
        ];
        for (p, op, inputs) in cases {
            let re = low(p, *op).unwrap();
            let alpha = crate::automata::alphabet_for(&[&re]);
            let m =
                crate::automata::dfa(&re, &alpha, crate::Engine::Derivative, &Budget::default())
                    .unwrap();
            for (s, want) in *inputs {
                assert_eq!(m.accepts(s), *want, "{p} {op:?} on {s:?}");
                assert_eq!(re.matches_naive(s), *want, "naive: {p} {op:?} on {s:?}");
            }
        }
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
