//! Backtracking reference interpreter over the IR (ADR-0011).
//!
//! Exact for every IR construct, including the non-regular ones (back-references,
//! conditionals) and atomic/possessive groups whose bodies are not prefix-free. It mirrors the
//! engines' observable rules that matter for match/no-match:
//!
//! - priority order: alternatives left to right, greedy before lazy, atomic groups commit;
//! - a repetition iteration that consumes nothing after `min` is rejected (ECMAScript rule,
//!   also what terminates the other engines' loops);
//! - back-reference to a group that has not participated: matches empty in JavaScript, fails
//!   in Python and Java; JavaScript clears a quantified atom's captures at each iteration;
//! - case-insensitive back-references compare with each engine's rule ([`BackrefCase`]).
//!
//! It is the bounded checker's matcher and an independent cross-check of the automata.

use crate::charset::CharSet;
use crate::error::{Error, Result};
use crate::flavor::Flavor;
use crate::ir::{Assertion, BackrefCase, LineTerms, Node, NodeKind, Regex, WordSet};
use crate::regular::Op;
use crate::tables;

type Caps = Vec<Option<(usize, usize)>>;

/// Out of steps: the pattern is too expensive to interpret on this input (e.g. ReDoS).
struct Stop;

type R = std::result::Result<bool, Stop>;

pub struct Interp<'a> {
    /// Code points, or (JS without `u`) UTF-16 code units with surrogates as pseudo points.
    input: &'a [u32],
    steps: std::cell::Cell<u64>,
    limit: u64,
    js: bool,
}

/// Does `re` accept `input` under `op`? `Err` (budget) when the step limit is exceeded.
pub fn is_match(re: &Regex, op: Op, input: &str, step_limit: u64) -> Result<bool> {
    let chars: Vec<u32> = if re.domain == crate::ir::Domain::Utf16 {
        input
            .encode_utf16()
            .map(|u| match u {
                0xD800..=0xDFFF => crate::charset::PSEUDO_BASE + (u32::from(u) - 0xD800),
                u => u32::from(u),
            })
            .collect()
    } else {
        input.chars().map(|c| c as u32).collect()
    };
    let it = Interp {
        input: &chars,
        steps: std::cell::Cell::new(0),
        limit: step_limit,
        js: re.flavor == Flavor::JavaScript,
    };
    let groups = re.captures.len() + 1;
    let n = chars.len();
    let run = |start: usize| -> R {
        let mut caps: Caps = vec![None; groups];
        let mut done = |p: usize, _: &mut Caps| Ok(op == Op::Search || p == n);
        it.m(&re.root, start, &mut caps, &mut done)
    };
    let starts: Vec<usize> = match op {
        Op::FullMatch => vec![0],
        Op::Search if re.quirks.sticky => vec![0],
        Op::Search => (0..=n).collect(),
    };
    // V8 `u`-mode search also tries positions inside surrogate pairs (docs/findings.md).
    if op == Op::Search
        && re.quirks.mid_pair_search
        && chars.iter().any(|&c| c > 0xFFFF)
        && crate::regular::matches_empty_mid_pair(&re.root).unwrap_or(false)
    {
        return Ok(true);
    }
    for s in starts {
        match run(s) {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(Stop) => {
                return Err(Error::budget(format!(
                    "interpreter step limit of {step_limit} exceeded (catastrophic backtracking?)"
                )));
            }
        }
    }
    Ok(false)
}

