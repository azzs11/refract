//! Front-end for Java `java.util.regex` (JDK 25). Spec: `spec/flavours/java.md`.
//!
//! A cursor-level port of OpenJDK's `Pattern.java` (`jdk-25-ga`): the same `temp` code-point
//! buffer with two zero terminators, the same `peek`/`read`/`next`/`skip`/`unread` primitives
//! (whitespace-skipping in comments mode) and the same `sequence`, `atom`, `closure`, `escape`,
//! `clazz`, `range`, `group0`, `addFlag` logic, so acceptance follows the engine exactly. Each
//! Java character predicate is turned into an explicit set using tables from
//! `java.lang.Character` (`tables::java`).

use crate::charset::CharSet;
use crate::error::{Error, Result, Span};
use crate::flavor::{Flavor, MatchPolicy, Profile};
use crate::ir::{
    Assertion, Capture, Domain, Group, Look, Node, NodeKind, Quirks, Regex, Repeat, WordSet,
};
use crate::tables::java as t;
use std::collections::HashMap;
use std::sync::OnceLock;

pub static PROFILE: Profile = Profile {
    flavor: Flavor::Java,
    engine: "java.util.regex (JDK 25)",
    unicode: "java.lang.Character of Java 25.0.5.1",
    dot_excludes: "\\n \\r \\u0085 \\u2028 \\u2029",
    dollar: "end, or before a final line terminator (incl. \\r\\n)",
    unicode_classes: false,
    case_folding: "ASCII; (?iu): toLowerCase(toUpperCase(c))",
    match_policy: MatchPolicy::LeftmostFirst,
    lookaround: true,
    backreferences: true,
    linear_time: false,
    spec: "spec/flavours/java.md",
};

const CASE_INSENSITIVE: u32 = 0x02;
const COMMENTS: u32 = 0x04;
const MULTILINE: u32 = 0x08;
const DOTALL: u32 = 0x20;
const UNICODE_CASE: u32 = 0x40;
const CANON_EQ: u32 = 0x80;
const UNIX_LINES: u32 = 0x01;
const UNICODE_CHARACTER_CLASS: u32 = 0x100;
const NEST_LIMIT: u32 = 400;

fn upper(c: u32) -> u32 {
    lookup(t::TO_UPPER, c)
}

fn lower(c: u32) -> u32 {
    lookup(t::TO_LOWER, c)
}

fn lookup(map: &[(u32, u32)], c: u32) -> u32 {
    map.binary_search_by_key(&c, |&(k, _)| k)
        .map_or(c, |i| map[i].1)
}

/// `toLowerCase(toUpperCase(c))`.
fn key(c: u32) -> u32 {
    lower(upper(c))
}

/// Code points whose upper- or lower-case mapping is not themselves.
fn cased() -> &'static [u32] {
    static S: OnceLock<Vec<u32>> = OnceLock::new();
    S.get_or_init(|| {
        let mut v: Vec<u32> = t::TO_UPPER
            .iter()
            .chain(t::TO_LOWER)
            .map(|&(c, _)| c)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    })
}

/// key → every cased code point with that key.
fn by_key() -> &'static HashMap<u32, Vec<u32>> {
    static M: OnceLock<HashMap<u32, Vec<u32>>> = OnceLock::new();
    M.get_or_init(|| {
        let mut m: HashMap<u32, Vec<u32>> = HashMap::new();
        for &c in cased() {
            m.entry(key(c)).or_default().push(c);
        }
        m
    })
}

fn points(v: impl IntoIterator<Item = u32>) -> CharSet {
    CharSet::from_ranges(v.into_iter().map(|c| (c, c)))
}

fn ascii_lower(c: u32) -> u32 {
    if (0x41..=0x5A).contains(&c) {
        c + 0x20
    } else {
        c
    }
}

fn ascii_upper(c: u32) -> u32 {
    if (0x61..=0x7A).contains(&c) {
        c - 0x20
    } else {
        c
    }
}

/// `SingleU(lower)`: `lower == ch || lower == toLowerCase(toUpperCase(ch))`.
fn single_u(l: u32) -> CharSet {
    let mut v = vec![l];
    if let Some(cs) = by_key().get(&l) {
        v.extend(cs);
    }
    points(v)
}

/// Escape results: a code point, a predicate (class escape), or a node (assertion etc.).
enum Esc {
    Char(u32),
    Pred(CharSet),
    Node(Node),
    /// Meta escape seen with `create == false`.
    None,
}

struct Parser<'a> {
    src: &'a str,
    temp: Vec<u32>,
    pattern_length: usize,
    cursor: usize,
    flags0: u32,
    capturing_group_count: u32,
    named: HashMap<String, u32>,
    captures: Vec<Option<String>>,
    /// Byte offset of each original code point (None after `\Q..\E` rewriting).
    byte_of: Option<Vec<usize>>,
    deferred: Option<Error>,
    depth: u32,
}

pub fn parse(pattern: &str) -> Result<Regex> {
    let mut temp: Vec<u32> = pattern.chars().map(|c| c as u32).collect();
    let mut byte_of: Option<Vec<usize>> = Some(pattern.char_indices().map(|(i, _)| i).collect());
    if remove_qe_quoting(&mut temp) {
        byte_of = None;
    }
    let pattern_length = temp.len();
    temp.extend([0, 0]);
    let mut p = Parser {
        src: pattern,
        temp,
        pattern_length,
        cursor: 0,
        flags0: 0,
        capturing_group_count: 1,
        named: HashMap::new(),
        captures: Vec::new(),
        byte_of,
        deferred: None,
        depth: 0,
    };
    let root = p.expr()?;
    if p.pattern_length != p.cursor {
        if p.peek() == ')' as u32 {
            return Err(p.error("Unmatched closing ')'"));
        }
        return Err(p.error("Unexpected internal error"));
    }
    let mut root = root;
    study_lookbehinds(&mut root)?;
    if let Some(e) = p.deferred {
        return Err(e);
    }
    Ok(Regex {
        flavor: Flavor::Java,
        pattern: pattern.to_string(),
        root,
        captures: p.captures,
        domain: Domain::Scalars,
        quirks: Quirks::default(),
    })
}

/// `TreeInfo` as Java computes it for a look-behind body: 32-bit wrapping arithmetic.
#[derive(Clone, Copy)]
struct Info {
    min: i32,
    max: i32,
    valid: bool,
    deterministic: bool,
}

impl Info {
    fn reset() -> Info {
        Info {
            min: 0,
            max: 0,
            valid: true,
            deterministic: true,
        }
    }
}

const MAX_REPS: i32 = i32::MAX;

