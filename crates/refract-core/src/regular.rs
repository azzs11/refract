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
    /// Least fixed point `μ var. body` (ADR-0008): the continuation of a repetition that
    /// contains a look-ahead, `X = k | r·X`. Only the derivative engine handles it.
    Fix {
        var: u32,
        body: Box<Re>,
    },
    /// A reference to the enclosing `Fix` with this `var`.
    Var(u32),
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
            Re::Nothing | Re::Eps | Re::Set(_) | Re::Var(_) => false,
            Re::Concat(xs) | Re::Alt(xs) | Re::And(xs) => xs.iter().any(Re::has_behind),
            Re::Repeat(r, _, _) | Re::Not(r) => r.has_behind(),
            Re::Fix { body, .. } => body.has_behind(),
        }
    }

    /// Number of nodes, for the lowering size budget.
    pub fn size(&self) -> usize {
        1 + match self {
            Re::Nothing | Re::Eps | Re::Set(_) | Re::Var(_) => 0,
            Re::Concat(xs) | Re::Alt(xs) | Re::And(xs) => xs.iter().map(Re::size).sum(),
            Re::Repeat(r, _, _) | Re::Not(r) => r.size(),
            Re::Behind { sub, .. } => sub.size(),
            Re::Fix { body, .. } => body.size(),
        }
    }

    /// Every character set mentioned, for building the minterm alphabet.
    pub fn collect_sets(&self, out: &mut Vec<CharSet>) {
        match self {
            Re::Nothing | Re::Eps | Re::Var(_) => {}
            Re::Fix { body, .. } => body.collect_sets(out),
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
        // Fixed points unfolded n + 2 times: only iterations that consume input matter.
        let re = self.unfold(chars.len() + 2);
        re.ends_from(&chars, 0).contains(&chars.len())
    }

    /// Replaces every `Fix` by `depth` unfoldings (the innermost by ∅).
    fn unfold(&self, depth: usize) -> Re {
        match self {
            Re::Fix { var, body } => {
                let mut acc = Re::Nothing;
                for _ in 0..depth {
                    acc = body.substitute(*var, &acc).unfold(depth);
                }
                acc
            }
            Re::Nothing | Re::Eps | Re::Set(_) | Re::Var(_) => self.clone(),
            Re::Concat(xs) => Re::Concat(xs.iter().map(|x| x.unfold(depth)).collect()),
            Re::Alt(xs) => Re::Alt(xs.iter().map(|x| x.unfold(depth)).collect()),
            Re::And(xs) => Re::And(xs.iter().map(|x| x.unfold(depth)).collect()),
            Re::Repeat(r, a, b) => Re::Repeat(Box::new(r.unfold(depth)), *a, *b),
            Re::Not(x) => Re::Not(Box::new(x.unfold(depth))),
            Re::Behind { sub, negated } => Re::Behind {
                sub: Box::new(sub.unfold(depth)),
                negated: *negated,
            },
        }
    }

    fn substitute(&self, v: u32, with: &Re) -> Re {
        match self {
            Re::Var(w) if *w == v => with.clone(),
            Re::Nothing | Re::Eps | Re::Set(_) | Re::Var(_) => self.clone(),
            Re::Fix { var, body } => Re::Fix {
                var: *var,
                body: Box::new(if *var == v {
                    (**body).clone()
                } else {
                    body.substitute(v, with)
                }),
            },
            Re::Concat(xs) => Re::Concat(xs.iter().map(|x| x.substitute(v, with)).collect()),
            Re::Alt(xs) => Re::Alt(xs.iter().map(|x| x.substitute(v, with)).collect()),
            Re::And(xs) => Re::And(xs.iter().map(|x| x.substitute(v, with)).collect()),
            Re::Repeat(r, a, b) => Re::Repeat(Box::new(r.substitute(v, with)), *a, *b),
            Re::Not(x) => Re::Not(Box::new(x.substitute(v, with))),
            Re::Behind { sub, negated } => Re::Behind {
                sub: Box::new(sub.substitute(v, with)),
                negated: *negated,
            },
        }
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
            // Already unfolded by `matches_naive`; a leftover reference matches nothing.
            Re::Fix { .. } | Re::Var(_) => vec![],
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
            Re::Fix { var, body } => write!(f, "(fix x{var} {body})"),
            Re::Var(v) => write!(f, "x{v}"),
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
    let lw = Lowerer {
        budget,
        next_var: std::cell::Cell::new(0),
        // JS without `u` matches UTF-16 code units (ADR-0007): Σ is every code unit.
        sigma: match re.domain {
            crate::ir::Domain::Scalars => CharSet::full(),
            crate::ir::Domain::Utf16 => CharSet::units(),
        },
    };
    let lang = match op {
        Op::FullMatch => lw.node(&re.root, Re::Eps)?,
        Op::Search if re.quirks.sticky => lw.node(&re.root, lw.any_star())?,
        Op::Search => {
            let normal = Re::Concat(vec![lw.any_star(), lw.node(&re.root, lw.any_star())?]);
            if re.quirks.mid_pair_search && matches_empty_mid_pair(&re.root)? {
                // Any haystack with an astral char offers such a position.
                let astral = Re::Set(CharSet::from_ranges([(0x1_0000, crate::charset::MAX_CHAR)]));
                Re::Alt(vec![
                    normal,
                    Re::Concat(vec![lw.any_star(), astral, lw.any_star()]),
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
    // For `Utf16` this is a language over code units; `utf16::scalar_dfa` lifts it to
    // haystacks of scalar values by the inverse UTF-16 homomorphism.
    Ok(lang)
}

/// The language "some match starts at one of the first `k + 1` positions": `Σ{0,k} · L · Σ*`,
/// with the engine's context (`^`, `\b`, look-behind) seen correctly at each start. The ReDoS
/// lab uses it to check that every early start of an attack fails.
pub fn lower_starts_within(re: &Regex, k: u32, budget: &Budget) -> Result<Re> {
    let lw = Lowerer {
        budget,
        next_var: std::cell::Cell::new(0),
        sigma: match re.domain {
            crate::ir::Domain::Scalars => CharSet::full(),
            crate::ir::Domain::Utf16 => CharSet::units(),
        },
    };
    Ok(Re::Concat(vec![
        Re::Repeat(Box::new(lw.any()), 0, Some(k)),
        lw.node(&re.root, lw.any_star())?,
    ]))
}

/// Can the pattern match the empty string at a V8 mid-surrogate-pair position (see
/// [`crate::ir::Quirks`])? There no character is visible on either side and the text-edge
/// anchors are false, so only empty matches are possible.
pub(crate) fn matches_empty_mid_pair(n: &Node) -> Result<bool> {
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
        NodeKind::Backref(..) | NodeKind::Conditional(_) => {
            return Err(Error::unsupported(n.span, "construct not analysed yet"));
        }
    })
}

/// Does the term accept the empty string (no look-arounds or fixed points inside)?
fn body_nullable(re: &Re) -> bool {
    re.matches_naive("")
}

/// Continuation threading copies `k` into every alternative that contains a look-ahead; this
/// bounds the result.
const MAX_LOWERED_SIZE: usize = 200_000;
/// Optional copies of a bounded repetition containing a look-ahead are unrolled up to this.
const MAX_CPS_UNROLL: u32 = 64;

struct Lowerer<'a> {
    budget: &'a Budget,
    /// Fresh `Fix` variables.
    next_var: std::cell::Cell<u32>,
    /// Σ: every character a haystack position can hold.
    sigma: CharSet,
}

fn behind(sub: Re, negated: bool) -> Re {
    Re::Behind {
        sub: Box::new(sub),
        negated,
    }
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
            // Atomic groups and possessive repetitions constrain what follows.
            NodeKind::Group(g) => g.atomic,
            NodeKind::Repeat(r) => r.possessive,
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
    fn any(&self) -> Re {
        Re::Set(self.sigma.clone())
    }

    fn any_star(&self) -> Re {
        Re::star(self.any())
    }

    /// `s · Σ*`: "the rest of the input starts with s".
    fn starts_with(&self, s: Re) -> Re {
        Re::Concat(vec![s, self.any_star()])
    }

    /// Java's `\b` (Pattern.Bound): a side is "word" when its char is an ASCII word char, or a
    /// non-spacing mark whose run of marks follows a Unicode letter or digit (`hasBaseCharacter`).
    fn java_word_boundary(&self, boundary: bool, k: Re) -> Re {
        let (mn, ld) = crate::tables::java_marks();
        // `hasBaseCharacter` walks back one UTF-16 unit at a time: an astral character is met
        // as its low surrogate and stops the scan, so the scan only passes BMP characters.
        let bmp = CharSet::from_ranges([(0, 0xFFFF)]);
        let w = Re::Set(crate::unicode::posix_class("word").unwrap_or_default());
        let mn_any = Re::Set(mn.clone());
        let mn = Re::Set(mn.intersect(&bmp));
        let ld = Re::Set(ld.intersect(&bmp));
        // Left char is word: prefix ends in W, or in LD·Mn+ (all BMP).
        let left_word = Re::Alt(vec![
            w.clone(),
            Re::Concat(vec![ld.clone(), Re::Repeat(Box::new(mn.clone()), 1, None)]),
        ]);
        let lw = behind(left_word.clone(), false);
        let not_lw = behind(left_word, true);
        // Right char is word: next is W, or next is Mn (any) and the prefix ends in LD·Mn*.
        let base = Re::Concat(vec![ld, Re::star(mn.clone())]);
        let mn = mn_any;
        let rw = |k: Re| {
            Re::Alt(vec![
                and(self.starts_with(w.clone()), k.clone()),
                cat(
                    behind(base.clone(), false),
                    and(self.starts_with(mn.clone()), k),
                ),
            ])
        };
        let not_rw = |k: Re| {
            let not_w = Re::Not(Box::new(self.starts_with(w.clone())));
            Re::Alt(vec![
                and(
                    Re::And(vec![
                        not_w.clone(),
                        Re::Not(Box::new(self.starts_with(mn.clone()))),
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
            NodeKind::Group(g) if g.atomic => self.atomic(&g.sub, k)?,
            NodeKind::Group(g) => self.node(&g.sub, k)?,
            NodeKind::Repeat(r) if r.possessive => self.atomic(n, k)?,
            NodeKind::Repeat(r) => {
                let mut acc = match r.max {
                    // Optional copies, innermost first.
                    Some(max) if max - r.min <= MAX_CPS_UNROLL => {
                        let mut acc = k.clone();
                        for _ in r.min..max {
                            acc = Re::Alt(vec![k.clone(), self.node(&r.sub, acc)?]);
                        }
                        acc
                    }
                    Some(_) => {
                        return Err(Error::budget(format!(
                            "a look-ahead inside a repetition with more than {MAX_CPS_UNROLL} \
                             optional copies at {}",
                            n.span
                        )));
                    }
                    // Unbounded: the least fixed point X = k | r·X.
                    None => {
                        let v = self.next_var.get();
                        self.next_var.set(v + 1);
                        let body = Re::Alt(vec![k.clone(), self.node(&r.sub, Re::Var(v))?]);
                        Re::Fix {
                            var: v,
                            body: Box::new(body),
                        }
                    }
                };
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
                let inner = self.node(&l.sub, self.any_star())?;
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

    /// Is the node's language prefix-free (no match is a proper prefix of another)? Then its
    /// first match at a position is its only match, so atomicity cannot change anything.
    /// Decided on the minimal DFA: no accepting state reaches an accepting state by a
    /// non-empty path. `None` when the node is outside what this check handles.
    fn prefix_free(&self, n: &Node) -> Result<Option<Re>> {
        if needs_continuation(n) {
            return Ok(None);
        }
        let mut has_behind = false;
        n.walk(&mut |m| has_behind |= matches!(m.kind, NodeKind::Look(_) | NodeKind::Assert(_)));
        if has_behind {
            return Ok(None);
        }
        let re = self.local(n)?;
        let alpha = crate::automata::alphabet_for(&[&re]);
        let d = crate::minimize::hopcroft(&crate::deriv::to_dfa(&re, &alpha, self.budget)?);
        let k = d.k();
        for q in (0..d.len()).filter(|&q| d.accepting[q]) {
            let mut seen = vec![false; d.len()];
            let mut stack: Vec<usize> = (0..k).map(|c| d.next(q, c)).collect();
            while let Some(p) = stack.pop() {
                if seen[p] {
                    continue;
                }
                seen[p] = true;
                if d.accepting[p] {
                    return Ok(None);
                }
                stack.extend((0..k).map(|c| d.next(p, c)));
            }
        }
        Ok(Some(re))
    }

    /// `(?>n)` (or a possessive repetition `n`) before continuation `k` — see ADR-0010.
    fn atomic(&self, n: &Node, k: Re) -> Result<Re> {
        let refuse = || {
            Err(Self::unsupported(
                n.span,
                "atomic/possessive over a body whose matches are not prefix-free (e.g. (?>a*b)) is not analysed yet",
            ))
        };
        match &n.kind {
            NodeKind::Group(g) => self.atomic(&g.sub, k),
            NodeKind::Empty | NodeKind::Assert(_) | NodeKind::Look(_) => self.node(n, k),
            NodeKind::Alt(xs) => {
                // The first alternative that matches here wins.
                let mut langs = Vec::new();
                for x in xs {
                    match self.prefix_free(x)? {
                        Some(l) => langs.push(l),
                        None => return refuse(),
                    }
                }
                let mut alts = Vec::new();
                for (i, l) in langs.iter().enumerate() {
                    let mut parts: Vec<Re> = langs[..i]
                        .iter()
                        .map(|p| Re::Not(Box::new(self.starts_with(p.clone()))))
                        .collect();
                    parts.push(cat(l.clone(), k.clone()));
                    alts.push(if parts.len() == 1 {
                        parts.pop().unwrap_or(Re::Nothing)
                    } else {
                        Re::And(parts)
                    });
                }
                Ok(Re::Alt(alts))
            }
            NodeKind::Concat(xs) if !xs.is_empty() => {
                if let Some(l) = self.prefix_free(n)? {
                    return Ok(cat(l, k));
                }
                // (?>y z) = (?>y)(?>z) when y has a single match.
                let (head, rest) = (&xs[0], &xs[1..]);
                if self.prefix_free(head)?.is_none() {
                    return refuse();
                }
                let rest_node = Node::new(NodeKind::Concat(rest.to_vec()), n.span);
                let tail = self.atomic(&rest_node, k)?;
                self.atomic(head, tail)
            }
            NodeKind::Repeat(r) => {
                let Some(body) = self.prefix_free(&r.sub)? else {
                    return refuse();
                };
                if body_nullable(&body) {
                    return refuse();
                }
                if !r.greedy && !r.possessive {
                    // Lazy inside an atomic group: the group's first match is `min` copies.
                    return Ok(cat(Re::Repeat(Box::new(body), r.min, Some(r.min)), k));
                }
                let stop = |k: Re| and(Re::Not(Box::new(self.starts_with(body.clone()))), k);
                Ok(match r.max {
                    None => cat(Re::Repeat(Box::new(body.clone()), r.min, None), stop(k)),
                    Some(m) if m == r.min => cat(Re::Repeat(Box::new(body), m, Some(m)), k),
                    Some(m) => Re::Alt(vec![
                        cat(
                            Re::Repeat(Box::new(body.clone()), r.min, Some(m - 1)),
                            stop(k.clone()),
                        ),
                        cat(Re::Repeat(Box::new(body), m, Some(m)), k),
                    ]),
                })
            }
            _ => match self.prefix_free(n)? {
                Some(l) => Ok(cat(l, k)),
                None => refuse(),
            },
        }
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
                    Re::And(vec![body, Re::Repeat(Box::new(self.any()), min, max)])
                } else {
                    body
                };
                behind(body, l.negated)
            }
            NodeKind::Assert(a) => match a {
                // (?<![\s\S]): nothing before.
                Assertion::TextStart => behind(self.any(), true),
                // (?<![^\n]) (resp. JS LineTerminators): start, or after a terminator.
                Assertion::LineStart { terms } => {
                    let t: CharSet = terms.chars().unwrap_or_default().iter().copied().collect();
                    behind(Re::Set(self.sigma.difference(&t)), true)
                }
                Assertion::WordStartHalf { word } => behind(Re::Set(word.chars()), true),
                other => {
                    return Err(Self::unsupported(
                        n.span,
                        &format!("internal: {other:?} needs a continuation"),
                    ));
                }
            },
            NodeKind::Backref(..) => {
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
                and(Re::Alt(vec![Re::Eps, self.starts_with(Re::Set(t))]), k)
            }
            // Before \r or \n or at the end, but never between \r and \n.
            Assertion::LineEnd {
                terms: LineTerms::CrLf,
            } => Re::Alt(vec![
                and(Re::Eps, k.clone()),
                and(self.starts_with(cr.clone()), k.clone()),
                cat(behind(cr, true), and(self.starts_with(nl), k)),
            ]),
            // At the start, after \n, or after \r unless \n follows.
            Assertion::LineStart {
                terms: LineTerms::CrLf,
            } => Re::Alt(vec![
                cat(behind(self.any(), true), k.clone()),
                cat(behind(nl.clone(), false), k.clone()),
                cat(
                    behind(cr, false),
                    and(Re::Not(Box::new(self.starts_with(nl))), k),
                ),
            ]),
            Assertion::TextStart
            | Assertion::LineStart { .. }
            | Assertion::WordStartHalf { .. } => {
                // Look-behind only; handled by `local`, kept total here.
                let local = match a {
                    Assertion::TextStart => behind(self.any(), true),
                    Assertion::WordStartHalf { word } => behind(Re::Set(word.chars()), true),
                    Assertion::LineStart { terms } => {
                        let t: CharSet =
                            terms.chars().unwrap_or_default().iter().copied().collect();
                        behind(Re::Set(self.sigma.difference(&t)), true)
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
                and(Re::Alt(vec![Re::Eps, self.starts_with(nl)]), k)
            }
            Assertion::JavaLineEnd { unix: false } => {
                let single = Re::Set(CharSet::from_iter(['\r', '\u{85}', '\u{2028}', '\u{2029}']));
                Re::Alt(vec![
                    and(Re::Alt(vec![Re::Eps, self.starts_with(single)]), k.clone()),
                    cat(behind(cr, true), and(self.starts_with(nl), k)),
                ])
            }
            Assertion::JavaLineStart { unix } => {
                let nonempty = self.starts_with(self.any());
                let mut alts = vec![cat(
                    behind(self.any(), true),
                    and(nonempty.clone(), k.clone()),
                )];
                if unix {
                    alts.push(cat(behind(nl, false), and(nonempty, k)));
                } else {
                    let other =
                        Re::Set(CharSet::from_iter(['\n', '\u{85}', '\u{2028}', '\u{2029}']));
                    alts.push(cat(behind(other, false), and(nonempty.clone(), k.clone())));
                    alts.push(cat(
                        behind(cr, false),
                        and(
                            Re::And(vec![nonempty, Re::Not(Box::new(self.starts_with(nl)))]),
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
            } => self.java_word_boundary(matches!(a, Assertion::WordBoundary { .. }), k),
            Assertion::WordBoundary { word }
            | Assertion::NotWordBoundary { word }
            | Assertion::WordStart { word }
            | Assertion::WordEnd { word }
            | Assertion::WordEndHalf { word } => {
                let w = Re::Set(word.chars());
                let ahead = self.starts_with(w.clone());
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