impl Interp<'_> {
    fn tick(&self) -> std::result::Result<(), Stop> {
        let s = self.steps.get() + 1;
        self.steps.set(s);
        if s > self.limit { Err(Stop) } else { Ok(()) }
    }

    /// The character at `i` for predicates; a pseudo code unit reads as U+FFFE (a non-word,
    /// non-terminator character), which is how a lone surrogate behaves in every predicate.
    fn at(&self, i: usize) -> Option<char> {
        self.input
            .get(i)
            .map(|&c| char::from_u32(c).unwrap_or('\u{FFFE}'))
    }

    fn raw(&self, i: usize) -> Option<u32> {
        self.input.get(i).copied()
    }

    fn m(
        &self,
        n: &Node,
        pos: usize,
        caps: &mut Caps,
        k: &mut dyn FnMut(usize, &mut Caps) -> R,
    ) -> R {
        self.tick()?;
        match &n.kind {
            NodeKind::Empty => k(pos, caps),
            NodeKind::Literal(c) => {
                if self.raw(pos) == Some(*c as u32) {
                    k(pos + 1, caps)
                } else {
                    Ok(false)
                }
            }
            NodeKind::Class(s) => match self.raw(pos) {
                Some(c) if s.contains_u32(c) => k(pos + 1, caps),
                _ => Ok(false),
            },
            NodeKind::Concat(xs) => self.seq(xs, 0, pos, caps, k),
            NodeKind::Alt(xs) => {
                for x in xs {
                    if self.m(x, pos, caps, k)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            NodeKind::Group(g) if g.atomic => self.atomic(&g.sub, pos, caps, k),
            NodeKind::Group(g) => match &g.capture {
                None => self.m(&g.sub, pos, caps, k),
                Some(c) => {
                    let idx = c.index as usize;
                    self.m(&g.sub, pos, caps, &mut |p, caps: &mut Caps| {
                        let saved = caps[idx];
                        caps[idx] = Some((pos, p));
                        if k(p, caps)? {
                            return Ok(true);
                        }
                        caps[idx] = saved;
                        Ok(false)
                    })
                }
            },
            NodeKind::Repeat(r) if r.possessive => {
                // Possessive = atomic over the greedy repetition.
                let greedy = Node::new(
                    NodeKind::Repeat(crate::ir::Repeat {
                        sub: r.sub.clone(),
                        min: r.min,
                        max: r.max,
                        greedy: true,
                        possessive: false,
                    }),
                    n.span,
                );
                self.atomic(&greedy, pos, caps, k)
            }
            NodeKind::Repeat(r) => {
                let groups = if self.js {
                    capture_indices(&r.sub)
                } else {
                    Vec::new()
                };
                self.rep(&r.sub, r.min, r.max, r.greedy, &groups, 0, pos, caps, k)
            }
            NodeKind::Assert(a) => {
                if self.holds(*a, pos) {
                    k(pos, caps)
                } else {
                    Ok(false)
                }
            }
            NodeKind::Look(l) if !l.behind => {
                let saved = caps.clone();
                let found = self.m(&l.sub, pos, caps, &mut |_, _| Ok(true))?;
                if l.negated {
                    *caps = saved;
                    if found { Ok(false) } else { k(pos, caps) }
                } else if found {
                    if k(pos, caps)? {
                        return Ok(true);
                    }
                    *caps = saved;
                    Ok(false)
                } else {
                    *caps = saved;
                    Ok(false)
                }
            }
            NodeKind::Look(l) => {
                let saved = caps.clone();
                let (lo, hi) = if l.window_empty {
                    (1, 0)
                } else {
                    match l.window {
                        Some((min, max)) => (
                            max.map_or(0, |m| pos.saturating_sub(m as usize)),
                            pos.checked_sub(min as usize).unwrap_or(usize::MAX),
                        ),
                        None => (0, pos),
                    }
                };
                let mut found = false;
                if hi != usize::MAX && lo <= hi {
                    for j in (lo..=hi).rev() {
                        if self.m(&l.sub, j, caps, &mut |p, _| Ok(p == pos))? {
                            found = true;
                            break;
                        }
                    }
                }
                if l.negated {
                    *caps = saved;
                    if found { Ok(false) } else { k(pos, caps) }
                } else if found {
                    if k(pos, caps)? {
                        return Ok(true);
                    }
                    *caps = saved;
                    Ok(false)
                } else {
                    *caps = saved;
                    Ok(false)
                }
            }
            NodeKind::Backref(g, case) => {
                let g = *g as usize;
                match caps.get(g).copied().flatten() {
                    None => {
                        if self.js {
                            k(pos, caps)
                        } else {
                            Ok(false)
                        }
                    }
                    Some((s, e)) => {
                        let len = e - s;
                        if pos + len > self.input.len() {
                            return Ok(false);
                        }
                        for i in 0..len {
                            let (x, y) = (self.input[s + i], self.input[pos + i]);
                            let eq = x == y
                                || match (char::from_u32(x), char::from_u32(y)) {
                                    (Some(a), Some(b)) => case_eq(*case, a, b),
                                    _ => false,
                                };
                            if !eq {
                                return Ok(false);
                            }
                        }
                        k(pos + len, caps)
                    }
                }
            }
            NodeKind::Conditional(c) => {
                let set = caps.get(c.group as usize).copied().flatten().is_some();
                match (set, &c.no) {
                    (true, _) => self.m(&c.yes, pos, caps, k),
                    (false, Some(no)) => self.m(no, pos, caps, k),
                    (false, None) => k(pos, caps),
                }
            }
        }
    }

    fn seq(
        &self,
        xs: &[Node],
        i: usize,
        pos: usize,
        caps: &mut Caps,
        k: &mut dyn FnMut(usize, &mut Caps) -> R,
    ) -> R {
        match xs.split_first() {
            None => k(pos, caps),
            Some((x, rest)) => {
                let _ = i;
                self.m(x, pos, caps, &mut |p, caps: &mut Caps| {
                    self.seq(rest, i + 1, p, caps, k)
                })
            }
        }
    }

    /// First match of `sub` alone, then commit.
    fn atomic(
        &self,
        sub: &Node,
        pos: usize,
        caps: &mut Caps,
        k: &mut dyn FnMut(usize, &mut Caps) -> R,
    ) -> R {
        let saved = caps.clone();
        let mut end = None;
        self.m(sub, pos, caps, &mut |p, _| {
            end = Some(p);
            Ok(true)
        })?;
        match end {
            Some(p) => {
                if k(p, caps)? {
                    return Ok(true);
                }
                *caps = saved;
                Ok(false)
            }
            None => {
                *caps = saved;
                Ok(false)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn rep(
        &self,
        sub: &Node,
        min: u32,
        max: Option<u32>,
        greedy: bool,
        groups: &[usize],
        count: u32,
        pos: usize,
        caps: &mut Caps,
        k: &mut dyn FnMut(usize, &mut Caps) -> R,
    ) -> R {
        self.tick()?;
        if count > 100_000 {
            return Err(Stop);
        }
        let more = max.is_none_or(|m| count < m);
        let iterate = |caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> R| -> R {
            if !more {
                return Ok(false);
            }
            // ECMAScript RepeatMatcher: clear the atom's captures at each iteration.
            let saved: Vec<(usize, Option<(usize, usize)>)> =
                groups.iter().map(|&g| (g, caps[g])).collect();
            for &g in groups {
                caps[g] = None;
            }
            let r = self.m(sub, pos, caps, &mut |p, caps: &mut Caps| {
                if p == pos && count >= min {
                    return Ok(false); // an empty iteration after `min` adds nothing
                }
                self.rep(sub, min, max, greedy, groups, count + 1, p, caps, k)
            })?;
            if !r {
                for (g, v) in saved {
                    caps[g] = v;
                }
            }
            Ok(r)
        };
        if greedy {
            if iterate(caps, k)? {
                return Ok(true);
            }
            if count >= min {
                k(pos, caps)
            } else {
                Ok(false)
            }
        } else {
            if count >= min && k(pos, caps)? {
                return Ok(true);
            }
            iterate(caps, k)
        }
    }

    fn word_at(&self, word: WordSet, i: usize) -> bool {
        let Some(c) = self.at(i) else { return false };
        if word != WordSet::Java {
            return word.chars().contains(c);
        }
        if word.chars().contains(c) {
            return true;
        }
        let (mn, _) = tables::java_marks();
        if !mn.contains(c) {
            return false;
        }
        // Java: a mark counts when, walking back one UTF-16 unit at a time, a letter or digit
        // is reached through marks only. An astral character is met as its low surrogate,
        // which stops the scan — so only BMP characters can be passed.
        self.java_base_before(i)
    }

    /// `hasBaseCharacter(i)` for the mark at scalar index `i` (Pattern.java).
    fn java_base_before(&self, i: usize) -> bool {
        let (mn, ld) = tables::java_marks();
        let mut x = i;
        loop {
            if x == 0 {
                return false;
            }
            x -= 1;
            let Some(d) = self.at(x) else { return false };
            if d as u32 > 0xFFFF {
                return false;
            }
            if ld.contains(d) {
                return true;
            }
            if !mn.contains(d) {
                return false;
            }
        }
    }

    /// Java's left side: the char before is word, or a BMP mark with a base before it. (The
    /// scan starts at the left char's last unit: an astral mark is a lone low surrogate there.)
    fn java_left_word(&self, pos: usize) -> bool {
        let Some(c) = pos.checked_sub(1).and_then(|i| self.at(i)) else {
            return false;
        };
        let w = WordSet::Java.chars();
        if w.contains(c) {
            return true;
        }
        let (mn, _) = tables::java_marks();
        (c as u32) <= 0xFFFF && mn.contains(c) && self.java_base_before(pos - 1)
    }

    fn holds(&self, a: Assertion, pos: usize) -> bool {
        let n = self.input.len();
        let prev = pos.checked_sub(1).and_then(|i| self.at(i));
        let next = self.at(pos);
        let java_lt = |c: char| matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}');
        let terms = |t: LineTerms, c: char| match t {
            LineTerms::Lf => c == '\n',
            LineTerms::Js => matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'),
            LineTerms::CrLf => c == '\n' || c == '\r',
        };
        match a {
            Assertion::TextStart => pos == 0,
            Assertion::TextEnd => pos == n,
            Assertion::TextEndOrFinalNewline => pos == n || (pos + 1 == n && next == Some('\n')),
            Assertion::LineStart { terms: t } => match prev {
                None => true,
                Some(p) if t == LineTerms::CrLf => p == '\n' || (p == '\r' && next != Some('\n')),
                Some(p) => terms(t, p),
            },
            Assertion::LineEnd { terms: t } => match next {
                None => true,
                Some(c) if t == LineTerms::CrLf => c == '\r' || (c == '\n' && prev != Some('\r')),
                Some(c) => terms(t, c),
            },
            Assertion::JavaDollar { unix: true } => {
                pos == n || (pos + 1 == n && next == Some('\n'))
            }
            Assertion::JavaDollar { unix: false } => {
                pos == n
                    || (pos + 2 == n && next == Some('\r') && self.at(pos + 1) == Some('\n'))
                    || (pos + 1 == n
                        && match next {
                            Some('\n') => prev != Some('\r'),
                            Some(c) => java_lt(c),
                            None => false,
                        })
            }
            Assertion::JavaLineEnd { unix } => match next {
                None => true,
                Some('\n') => unix || prev != Some('\r'),
                Some(c) => !unix && java_lt(c),
            },
            Assertion::JavaLineStart { unix } => {
                pos != n
                    && match prev {
                        None => true,
                        Some(p) if unix => p == '\n',
                        Some(p) => java_lt(p) && !(p == '\r' && next == Some('\n')),
                    }
            }
            Assertion::WordBoundary { word }
            | Assertion::NotWordBoundary { word }
            | Assertion::WordStart { word }
            | Assertion::WordEnd { word }
            | Assertion::WordStartHalf { word }
            | Assertion::WordEndHalf { word } => {
                let left = if word == WordSet::Java {
                    self.java_left_word(pos)
                } else {
                    pos > 0 && self.word_at(word, pos - 1)
                };
                let right = pos < n && self.word_at(word, pos);
                match a {
                    Assertion::WordBoundary { .. } => left != right,
                    Assertion::NotWordBoundary { .. } => left == right,
                    Assertion::WordStart { .. } => !left && right,
                    Assertion::WordEnd { .. } => left && !right,
                    Assertion::WordStartHalf { .. } => !left,
                    _ => !right,
                }
            }
        }
    }
}

fn capture_indices(n: &Node) -> Vec<usize> {
    let mut v = Vec::new();
    n.walk(&mut |m| {
        if let NodeKind::Group(g) = &m.kind
            && let Some(c) = &g.capture
        {
            v.push(c.index as usize);
        }
    });
    v
}

/// Character comparison of a back-reference.
fn case_eq(case: BackrefCase, a: char, b: char) -> bool {
    if a == b {
        return true;
    }
    let rel = |r: &[(u32, &[u32])]| tables::fold_with(&CharSet::single(a), r).contains(b);
    match case {
        BackrefCase::Exact => false,
        BackrefCase::Python { ascii: true } | BackrefCase::Java { unicode: false } => {
            a.is_ascii() && b.is_ascii() && a.eq_ignore_ascii_case(&b)
        }
        BackrefCase::Python { ascii: false } => rel(tables::python::FOLD),
        BackrefCase::Js { unicode: false } => rel(tables::js::FOLD),
        BackrefCase::Js { unicode: true } => rel(tables::js::FOLD_U),
        BackrefCase::Java { unicode: true } => {
            use crate::flavor::java::{lower, upper};
            let (ua, ub) = (upper(a as u32), upper(b as u32));
            ua == ub || lower(ua) == lower(ub)
        }
    }
}