/// The node chain Java builds, flattened (groups are transparent to `study`).
enum E {
    Len(i32),
    Ques(Vec<E>),
    Branch(Vec<Vec<E>>),
    /// `CharPropertyGreedy` (greedy `*`, `+`, `{n,}` on one char).
    Greedy(i32),
    Curly {
        atom: Vec<E>,
        cmin: i32,
        cmax: i32,
    },
    /// `Loop` (non-deterministic quantified group) and back-references: no maximum.
    Unbounded,
}

fn single_char(n: &Node) -> bool {
    matches!(n.kind, NodeKind::Literal(_) | NodeKind::Class(_))
}

fn flatten(n: &Node, out: &mut Vec<E>) {
    match &n.kind {
        NodeKind::Empty | NodeKind::Assert(_) | NodeKind::Look(_) => {}
        NodeKind::Literal(_) | NodeKind::Class(_) => out.push(E::Len(1)),
        NodeKind::Concat(xs) => xs.iter().for_each(|x| flatten(x, out)),
        NodeKind::Group(g) => flatten(&g.sub, out),
        NodeKind::Alt(xs) => out.push(E::Branch(
            xs.iter()
                .map(|x| {
                    let mut v = Vec::new();
                    flatten(x, &mut v);
                    v
                })
                .collect(),
        )),
        NodeKind::Backref(_) | NodeKind::Conditional(_) => out.push(E::Unbounded),
        NodeKind::Repeat(r) => {
            let mut atom = Vec::new();
            flatten(&r.sub, &mut atom);
            let group = matches!(r.sub.kind, NodeKind::Group(_));
            if r.min == 0 && r.max == Some(1) {
                // group0 turns a non-possessive `?` on a group into Branch(head, null).
                out.push(if group && !r.possessive {
                    E::Branch(vec![atom, Vec::new()])
                } else {
                    E::Ques(atom)
                });
            } else if r.greedy && !r.possessive && r.max.is_none() && single_char(&r.sub) {
                out.push(E::Greedy(r.min as i32));
            } else if group && !r.possessive && !chain_info(&atom).deterministic {
                out.push(E::Unbounded);
            } else {
                let cmax = r.max.map_or(MAX_REPS, |m| m.min(i32::MAX as u32) as i32);
                out.push(E::Curly {
                    atom,
                    cmin: r.min.min(i32::MAX as u32) as i32,
                    cmax,
                });
            }
        }
    }
}

fn chain_info(chain: &[E]) -> Info {
    let mut info = Info::reset();
    study_chain(chain, &mut info);
    info
}

/// `study()` along the chain, exactly as Pattern.java's nodes do it.
fn study_chain(chain: &[E], info: &mut Info) {
    for (idx, e) in chain.iter().enumerate() {
        match e {
            E::Len(k) => {
                info.min = info.min.wrapping_add(*k);
                info.max = info.max.wrapping_add(*k);
            }
            E::Ques(atom) => {
                let min_l = info.min;
                study_chain(atom, info);
                info.min = min_l;
                info.deterministic = false;
            }
            E::Greedy(cmin) => {
                info.min = info.min.wrapping_add(*cmin);
                if info.valid {
                    info.max = info.max.wrapping_add(MAX_REPS);
                }
                info.deterministic = false;
            }
            E::Curly { atom, cmin, cmax } => {
                let (min_l, max_l, max_v, detm) =
                    (info.min, info.max, info.valid, info.deterministic);
                *info = Info::reset();
                study_chain(atom, info);
                let mut t = info.min.wrapping_mul(*cmin).wrapping_add(min_l);
                if t < min_l {
                    t = 0xFFF_FFFF;
                }
                info.min = t;
                if max_v && info.valid {
                    let t = info.max.wrapping_mul(*cmax).wrapping_add(max_l);
                    info.max = t;
                    if t < max_l {
                        info.valid = false;
                    }
                } else {
                    info.valid = false;
                }
                info.deterministic = info.deterministic && cmin == cmax && detm;
            }
            E::Unbounded => {
                info.valid = false;
                info.deterministic = false;
            }
            E::Branch(alts) => {
                let (mut min_l, mut max_l, mut max_v) = (info.min, info.max, info.valid);
                let (mut min2, mut max2) = (i32::MAX, -1i32);
                for alt in alts {
                    let i = chain_info(alt);
                    min2 = min2.min(i.min);
                    max2 = max2.max(i.max);
                    max_v &= i.valid;
                }
                min_l = min_l.wrapping_add(min2);
                max_l = max_l.wrapping_add(max2);
                // The rest of the chain is studied from a fresh info, then the prefix is added.
                let rest = chain_info(&chain[idx + 1..]);
                info.min = rest.min.wrapping_add(min_l);
                info.max = rest.max.wrapping_add(max_l);
                info.valid = rest.valid && max_v;
                info.deterministic = false;
                return;
            }
        }
    }
}

/// Applies Java's compile-time look-behind check and records each look-behind's window.
fn study_lookbehinds(n: &mut Node) -> Result<()> {
    match &mut n.kind {
        NodeKind::Look(l) => {
            study_lookbehinds(&mut l.sub)?;
            if l.behind {
                let mut chain = Vec::new();
                flatten(&l.sub, &mut chain);
                let i = chain_info(&chain);
                if !i.valid {
                    return Err(Error::syntax(
                        n.span,
                        "Look-behind group does not have an obvious maximum length",
                    ));
                }
                if i.max < i.min {
                    l.window_empty = true;
                } else {
                    let max = if i.max >= 1_000_000 {
                        None
                    } else {
                        Some(i.max as u32)
                    };
                    l.window = Some((i.min.max(0) as u32, max));
                }
            }
        }
        NodeKind::Concat(xs) | NodeKind::Alt(xs) => {
            for x in xs {
                study_lookbehinds(x)?;
            }
        }
        NodeKind::Repeat(r) => study_lookbehinds(&mut r.sub)?,
        NodeKind::Group(g) => study_lookbehinds(&mut g.sub)?,
        _ => {}
    }
    Ok(())
}

/// `RemoveQEQuoting`: rewrites `\Q..\E` into escaped literals. Returns whether it changed.
fn remove_qe_quoting(temp: &mut Vec<u32>) -> bool {
    let p_len = temp.len();
    let at = |t: &[u32], i: usize| t.get(i).copied().unwrap_or(0);
    let mut i = 0;
    while i + 1 < p_len {
        if temp[i] != '\\' as u32 {
            i += 1;
        } else if temp[i + 1] != 'Q' as u32 {
            i += 2;
        } else {
            break;
        }
    }
    if i + 1 >= p_len {
        return false;
    }
    let mut out: Vec<u32> = temp[..i].to_vec();
    i += 2;
    let (mut in_quote, mut begin_quote) = (true, true);
    let is_alpha = |c: u32| (0x41..=0x5A).contains(&c) || (0x61..=0x7A).contains(&c);
    while i < p_len {
        let c = temp[i];
        i += 1;
        if c > 0x7F || is_alpha(c) {
            out.push(c);
        } else if (0x30..=0x39).contains(&c) {
            if begin_quote {
                out.extend(['\\' as u32, 'x' as u32, '3' as u32]);
            }
            out.push(c);
        } else if c != '\\' as u32 {
            if in_quote {
                out.push('\\' as u32);
            }
            out.push(c);
        } else if in_quote {
            if at(temp, i) == 'E' as u32 {
                i += 1;
                in_quote = false;
            } else {
                out.extend(['\\' as u32, '\\' as u32]);
            }
        } else if at(temp, i) == 'Q' as u32 {
            i += 1;
            in_quote = true;
            begin_quote = true;
            continue;
        } else {
            out.push(c);
            if i != p_len {
                out.push(temp[i]);
                i += 1;
            }
        }
        begin_quote = false;
    }
    *temp = out;
    true
}

fn is_space(c: u32) -> bool {
    matches!(c, 0x20 | 0x09 | 0x0A | 0x0B | 0x0C | 0x0D)
}

fn is_digit(c: u32) -> bool {
    (0x30..=0x39).contains(&c)
}

impl<'a> Parser<'a> {
    fn has(&self, f: u32) -> bool {
        self.flags0 & f != 0
    }

    fn at(&self, i: usize) -> u32 {
        self.temp.get(i).copied().unwrap_or(0)
    }

    fn span_at(&self, idx: usize) -> Span {
        match &self.byte_of {
            Some(b) => {
                let start = b.get(idx).copied().unwrap_or(self.src.len());
                let end = b.get(idx + 1).copied().unwrap_or(self.src.len());
                Span::new(start, end.max(start))
            }
            None => Span::new(0, self.src.len()),
        }
    }

    fn error(&self, msg: &str) -> Error {
        Error::syntax(self.span_at(self.cursor.saturating_sub(1)), msg.to_string())
    }

    fn span_from(&self, start_idx: usize) -> Span {
        match &self.byte_of {
            Some(b) => {
                let s = b.get(start_idx).copied().unwrap_or(self.src.len());
                let e = b.get(self.cursor).copied().unwrap_or(self.src.len());
                Span::new(s, e.max(s))
            }
            None => Span::new(0, self.src.len()),
        }
    }

    fn defer(&mut self, what: &str) {
        if self.deferred.is_none() {
            self.deferred = Some(Error::unsupported(
                self.span_at(self.cursor.saturating_sub(1)),
                what.to_string(),
            ));
        }
    }

    // ------------------------------------------------------------- cursor primitives

    fn is_line_separator(&self, ch: u32) -> bool {
        if self.has(UNIX_LINES) {
            ch == 0x0A
        } else {
            ch == 0x0A || ch == 0x0D || (ch | 1) == 0x2029 || ch == 0x85
        }
    }

    fn peek_past_line(&mut self) -> u32 {
        self.cursor += 1;
        let mut ch = self.at(self.cursor);
        while ch != 0 && !self.is_line_separator(ch) {
            self.cursor += 1;
            ch = self.at(self.cursor);
        }
        if ch == 0 && self.cursor > self.pattern_length {
            self.cursor = self.pattern_length;
            ch = self.at(self.cursor);
        }
        ch
    }

    fn parse_past_line(&mut self) -> u32 {
        let mut ch = self.at(self.cursor);
        self.cursor += 1;
        while ch != 0 && !self.is_line_separator(ch) {
            ch = self.at(self.cursor);
            self.cursor += 1;
        }
        if ch == 0 && self.cursor > self.pattern_length {
            self.cursor = self.pattern_length;
            ch = self.at(self.cursor);
            self.cursor += 1;
        }
        ch
    }

    fn peek_past_whitespace(&mut self, mut ch: u32) -> u32 {
        while is_space(ch) || ch == '#' as u32 {
            while is_space(ch) {
                self.cursor += 1;
                ch = self.at(self.cursor);
            }
            if ch == '#' as u32 {
                ch = self.peek_past_line();
            }
        }
        ch
    }

    fn parse_past_whitespace(&mut self, mut ch: u32) -> u32 {
        while is_space(ch) || ch == '#' as u32 {
            while is_space(ch) {
                ch = self.at(self.cursor);
                self.cursor += 1;
            }
            if ch == '#' as u32 {
                ch = self.parse_past_line();
            }
        }
        ch
    }

    fn peek(&mut self) -> u32 {
        let ch = self.at(self.cursor);
        if self.has(COMMENTS) {
            self.peek_past_whitespace(ch)
        } else {
            ch
        }
    }

    fn read(&mut self) -> u32 {
        let ch = self.at(self.cursor);
        self.cursor += 1;
        if self.has(COMMENTS) {
            self.parse_past_whitespace(ch)
        } else {
            ch
        }
    }

    fn next(&mut self) -> u32 {
        self.cursor += 1;
        let ch = self.at(self.cursor);
        if self.has(COMMENTS) {
            self.peek_past_whitespace(ch)
        } else {
            ch
        }
    }

    fn next_escaped(&mut self) -> u32 {
        self.cursor += 1;
        self.at(self.cursor)
    }

    fn skip(&mut self) -> u32 {
        let ch = self.at(self.cursor + 1);
        self.cursor += 2;
        ch
    }

    fn unread(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn accept(&mut self, ch: u32, msg: &str) -> Result<()> {
        let mut test = self.at(self.cursor);
        self.cursor += 1;
        if self.has(COMMENTS) {
            test = self.parse_past_whitespace(test);
        }
        if ch != test {
            return Err(self.error(msg));
        }
        Ok(())
    }

    fn enter(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > NEST_LIMIT {
            return Err(Error::unsupported(
                self.span_at(self.cursor),
                "nesting too deep to model Java's stack limit",
            ));
        }
        Ok(())
    }

    // ------------------------------------------------------------- predicates

    fn dot(&self) -> CharSet {
        if self.has(DOTALL) {
            CharSet::full()
        } else if self.has(UNIX_LINES) {
            CharSet::full().difference(&CharSet::single('\n'))
        } else {
            CharSet::full().difference(&CharSet::from_iter([
                '\n', '\r', '\u{85}', '\u{2028}', '\u{2029}',
            ]))
        }
    }

    /// `single(ch)`.
    fn single(&self, ch: u32) -> CharSet {
        if self.has(CASE_INSENSITIVE) {
            if self.has(UNICODE_CASE) {
                let up = upper(ch);
                let lo = lower(up);
                if up != lo {
                    return single_u(lo);
                }
            } else if ch < 0x80 {
                let (lo, up) = (ascii_lower(ch), ascii_upper(ch));
                if lo != up {
                    return points([lo, up]);
                }
            }
        }
        points([ch])
    }

    /// One char of a slice (`SliceI` / `SliceU`).
    fn slice_char(&self, ch: u32) -> CharSet {
        if !self.has(CASE_INSENSITIVE) {
            return points([ch]);
        }
        if self.has(UNICODE_CASE) {
            return single_u(key(ch));
        }
        let b = ascii_lower(ch);
        // c == b || ASCII.toLower(c) == b
        if (0x61..=0x7A).contains(&b) {
            points([b, b - 0x20])
        } else {
            points([b])
        }
    }

    /// `bitsOrSingle` (the bit-class path, folded per `BitClass.add`).
    fn bits_or_single(&self, ch: u32) -> CharSet {
        let ci = self.has(CASE_INSENSITIVE);
        let iu = ci && self.has(UNICODE_CASE);
        let exception = iu
            && matches!(
                ch,
                0xFF | 0xB5 | 0x49 | 0x69 | 0x53 | 0x73 | 0x4B | 0x6B | 0xC5 | 0xE5
            );
        if ch < 256 && !exception {
            let mut v = vec![ch];
            if ci {
                if ch < 0x80 {
                    v.extend([ascii_upper(ch), ascii_lower(ch)]);
                } else if iu {
                    v.extend([lower(ch), upper(ch)]);
                }
            }
            return points(v.into_iter().filter(|&c| c < 256));
        }
        self.single(ch)
    }

    /// `Range` / `CIRange` / `CIRangeU`.
    fn range_set(&self, lo: u32, hi: u32) -> CharSet {
        let s = CharSet::from_ranges([(lo, hi)]);
        if !self.has(CASE_INSENSITIVE) {
            return s;
        }
        let in_s = |c: u32| lo <= c && c <= hi;
        let extra: Vec<u32> = if self.has(UNICODE_CASE) {
            cased()
                .iter()
                .copied()
                .filter(|&c| in_s(upper(c)) || in_s(key(c)))
                .collect()
        } else {
            (0..0x80)
                .filter(|&c| in_s(ascii_upper(c)) || in_s(ascii_lower(c)))
                .collect()
        };
        s.union(&points(extra))
    }

    fn word(&self) -> CharSet {
        crate::unicode::posix_class("word").unwrap_or_default()
    }

    // ------------------------------------------------------------- structure

    fn expr(&mut self) -> Result<Node> {
        let start = self.cursor;
        let mut alts = Vec::new();
        loop {
            alts.push(self.sequence()?);
            if self.peek() != '|' as u32 {
                break;
            }
            self.next();
        }
        Ok(if alts.len() == 1 {
            alts.pop()
                .unwrap_or(Node::new(NodeKind::Empty, self.span_from(start)))
        } else {
            Node::new(NodeKind::Alt(alts), self.span_from(start))
        })
    }

    fn sequence(&mut self) -> Result<Node> {
        let start = self.cursor;
        let mut items: Vec<Node> = Vec::new();
        loop {
            let ch = self.peek();
            let at = self.cursor;
            let node = match ch {
                0x28 => {
                    // '(' — group0 applies its own closure.
                    if let Some(n) = self.group0()? {
                        items.push(n);
                    }
                    continue;
                }
                0x5B => {
                    let set = self.clazz(true)?;
                    Node::new(NodeKind::Class(set), self.span_from(at))
                }
                0x5C => {
                    let c = self.next_escaped();
                    if c == 'p' as u32 || c == 'P' as u32 {
                        let set = self.family_after_p(c == 'P' as u32)?;
                        Node::new(NodeKind::Class(set), self.span_from(at))
                    } else {
                        self.unread();
                        self.atom()?
                    }
                }
                0x5E => {
                    self.next();
                    let a = if self.has(MULTILINE) {
                        Assertion::JavaLineStart {
                            unix: self.has(UNIX_LINES),
                        }
                    } else {
                        Assertion::TextStart
                    };
                    Node::new(NodeKind::Assert(a), self.span_from(at))
                }
                0x24 => {
                    self.next();
                    let unix = self.has(UNIX_LINES);
                    let a = if self.has(MULTILINE) {
                        Assertion::JavaLineEnd { unix }
                    } else {
                        Assertion::JavaDollar { unix }
                    };
                    Node::new(NodeKind::Assert(a), self.span_from(at))
                }
                0x2E => {
                    self.next();
                    Node::new(NodeKind::Class(self.dot()), self.span_from(at))
                }
                0x7C | 0x29 => break,
                0x3F | 0x2A | 0x2B => {
                    self.next();
                    return Err(self.error(&format!(
                        "Dangling meta character '{}'",
                        char::from_u32(ch).unwrap_or('?')
                    )));
                }
                0 if self.cursor >= self.pattern_length => break,
                _ => self.atom()?,
            };
            let node = self.closure(node, at)?;
            items.push(node);
        }
        let span = self.span_from(start);
        Ok(match items.len() {
            0 => Node::new(NodeKind::Empty, span),
            1 => items.pop().unwrap_or(Node::new(NodeKind::Empty, span)),
            _ => Node::new(NodeKind::Concat(items), span),
        })
    }

    /// `atom()`: a slice of literal chars, or one escape node.
    fn atom(&mut self) -> Result<Node> {
        let start = self.cursor;
        let mut buf: Vec<u32> = Vec::new();
        let mut prev: usize = 0;
        let mut ch = self.peek();
        loop {
            match ch {
                0x2A | 0x2B | 0x3F | 0x7B => {
                    if buf.len() > 1 {
                        self.cursor = prev;
                        buf.pop();
                    }
                    break;
                }
                0x24 | 0x2E | 0x5E | 0x28 | 0x5B | 0x7C | 0x29 => break,
                0x5C => {
                    let c = self.next_escaped();
                    if c == 'p' as u32 || c == 'P' as u32 {
                        if !buf.is_empty() {
                            self.unread();
                            break;
                        }
                        let set = self.family_after_p(c == 'P' as u32)?;
                        return Ok(Node::new(NodeKind::Class(set), self.span_from(start)));
                    }
                    self.unread();
                    prev = self.cursor;
                    match self.escape(false, buf.is_empty(), false)? {
                        Esc::Char(v) => {
                            buf.push(v);
                            ch = self.peek();
                            continue;
                        }
                        Esc::Pred(set) => {
                            return Ok(Node::new(NodeKind::Class(set), self.span_from(start)));
                        }
                        Esc::Node(n) => return Ok(n),
                        Esc::None => {
                            self.cursor = prev;
                            break;
                        }
                    }
                }
                0 if self.cursor >= self.pattern_length => break,
                _ => {
                    prev = self.cursor;
                    buf.push(ch);
                    ch = self.next();
                    continue;
                }
            }
        }
        let span = self.span_from(start);
        if buf.len() == 1 {
            return Ok(self.set_node(self.single(buf[0]), buf[0], span));
        }
        let nodes: Vec<Node> = buf
            .iter()
            .map(|&c| self.set_node(self.slice_char(c), c, span))
            .collect();
        Ok(match nodes.len() {
            0 => Node::new(NodeKind::Empty, span),
            _ => Node::new(NodeKind::Concat(nodes), span),
        })
    }

    /// A literal when the set is exactly that char, else a class.
    fn set_node(&self, set: CharSet, c: u32, span: Span) -> Node {
        match char::from_u32(c) {
            Some(ch) if set == CharSet::single(ch) => Node::new(NodeKind::Literal(ch), span),
            _ => Node::new(NodeKind::Class(set), span),
        }
    }

    /// `closure(prev)`.
    fn closure(&mut self, prev: Node, start: usize) -> Result<Node> {
        let ch = self.peek();
        let (min, max) = match ch {
            0x3F => (0, Some(1)),
            0x2A => (0, None),
            0x2B => (1, None),
            0x7B => {
                let mut ch = self.skip();
                if !is_digit(ch) {
                    return Err(self.error("Illegal repetition"));
                }
                let overflow = |p: &Self| p.error("Illegal repetition range");
                let mut cmin: i32 = 0;
                loop {
                    cmin = cmin
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((ch - 0x30) as i32))
                        .ok_or_else(|| overflow(self))?;
                    ch = self.read();
                    if !is_digit(ch) {
                        break;
                    }
                }
                let cmax: Option<i32> = if ch == ',' as u32 {
                    ch = self.read();
                    if ch == '}' as u32 {
                        self.unread();
                        let (greedy, possessive) = self.qtype();
                        return Ok(self.repeat(prev, cmin as u32, None, greedy, possessive, start));
                    }
                    let mut m: i32 = 0;
                    while is_digit(ch) {
                        m = m
                            .checked_mul(10)
                            .and_then(|v| v.checked_add((ch - 0x30) as i32))
                            .ok_or_else(|| overflow(self))?;
                        ch = self.read();
                    }
                    Some(m)
                } else {
                    Some(cmin)
                };
                if ch != '}' as u32 {
                    return Err(self.error("Unclosed counted closure"));
                }
                if cmax.is_some_and(|m| m < cmin) {
                    return Err(self.error("Illegal repetition range"));
                }
                self.unread();
                let (greedy, possessive) = self.qtype();
                return Ok(self.repeat(
                    prev,
                    cmin as u32,
                    cmax.map(|m| m as u32),
                    greedy,
                    possessive,
                    start,
                ));
            }
            _ => return Ok(prev),
        };
        let (greedy, possessive) = self.qtype();
        Ok(self.repeat(prev, min, max, greedy, possessive, start))
    }

    /// `qtype()`: `(greedy, possessive)`.
    fn qtype(&mut self) -> (bool, bool) {
        let ch = self.next();
        if ch == '?' as u32 {
            self.next();
            (false, false)
        } else if ch == '+' as u32 {
            self.next();
            (true, true)
        } else {
            (true, false)
        }
    }

    fn repeat(
        &self,
        sub: Node,
        min: u32,
        max: Option<u32>,
        greedy: bool,
        possessive: bool,
        start: usize,
    ) -> Node {
        Node::new(
            NodeKind::Repeat(Repeat {
                sub: Box::new(sub),
                min,
                max,
                greedy,
                possessive,
            }),
            self.span_from(start),
        )
    }

    /// `ref(refNum)`: greedy multi-digit backreference.
    fn backref(&mut self, mut n: u32) -> Node {
        let start = self.cursor;
        loop {
            let ch = self.peek();
            if !is_digit(ch) {
                break;
            }
            let m = n.saturating_mul(10).saturating_add(ch - 0x30);
            if self.capturing_group_count - 1 < m {
                break;
            }
            n = m;
            self.read();
        }
        Node::new(NodeKind::Backref(n), self.span_from(start))
    }

    /// `escape(inclass, create, isrange)`.
    fn escape(&mut self, inclass: bool, create: bool, isrange: bool) -> Result<Esc> {
        let start = self.cursor;
        let ch = self.skip();
        let c = char::from_u32(ch).unwrap_or('\0');
        let node = |p: &Self, a: Assertion| Node::new(NodeKind::Assert(a), p.span_from(start));
        let pred = |p: &Self, s: CharSet, create: bool| {
            let _ = p;
            if create { Esc::Pred(s) } else { Esc::None }
        };
        let unicode_cc = self.has(UNICODE_CHARACTER_CLASS);
        let bad = |p: &Self| Err(p.error("Illegal/unsupported escape sequence"));
        Ok(match c {
            '0' => Esc::Char(self.octal()?),
            '1'..='9' => {
                if inclass {
                    return bad(self);
                }
                if create {
                    Esc::Node(self.backref(ch - 0x30))
                } else {
                    Esc::None
                }
            }
            'A' if !inclass => {
                if create {
                    Esc::Node(node(self, Assertion::TextStart))
                } else {
                    Esc::None
                }
            }
            'B' if !inclass => {
                if unicode_cc {
                    self.defer("(?U) Unicode character classes are not analysed yet");
                }
                if create {
                    Esc::Node(node(
                        self,
                        Assertion::NotWordBoundary {
                            word: WordSet::Java,
                        },
                    ))
                } else {
                    Esc::None
                }
            }
            'G' if !inclass => {
                // \G is the end of the previous match: position 0 for matches() and a first find().
                if create {
                    Esc::Node(node(self, Assertion::TextStart))
                } else {
                    Esc::None
                }
            }
            'Z' if !inclass => {
                if create {
                    Esc::Node(node(
                        self,
                        Assertion::JavaDollar {
                            unix: self.has(UNIX_LINES),
                        },
                    ))
                } else {
                    Esc::None
                }
            }
            'z' if !inclass => {
                if create {
                    Esc::Node(node(self, Assertion::TextEnd))
                } else {
                    Esc::None
                }
            }
            'R' if !inclass => {
                if create {
                    let span = self.span_from(start);
                    let lt = CharSet::from_iter([
                        '\n', '\x0B', '\x0C', '\r', '\u{85}', '\u{2028}', '\u{2029}',
                    ]);
                    Esc::Node(Node::new(
                        NodeKind::Alt(vec![
                            Node::new(
                                NodeKind::Concat(vec![
                                    Node::new(NodeKind::Literal('\r'), span),
                                    Node::new(NodeKind::Literal('\n'), span),
                                ]),
                                span,
                            ),
                            Node::new(NodeKind::Class(lt), span),
                        ]),
                        span,
                    ))
                } else {
                    Esc::None
                }
            }
            'X' if !inclass => {
                self.defer("\\X (grapheme clusters) is not analysed yet");
                if create {
                    Esc::Node(Node::new(
                        NodeKind::Class(CharSet::empty()),
                        self.span_from(start),
                    ))
                } else {
                    Esc::None
                }
            }
            'b' if !inclass => {
                if create {
                    if self.peek() == '{' as u32 {
                        if self.skip() == 'g' as u32 {
                            if self.read() == '}' as u32 {
                                self.defer("\\b{g} (grapheme boundaries) is not analysed yet");
                                return Ok(Esc::Node(node(self, Assertion::TextStart)));
                            }
                            return bad(self);
                        }
                        self.unread();
                        self.unread();
                    }
                    if unicode_cc {
                        self.defer("(?U) Unicode character classes are not analysed yet");
                    }
                    Esc::Node(node(
                        self,
                        Assertion::WordBoundary {
                            word: WordSet::Java,
                        },
                    ))
                } else {
                    Esc::None
                }
            }
            'k' if !inclass => {
                if self.read() != '<' as u32 {
                    return Err(self.error("\\k is not followed by '<' for named capturing group"));
                }
                let first = self.read();
                let name = self.group_name(first)?;
                let Some(&n) = self.named.get(&name) else {
                    return Err(
                        self.error(&format!("named capturing group <{name}> does not exist"))
                    );
                };
                if create {
                    Esc::Node(Node::new(NodeKind::Backref(n), self.span_from(start)))
                } else {
                    Esc::None
                }
            }
            'D' | 'd' | 'S' | 's' | 'W' | 'w' | 'H' | 'h' | 'V' | 'v' => {
                if c == 'v' && isrange {
                    return Ok(Esc::Char(0x0B));
                }
                if unicode_cc && "DdSsWw".contains(c) {
                    self.defer("(?U) Unicode character classes are not analysed yet");
                }
                let base = match c.to_ascii_lowercase() {
                    'd' => CharSet::range('0', '9'),
                    's' => CharSet::from_iter([' ', '\t', '\n', '\x0B', '\x0C', '\r']),
                    'w' => self.word(),
                    'h' => CharSet::from_iter([
                        '\t', ' ', '\u{A0}', '\u{1680}', '\u{180E}', '\u{202F}', '\u{205F}',
                        '\u{3000}',
                    ])
                    .union(&CharSet::range('\u{2000}', '\u{200A}')),
                    _ => CharSet::range('\n', '\r')
                        .union(&CharSet::from_iter(['\u{85}', '\u{2028}', '\u{2029}'])),
                };
                let set = if c.is_ascii_uppercase() {
                    base.complement()
                } else {
                    base
                };
                pred(self, set, create)
            }
            'N' => {
                // \N{name}: needs the Unicode name database.
                if self.read() != '{' as u32 {
                    return Err(self.error("Illegal character name escape sequence"));
                }
                loop {
                    let r = self.read();
                    if r == '}' as u32 {
                        break;
                    }
                    if self.cursor >= self.pattern_length {
                        return Err(self.error("Unclosed character name escape sequence"));
                    }
                }
                self.defer("\\N{name} needs the Unicode name database (not bundled yet)");
                Esc::Char(0xD800)
            }
            'a' => Esc::Char(0x07),
            'c' => {
                if self.cursor < self.pattern_length {
                    Esc::Char(self.read() ^ 64)
                } else {
                    return Err(self.error("Illegal control escape sequence"));
                }
            }
            'e' => Esc::Char(0x1B),
            'f' => Esc::Char(0x0C),
            'n' => Esc::Char(0x0A),
            'r' => Esc::Char(0x0D),
            't' => Esc::Char(0x09),
            'u' => Esc::Char(self.unicode_escape()?),
            'x' => Esc::Char(self.hex()?),
            c if c.is_ascii_alphabetic() => return bad(self),
            _ if inclass && is_digit(ch) => return bad(self),
            _ => Esc::Char(ch),
        })
    }

    fn octal(&mut self) -> Result<u32> {
        let o = |c: u32| (0x30..=0x37).contains(&c);
        let n = self.read();
        if o(n) {
            let m = self.read();
            if o(m) {
                let x = self.read();
                if o(x) && (0x30..=0x33).contains(&n) {
                    return Ok((n - 0x30) * 64 + (m - 0x30) * 8 + (x - 0x30));
                }
                self.unread();
                return Ok((n - 0x30) * 8 + (m - 0x30));
            }
            self.unread();
            return Ok(n - 0x30);
        }
        Err(self.error("Illegal octal escape sequence"))
    }

    fn hex(&mut self) -> Result<u32> {
        let hd = |c: u32| char::from_u32(c).and_then(|c| c.to_digit(16));
        let n = self.read();
        if let Some(a) = hd(n) {
            let m = self.read();
            if let Some(b) = hd(m) {
                return Ok(a * 16 + b);
            }
        } else if n == '{' as u32 && hd(self.peek()).is_some() {
            let mut ch: u32 = 0;
            loop {
                let d = self.read();
                match hd(d) {
                    Some(v) => {
                        ch = (ch << 4) + v;
                        if ch > 0x10_FFFF {
                            return Err(self.error("Hexadecimal codepoint is too big"));
                        }
                    }
                    None => {
                        if d != '}' as u32 {
                            return Err(self.error("Unclosed hexadecimal escape sequence"));
                        }
                        return Ok(ch);
                    }
                }
            }
        }
        Err(self.error("Illegal hexadecimal escape sequence"))
    }

    fn uxxxx(&mut self) -> Result<u32> {
        let mut n = 0;
        for _ in 0..4 {
            let ch = self.read();
            let d = char::from_u32(ch)
                .and_then(|c| c.to_digit(16))
                .ok_or_else(|| self.error("Illegal Unicode escape sequence"))?;
            n = n * 16 + d;
        }
        Ok(n)
    }

    fn unicode_escape(&mut self) -> Result<u32> {
        let n = self.uxxxx()?;
        if (0xD800..0xDC00).contains(&n) {
            let cur = self.cursor;
            if self.read() == '\\' as u32 && self.read() == 'u' as u32 {
                let n2 = self.uxxxx()?;
                if (0xDC00..0xE000).contains(&n2) {
                    return Ok(0x10000 + ((n - 0xD800) << 10) + (n2 - 0xDC00));
                }
            }
            self.cursor = cur;
        }
        Ok(n)
    }

    /// At `p`/`P` (cursor on that letter): `family()` — not analysed, but syntax-checked.
    fn family_after_p(&mut self, _complement: bool) -> Result<CharSet> {
        let ch = self.next();
        let single = ch != '{' as u32;
        if single {
            self.unread();
        }
        self.next();
        if single {
            self.read();
        } else {
            let i = self.cursor;
            loop {
                let r = self.read();
                if r == '}' as u32 {
                    break;
                }
                if self.cursor > self.pattern_length {
                    return Err(self.error("Unclosed character family"));
                }
            }
            if i + 1 >= self.cursor {
                return Err(self.error("Empty character family"));
            }
        }
        self.defer("\\p{..} Unicode properties need Java's property tables (not bundled yet)");
        Ok(CharSet::empty())
    }

    /// `groupname(ch)`: `[A-Za-z][A-Za-z0-9]*` then `>`.
    fn group_name(&mut self, mut ch: u32) -> Result<String> {
        let alpha = |c: u32| (0x41..=0x5A).contains(&c) || (0x61..=0x7A).contains(&c);
        if !alpha(ch) {
            return Err(self.error("capturing group name does not start with a Latin letter"));
        }
        let mut s = String::new();
        loop {
            s.push(char::from_u32(ch).unwrap_or('?'));
            ch = self.read();
            if !(alpha(ch) || is_digit(ch)) {
                break;
            }
        }
        if ch != '>' as u32 {
            return Err(self.error("named capturing group is missing trailing '>'"));
        }
        Ok(s)
    }

    // ------------------------------------------------------------- classes

    /// `clazz(consume)`, cursor on the `[` (or, for `consume == false`, before the first item).
    fn clazz(&mut self, consume: bool) -> Result<CharSet> {
        self.enter()?;
        let r = self.clazz_inner(consume);
        self.depth -= 1;
        r
    }

    fn clazz_inner(&mut self, consume: bool) -> Result<CharSet> {
        let mut prev: Option<CharSet> = None;
        // Like the JDK, `curr` persists across iterations: the last range / nested class, or
        // `None` after a bit-class item.
        let mut curr: Option<CharSet> = None;
        let mut bits = CharSet::empty();
        let mut has_bits = false;
        let mut is_neg = false;
        let mut ch = self.next();
        if ch == '^' as u32 && self.at(self.cursor - 1) == '[' as u32 {
            ch = self.next();
            is_neg = true;
        }
        let union = |a: Option<CharSet>, b: &CharSet| Some(a.map_or(b.clone(), |a| a.union(b)));
        loop {
            match ch {
                0x5B => {
                    let c = self.clazz(true)?;
                    prev = union(prev, &c);
                    curr = Some(c);
                    ch = self.peek();
                    continue;
                }
                0x26 => {
                    ch = self.next();
                    if ch == '&' as u32 {
                        ch = self.next();
                        let mut right: Option<CharSet> = None;
                        while ch != ']' as u32 && ch != '&' as u32 {
                            if ch == '[' as u32 {
                                let c = self.clazz(true)?;
                                right = union(right, &c);
                            } else {
                                self.unread();
                                let c = self.clazz(false)?;
                                right = union(right, &c);
                            }
                            ch = self.peek();
                        }
                        if has_bits {
                            match &prev {
                                None => {
                                    prev = Some(bits.clone());
                                    curr = Some(bits.clone());
                                }
                                Some(_) => prev = union(prev, &bits),
                            }
                            has_bits = false;
                        }
                        if right.is_some() {
                            curr = right.clone();
                        }
                        match prev {
                            None => match right {
                                None => return Err(self.error("Bad class syntax")),
                                Some(r) => prev = Some(r),
                            },
                            Some(p) => {
                                let Some(c) = curr.as_ref() else {
                                    return Err(self.error("Bad intersection syntax"));
                                };
                                prev = Some(p.intersect(c));
                            }
                        }
                        continue;
                    }
                    // A literal '&'.
                    self.unread();
                }
                0 if self.cursor >= self.pattern_length => {
                    return Err(self.error("Unclosed character class"));
                }
                0x5D if prev.is_some() || has_bits => {
                    if consume {
                        self.next();
                    }
                    let mut result = match prev {
                        None => bits,
                        Some(p) if has_bits => p.union(&bits),
                        Some(p) => p,
                    };
                    if is_neg {
                        result = result.complement();
                    }
                    return Ok(result);
                }
                _ => {}
            }
            let (set, is_bits) = self.range()?;
            if is_bits {
                bits = bits.union(&set);
                has_bits = true;
                curr = None;
            } else {
                prev = union(prev, &set);
                curr = Some(set);
            }
            ch = self.peek();
        }
    }

    /// `range(bits)`: `(set, went_to_bits)`.
    fn range(&mut self) -> Result<(CharSet, bool)> {
        let mut ch = self.peek();
        if ch == '\\' as u32 {
            ch = self.next_escaped();
            if ch == 'p' as u32 || ch == 'P' as u32 {
                return Ok((self.family_after_p(ch == 'P' as u32)?, false));
            }
            let isrange = self.at(self.cursor + 1) == '-' as u32;
            self.unread();
            match self.escape(true, true, isrange)? {
                Esc::Char(v) => ch = v,
                Esc::Pred(s) => return Ok((s, false)),
                Esc::Node(_) | Esc::None => {
                    return Err(self.error("Illegal/unsupported escape sequence"));
                }
            }
        } else {
            self.next();
        }
        if self.peek() == '-' as u32 {
            let end_range = self.at(self.cursor + 1);
            if end_range == '[' as u32 {
                return Ok(self.bits_item(ch));
            }
            if end_range != ']' as u32 {
                self.next();
                let mut m = self.peek();
                if m == '\\' as u32 {
                    m = match self.escape(true, false, true)? {
                        Esc::Char(v) => v,
                        _ => u32::MAX,
                    };
                } else {
                    self.next();
                }
                if m == u32::MAX || m < ch {
                    return Err(self.error("Illegal character range"));
                }
                return Ok((self.range_set(ch, m), false));
            }
        }
        Ok(self.bits_item(ch))
    }

    fn bits_item(&self, ch: u32) -> (CharSet, bool) {
        let set = self.bits_or_single(ch);
        let ci_iu = self.has(CASE_INSENSITIVE) && self.has(UNICODE_CASE);
        let exception = ci_iu
            && matches!(
                ch,
                0xFF | 0xB5 | 0x49 | 0x69 | 0x53 | 0x73 | 0x4B | 0x6B | 0xC5 | 0xE5
            );
        (set, ch < 256 && !exception)
    }

    // ------------------------------------------------------------- groups

    /// `group0()`, cursor on `(`. `None` for a pure flag group.
    fn group0(&mut self) -> Result<Option<Node>> {
        let start = self.cursor;
        let save = self.flags0;
        let mut capture = None;
        let mut atomic = false;
        let mut look: Option<(bool, bool)> = None;
        let ch = self.next();
        if ch == '?' as u32 {
            let ch = self.skip();
            match char::from_u32(ch).unwrap_or('\0') {
                ':' => {}
                '=' => look = Some((false, false)),
                '!' => look = Some((false, true)),
                '>' => atomic = true,
                '<' => {
                    let c = self.read();
                    if c == '=' as u32 {
                        look = Some((true, false));
                    } else if c == '!' as u32 {
                        look = Some((true, true));
                    } else {
                        let name = self.group_name(c)?;
                        if self.named.contains_key(&name) {
                            return Err(self.error(&format!(
                                "Named capturing group <{name}> is already defined"
                            )));
                        }
                        let idx = self.capturing_group_count;
                        self.capturing_group_count += 1;
                        self.named.insert(name.clone(), idx);
                        self.captures.push(Some(name.clone()));
                        capture = Some(Capture {
                            index: idx,
                            name: Some(name),
                        });
                    }
                }
                '$' | '@' => return Err(self.error("Unknown group type")),
                _ => {
                    self.unread();
                    self.add_flag();
                    let c = self.read();
                    if c == ')' as u32 {
                        return Ok(None);
                    }
                    if c != ':' as u32 {
                        return Err(self.error("Unknown inline modifier"));
                    }
                }
            }
        } else {
            let idx = self.capturing_group_count;
            self.capturing_group_count += 1;
            self.captures.push(None);
            capture = Some(Capture {
                index: idx,
                name: None,
            });
        }
        self.enter()?;
        let sub = self.expr();
        self.depth -= 1;
        let sub = sub?;
        self.accept(')' as u32, "Unclosed group")?;
        self.flags0 = save;
        let span = self.span_from(start);
        let node = match look {
            Some((behind, negated)) => Node::new(
                NodeKind::Look(Look {
                    sub: Box::new(sub),
                    behind,
                    negated,
                    window: None,
                    window_empty: false,
                }),
                span,
            ),
            None => Node::new(
                NodeKind::Group(Group {
                    sub: Box::new(sub),
                    capture,
                    atomic,
                }),
                span,
            ),
        };
        Ok(Some(self.closure(node, start)?))
    }

    /// `addFlag()` / `subFlag()`.
    fn add_flag(&mut self) {
        let mut ch = self.peek();
        loop {
            let f = match char::from_u32(ch).unwrap_or('\0') {
                'i' => CASE_INSENSITIVE,
                'm' => MULTILINE,
                's' => DOTALL,
                'd' => UNIX_LINES,
                'u' => UNICODE_CASE,
                'c' => CANON_EQ,
                'x' => COMMENTS,
                'U' => UNICODE_CHARACTER_CLASS | UNICODE_CASE,
                '-' => {
                    self.next();
                    self.sub_flag();
                    return;
                }
                _ => return,
            };
            self.flags0 |= f;
            self.note_flag(f);
            ch = self.next();
        }
    }

    fn sub_flag(&mut self) {
        let mut ch = self.peek();
        loop {
            let f = match char::from_u32(ch).unwrap_or('\0') {
                'i' => CASE_INSENSITIVE,
                'm' => MULTILINE,
                's' => DOTALL,
                'd' => UNIX_LINES,
                'u' => UNICODE_CASE,
                'c' => CANON_EQ,
                'x' => COMMENTS,
                'U' => UNICODE_CHARACTER_CLASS | UNICODE_CASE,
                _ => return,
            };
            self.flags0 &= !f;
            ch = self.next();
        }
    }

    fn note_flag(&mut self, f: u32) {
        if f & CANON_EQ != 0 {
            self.defer("(?c) canonical equivalence is not analysed yet");
        }
        if f & UNICODE_CHARACTER_CLASS != 0 {
            self.defer("(?U) Unicode character classes are not analysed yet");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    fn ok(p: &str) -> String {
        match parse(p) {
            Ok(r) => r.root.to_string(),
            Err(e) => panic!("{p:?} should parse: {}", e.render(p)),
        }
    }

    fn syntax(p: &str) {
        match parse(p) {
            Err(e) if e.kind == ErrorKind::Syntax => {}
            other => panic!("{p:?} should be a syntax error, got {other:?}"),
        }
    }

    #[test]
    fn java_rules() {
        assert_eq!(ok("{2}"), "(rep{2,2} ())");
        assert_eq!(ok("a{2}{3}"), "(cat (rep{2,2} 'a') (rep{3,3} ()))");
        assert_eq!(ok("ab*"), "(cat 'a' (rep{0,} 'b'))");
        assert_eq!(ok("]}"), "(cat ']' '}')");
        assert_eq!(ok(r"\Q*+\E"), "(cat '*' '+')");
        assert_eq!(ok("a$"), "(cat 'a' JavaDollar { unix: false })");
        for p in [
            "a{",
            "x{a}",
            "a{2}*",
            "a**",
            "a*??",
            "a(?i)*",
            "[]",
            "[a-\\d]",
            r"\0",
            r"\c",
            r"\E",
            "(?<a_b>x)",
            "(?<n>a)(?<n>b)",
            r"\k<n>(?<n>a)",
            "(?#c)",
            "(?P<n>a)",
            ")",
            "(?:",
            r"\x4",
            r"[\b]",
        ] {
            syntax(p);
        }
        for p in [
            "(?)",
            "(?-)",
            "(?i-)",
            r"\8",
            r"\1(a)",
            "[]a]",
            "[&&a]",
            "[a&&]",
            r"(?x)[a b]",
        ] {
            assert!(parse(p).is_ok(), "{p}");
        }
    }
}
