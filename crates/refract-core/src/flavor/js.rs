//! Front-end for JavaScript (ECMAScript 2025, V8). Spec: `spec/flavours/javascript.md`.
//!
//! One recursive-descent parser for both grammars: the Annex B web-compatibility grammar (no
//! `u` flag) and the strict Unicode grammar (`u`). Without `u` the engine matches UTF-16 code
//! units, so an astral character in the pattern is two atoms (its surrogate halves); the
//! analysis domain is then BMP haystacks (ADR-0007), where those halves never match.

use crate::charset::CharSet;
use crate::error::{Error, Result, Span};
use crate::flavor::{Flavor, MatchPolicy, Profile};
use crate::ir::{
    Assertion, Capture, Domain, Group, LineTerms, Look, Node, NodeKind, Regex, Repeat, WordSet,
};
use crate::tables::{self, Table, js as t};
use std::cmp::Ordering;

pub static PROFILE: Profile = Profile {
    flavor: Flavor::JavaScript,
    engine: "V8 RegExp (Irregexp)",
    unicode: "measured from V8 14.6 (Unicode 17.0)",
    dot_excludes: "\\n \\r \\u2028 \\u2029",
    dollar: "end of haystack only",
    unicode_classes: false,
    case_folding: "Canonicalize (toUpperCase) without u; simple case folding with u",
    match_policy: MatchPolicy::LeftmostFirst,
    lookaround: true,
    backreferences: true,
    linear_time: false,
    spec: "spec/flavours/javascript.md",
};

static SPACE: Table = Table::new(t::SPACE);
static WORD_IU: Table = Table::new(t::WORD_IU);
static ID_START: Table = Table::new(t::ID_START);
static ID_CONTINUE: Table = Table::new(t::ID_CONTINUE);

const NEST_LIMIT: u32 = 250;
const LINE_TERMS: [char; 4] = ['\n', '\r', '\u{2028}', '\u{2029}'];

#[derive(Clone, Copy, Debug, Default)]
struct Flags {
    i: bool,
    m: bool,
    s: bool,
}

/// What the previous term was, for quantifier errors.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Last {
    Atom,
    Repeat,
    Anchor,
    Lookbehind,
}

enum Esc {
    /// A code point or (without `u`) a code unit, possibly a surrogate.
    Char(u32),
    Set(CharSet),
    Assert(Assertion),
    Backref(u32),
    /// Annex B `\c` not followed by a control letter: a literal `\`; the `c` is re-read.
    Backslash,
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    unicode: bool,
    /// `v` flag: Unicode sets (implies Unicode mode).
    vmode: bool,
    flags: Flags,
    group_count: u32,
    names: Vec<String>,
    captures: Vec<Option<String>>,
    /// Alternative path of each named group, for the duplicate-name rule.
    name_paths: Vec<(String, Vec<(u32, u32)>)>,
    path: Vec<(u32, u32)>,
    next_disjunction: u32,
    /// Low surrogate still to be emitted after an astral char split into code units.
    pending_unit: Option<u32>,
    deferred: Option<Error>,
    depth: u32,
}

/// Parses `/source/flags` or a bare source.
pub fn parse(pattern: &str) -> Result<Regex> {
    let (src, offset, flag_text) = split_literal(pattern);
    let mut flags = Flags::default();
    let (mut unicode, mut v, deferred) = (false, false, None);
    let mut sticky = false;
    let mut seen = String::new();
    for (k, f) in flag_text.char_indices() {
        let span = Span::new(offset + src.len() + 1 + k, offset + src.len() + 2 + k);
        if seen.contains(f) {
            return Err(Error::syntax(span, format!("duplicate flag '{f}'")));
        }
        seen.push(f);
        match f {
            'i' => flags.i = true,
            'm' => flags.m = true,
            's' => flags.s = true,
            'u' => unicode = true,
            'v' => v = true,
            'd' | 'g' => {}
            'y' => sticky = true,
            other => return Err(Error::syntax(span, format!("invalid flag '{other}'"))),
        }
    }
    if unicode && v {
        return Err(Error::syntax(
            Span::new(0, pattern.len()),
            "flags u and v are incompatible",
        ));
    }
    let (group_count, names) = prescan(src);
    let mut p = Parser {
        src,
        pos: 0,
        unicode: unicode || v,
        vmode: v,
        flags,
        group_count,
        names,
        captures: Vec::new(),
        name_paths: Vec::new(),
        path: Vec::new(),
        next_disjunction: 0,
        pending_unit: None,
        deferred,
        depth: 0,
    };
    let root = p.disjunction().map_err(|e| shift(e, offset))?;
    if p.peek() == Some(')') {
        return Err(shift(Error::syntax(p.here(), "Unmatched ')'"), offset));
    }
    if let Some(e) = p.deferred {
        return Err(shift(e, offset));
    }
    Ok(Regex {
        flavor: Flavor::JavaScript,
        pattern: pattern.to_string(),
        root: shift_node(root, offset),
        captures: p.captures,
        domain: if unicode || v {
            Domain::Scalars
        } else {
            Domain::Utf16
        },
        quirks: crate::ir::Quirks {
            // A sticky search only tries index 0, so the mid-pair positions never arise.
            mid_pair_search: (unicode || v) && !sticky,
            sticky,
        },
    })
}

/// `/src/flags` → (src, byte offset of src, flags); anything else is a bare source.
fn split_literal(p: &str) -> (&str, usize, &str) {
    if let Some(rest) = p.strip_prefix('/')
        && let Some(end) = rest.rfind('/')
    {
        let flags = &rest[end + 1..];
        if flags.chars().all(|c| c.is_ascii_alphabetic()) {
            return (&rest[..end], 1, flags);
        }
    }
    (p, 0, "")
}

fn shift(mut e: Error, by: usize) -> Error {
    e.span = Span::new(e.span.start + by, e.span.end + by);
    e
}

fn shift_node(mut n: Node, by: usize) -> Node {
    if by == 0 {
        return n;
    }
    n.span = Span::new(n.span.start + by, n.span.end + by);
    n.kind = match n.kind {
        NodeKind::Concat(xs) => {
            NodeKind::Concat(xs.into_iter().map(|x| shift_node(x, by)).collect())
        }
        NodeKind::Alt(xs) => NodeKind::Alt(xs.into_iter().map(|x| shift_node(x, by)).collect()),
        NodeKind::Repeat(mut r) => {
            r.sub = Box::new(shift_node(*r.sub, by));
            NodeKind::Repeat(r)
        }
        NodeKind::Group(mut g) => {
            g.sub = Box::new(shift_node(*g.sub, by));
            NodeKind::Group(g)
        }
        NodeKind::Look(mut l) => {
            l.sub = Box::new(shift_node(*l.sub, by));
            NodeKind::Look(l)
        }
        other => other,
    };
    n
}

/// Counts capturing groups and collects group names, skipping escapes and class contents.
fn prescan(src: &str) -> (u32, Vec<String>) {
    let chars: Vec<char> = src.chars().collect();
    let (mut count, mut names, mut i, mut in_class) = (0u32, Vec::new(), 0, false);
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '[' => in_class = true,
            ']' => in_class = false,
            '(' if !in_class => {
                if chars.get(i + 1) != Some(&'?') {
                    count += 1;
                } else if chars.get(i + 2) == Some(&'<')
                    && !matches!(chars.get(i + 3), Some('=') | Some('!'))
                {
                    count += 1;
                    let name: String = chars[i + 3..].iter().take_while(|&&c| c != '>').collect();
                    names.push(name);
                }
            }
            _ => {}
        }
        i += 1;
    }
    (count, names)
}

/// Compares two decimal digit strings numerically, at any length.
fn cmp_decimal(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn clamp_decimal(s: &str) -> u32 {
    s.parse::<u64>()
        .map_or(u32::MAX, |v| v.min(u32::MAX as u64) as u32)
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_nth(&self, n: usize) -> Option<char> {
        self.src[self.pos..].chars().nth(n)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn eat_str(&mut self, s: &str) -> bool {
        if self.src[self.pos..].starts_with(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    fn here(&self) -> Span {
        Span::new(self.pos, self.pos + self.peek().map_or(0, char::len_utf8))
    }

    fn span_from(&self, start: usize) -> Span {
        Span::new(start, self.pos)
    }

    fn defer(&mut self, e: Error) {
        if self.deferred.is_none() {
            self.deferred = Some(e);
        }
    }

    /// Reads one source character as the engine sees it: a code point with `u`, else a code
    /// unit (astral characters yield their high then low surrogate).
    fn unit(&mut self) -> Option<u32> {
        if let Some(lo) = self.pending_unit.take() {
            return Some(lo);
        }
        let c = self.bump()? as u32;
        if !self.unicode && c > 0xFFFF {
            let v = c - 0x10000;
            self.pending_unit = Some(0xDC00 + (v & 0x3FF));
            return Some(0xD800 + (v >> 10));
        }
        Some(c)
    }

    // ------------------------------------------------------------------ structure

    fn disjunction(&mut self) -> Result<Node> {
        let start = self.pos;
        let id = self.next_disjunction;
        self.next_disjunction += 1;
        let mut alts = Vec::new();
        let mut k = 0;
        loop {
            self.path.push((id, k));
            let alt = self.alternative();
            self.path.pop();
            alts.push(alt?);
            if !self.eat('|') {
                break;
            }
            k += 1;
        }
        Ok(if alts.len() == 1 {
            alts.pop()
                .unwrap_or(Node::new(NodeKind::Empty, self.span_from(start)))
        } else {
            Node::new(NodeKind::Alt(alts), self.span_from(start))
        })
    }

    fn alternative(&mut self) -> Result<Node> {
        let start = self.pos;
        let mut items: Vec<(Node, Last)> = Vec::new();
        loop {
            if self.pending_unit.is_some() {
                let lo = self.unit().unwrap_or(0);
                let n = self.char_node(lo, self.span_from(self.pos));
                items.push((n, Last::Atom));
                continue;
            }
            let Some(c) = self.peek() else { break };
            let at = self.pos;
            match c {
                '|' | ')' => break,
                '*' | '+' | '?' | '{' => {
                    let (min, max) = match c {
                        '*' => (0, None),
                        '+' => (1, None),
                        '?' => (0, Some(1)),
                        _ => match self.braced_quantifier()? {
                            Some(q) => q,
                            None => {
                                if self.unicode {
                                    return Err(Error::syntax(
                                        self.here(),
                                        "Incomplete quantifier",
                                    ));
                                }
                                self.bump();
                                let n = self.char_node('{' as u32, self.span_from(at));
                                items.push((n, Last::Atom));
                                continue;
                            }
                        },
                    };
                    if c != '{' {
                        self.bump();
                    }
                    let last = items.last().map(|(_, l)| *l);
                    match last {
                        Some(Last::Atom) => {}
                        Some(Last::Lookbehind) => {
                            return Err(Error::syntax(self.span_from(at), "Invalid quantifier"));
                        }
                        _ => return Err(Error::syntax(self.span_from(at), "Nothing to repeat")),
                    }
                    let (item, _) = items
                        .pop()
                        .unwrap_or((Node::new(NodeKind::Empty, Span::default()), Last::Atom));
                    let lazy = self.eat('?');
                    let span = item.span.to(self.span_from(at));
                    items.push((
                        Node::new(
                            NodeKind::Repeat(Repeat {
                                sub: Box::new(item),
                                min,
                                max,
                                greedy: !lazy,
                                possessive: false,
                            }),
                            span,
                        ),
                        Last::Repeat,
                    ));
                }
                '^' | '$' => {
                    self.bump();
                    let a = match (c, self.flags.m) {
                        ('^', false) => Assertion::TextStart,
                        ('^', true) => Assertion::LineStart {
                            terms: LineTerms::Js,
                        },
                        (_, false) => Assertion::TextEnd,
                        (_, true) => Assertion::LineEnd {
                            terms: LineTerms::Js,
                        },
                    };
                    items.push((
                        Node::new(NodeKind::Assert(a), self.span_from(at)),
                        Last::Anchor,
                    ));
                }
                '(' => {
                    let (node, last) = self.group()?;
                    items.push((node, last));
                }
                '[' if self.vmode => {
                    let set = self.class_v()?;
                    let node = self.vset_node(set, self.span_from(at));
                    items.push((node, Last::Atom));
                }
                '[' => {
                    let set = self.class()?;
                    items.push((
                        Node::new(NodeKind::Class(set), self.span_from(at)),
                        Last::Atom,
                    ));
                }
                '.' => {
                    self.bump();
                    let all = self.negate(&CharSet::empty());
                    let set = if self.flags.s {
                        all
                    } else {
                        all.difference(&LINE_TERMS.iter().copied().collect())
                    };
                    items.push((
                        Node::new(NodeKind::Class(set), self.span_from(at)),
                        Last::Atom,
                    ));
                }
                ']' | '}' if self.unicode => {
                    return Err(Error::syntax(self.here(), "Lone quantifier brackets"));
                }
                '\\' => {
                    let esc = self.escape(false)?;
                    let span = self.span_from(at);
                    let item = match esc {
                        Esc::Char(v) => (self.char_node(v, span), Last::Atom),
                        // Under `i` the character-set matcher compares canonicalised chars,
                        // so a class escape behaves as its case closure.
                        Esc::Set(s) => (Node::new(NodeKind::Class(self.fold(s)), span), Last::Atom),
                        Esc::Assert(a) => (Node::new(NodeKind::Assert(a), span), Last::Anchor),
                        Esc::Backref(g) => {
                            let case = if self.flags.i {
                                crate::ir::BackrefCase::Js {
                                    unicode: self.unicode,
                                }
                            } else {
                                crate::ir::BackrefCase::Exact
                            };
                            (Node::new(NodeKind::Backref(g, case), span), Last::Atom)
                        }
                        Esc::Backslash => (self.char_node('\\' as u32, span), Last::Atom),
                    };
                    items.push(item);
                }
                _ => {
                    let v = self.unit().unwrap_or(0);
                    let n = self.char_node(v, self.span_from(at));
                    items.push((n, Last::Atom));
                }
            }
        }
        let span = self.span_from(start);
        let mut nodes: Vec<Node> = items.into_iter().map(|(n, _)| n).collect();
        Ok(match nodes.len() {
            0 => Node::new(NodeKind::Empty, span),
            1 => nodes.pop().unwrap_or(Node::new(NodeKind::Empty, span)),
            _ => Node::new(NodeKind::Concat(nodes), span),
        })
    }

    /// At `{`: a valid `{n}`, `{n,}` or `{n,m}` (consumed), else `None` (nothing consumed).
    fn braced_quantifier(&mut self) -> Result<Option<(u32, Option<u32>)>> {
        let rest = &self.src[self.pos + 1..];
        let lo_len = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if lo_len == 0 {
            return Ok(None);
        }
        let lo = &rest[..lo_len];
        let after = &rest[lo_len..];
        let (hi, used) = if let Some(r) = after.strip_prefix(',') {
            let hi_len = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
            if !r[hi_len..].starts_with('}') {
                return Ok(None);
            }
            (Some(&r[..hi_len]), 1 + hi_len)
        } else if after.starts_with('}') {
            (None, 0)
        } else {
            return Ok(None);
        };
        let start = self.pos;
        self.pos += 1 + lo_len + used + 1;
        let max = match hi {
            None => Some(clamp_decimal(lo)),
            Some("") => None,
            Some(h) => {
                if cmp_decimal(lo, h) == Ordering::Greater {
                    return Err(Error::syntax(
                        self.span_from(start),
                        "numbers out of order in {} quantifier",
                    ));
                }
                Some(clamp_decimal(h))
            }
        };
        Ok(Some((clamp_decimal(lo), max)))
    }

    fn char_node(&self, v: u32, span: Span) -> Node {
        let Some(c) = char::from_u32(v) else {
            // A surrogate code unit: matchable only without `u` (ADR-0007).
            return Node::new(NodeKind::Class(self.unit_set(v, v)), span);
        };
        if !self.flags.i {
            return Node::new(NodeKind::Literal(c), span);
        }
        let set = self.fold(CharSet::single(c));
        if set.len() == 1 {
            Node::new(NodeKind::Literal(c), span)
        } else {
            Node::new(NodeKind::Class(set), span)
        }
    }

    /// `lo..=hi` as a set of matchable characters: without `u`, surrogate code units become
    /// pseudo code points; with `u`, lone surrogate code points never match.
    fn unit_set(&self, lo: u32, hi: u32) -> CharSet {
        if self.unicode || hi < 0xD800 || lo > 0xDFFF {
            return CharSet::from_ranges([(lo, hi)]);
        }
        let p = |u: u32| crate::charset::PSEUDO_BASE + (u - 0xD800);
        CharSet::from_ranges([
            (lo, hi.min(0xD7FF)),
            (p(lo.max(0xD800)), p(hi.min(0xDFFF))),
            (lo.max(0xE000), hi),
        ])
    }

    fn atom_set(&self, a: &Esc) -> CharSet {
        match a {
            Esc::Char(v) => self.unit_set(*v, *v),
            Esc::Set(s) => s.clone(),
            _ => CharSet::empty(),
        }
    }

    /// Complement over what a position can hold: code points with `u`, code units without.
    fn negate(&self, set: &CharSet) -> CharSet {
        if self.unicode {
            set.complement()
        } else {
            set.units_complement()
        }
    }

    fn fold(&self, set: CharSet) -> CharSet {
        match (self.flags.i, self.unicode) {
            (false, _) => set,
            (true, false) => tables::fold_with(&set, t::FOLD),
            (true, true) => tables::fold_with(&set, t::FOLD_U),
        }
    }

    fn class_escape_set(&self, c: char) -> Option<CharSet> {
        let word = if self.unicode && self.flags.i {
            WORD_IU.get().clone()
        } else {
            crate::unicode::posix_class("word")?
        };
        let base = match c.to_ascii_lowercase() {
            'd' => CharSet::range('0', '9'),
            's' => SPACE.get().clone(),
            'w' => word,
            _ => return None,
        };
        Some(if c.is_ascii_uppercase() {
            self.negate(&base)
        } else {
            base
        })
    }

    // ------------------------------------------------------------------ escapes

    fn hex(&mut self, n: usize) -> Option<u32> {
        let s = self.src.get(self.pos..)?;
        let digits: String = s.chars().take(n).collect();
        if digits.chars().count() == n && digits.chars().all(|c| c.is_ascii_hexdigit()) {
            self.pos += n;
            u32::from_str_radix(&digits, 16).ok()
        } else {
            None
        }
    }

    /// After `\u`: `XXXX` (joining a following `\uXXXX` low surrogate in `u` mode) or `{X…}`
    /// (only in `u` mode or inside group names). `None` if malformed (nothing consumed).
    fn unicode_escape(&mut self, braces: bool) -> Option<u32> {
        if braces && self.peek() == Some('{') {
            let rest = &self.src[self.pos + 1..];
            let len = rest.find('}')?;
            let digits = &rest[..len];
            if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            let v = u32::from_str_radix(digits, 16)
                .ok()
                .filter(|&v| v <= 0x10_FFFF)?;
            self.pos += len + 2;
            return Some(v);
        }
        let v = self.hex(4)?;
        if self.unicode && (0xD800..0xDC00).contains(&v) && self.src[self.pos..].starts_with("\\u")
        {
            let save = self.pos;
            self.pos += 2;
            match self.hex(4) {
                Some(lo) if (0xDC00..0xE000).contains(&lo) => {
                    return Some(0x10000 + ((v - 0xD800) << 10) + (lo - 0xDC00));
                }
                _ => self.pos = save,
            }
        }
        Some(v)
    }

    /// At `\`.
    fn escape(&mut self, in_class: bool) -> Result<Esc> {
        let start = self.pos;
        self.bump();
        let Some(c) = self.bump() else {
            return Err(Error::syntax(self.span_from(start), "\\ at end of pattern"));
        };
        let bad = |p: &Self| Error::syntax(p.span_from(start), "Invalid escape");
        let u = self.unicode;
        let ok = |v: char| Ok(Esc::Char(v as u32));
        match c {
            'd' | 'D' | 's' | 'S' | 'w' | 'W' => {
                Ok(Esc::Set(self.class_escape_set(c).unwrap_or_default()))
            }
            'b' if in_class => Ok(Esc::Char(8)),
            'b' | 'B' if !in_class => {
                let word = if u && self.flags.i {
                    WordSet::JsIgnoreCaseUnicode
                } else {
                    WordSet::Ascii
                };
                Ok(Esc::Assert(if c == 'b' {
                    Assertion::WordBoundary { word }
                } else {
                    Assertion::NotWordBoundary { word }
                }))
            }
            'f' => ok('\x0C'),
            'n' => ok('\n'),
            'r' => ok('\r'),
            't' => ok('\t'),
            'v' => ok('\x0B'),
            'c' => {
                let next = self.peek();
                match next {
                    Some(l) if l.is_ascii_alphabetic() => {
                        self.bump();
                        Ok(Esc::Char(l as u32 % 32))
                    }
                    Some(l) if in_class && !u && (l.is_ascii_digit() || l == '_') => {
                        self.bump();
                        Ok(Esc::Char(l as u32 % 32))
                    }
                    _ if u => Err(bad(self)),
                    _ => {
                        // Annex B: `\` is literal and the `c` is read again as a character.
                        self.pos -= 1;
                        Ok(Esc::Backslash)
                    }
                }
            }
            'x' => match self.hex(2) {
                Some(v) => Ok(Esc::Char(v)),
                None if u => Err(bad(self)),
                None => ok('x'),
            },
            'u' => match self.unicode_escape(u) {
                Some(v) => Ok(Esc::Char(v)),
                None if u => Err(bad(self)),
                None => ok('u'),
            },
            '0' if !self.peek().is_some_and(|d| d.is_ascii_digit()) => Ok(Esc::Char(0)),
            '0'..='9' => self.decimal_escape(c, start, in_class),
            'k' => {
                if !u && self.names.is_empty() {
                    return ok('k');
                }
                if in_class {
                    return Err(bad(self));
                }
                if !self.eat('<') {
                    return Err(Error::syntax(
                        self.span_from(start),
                        "Invalid named reference",
                    ));
                }
                let name_start = self.pos;
                let Some(len) = self.src[self.pos..].find('>') else {
                    return Err(Error::syntax(
                        self.span_from(start),
                        "Invalid named reference",
                    ));
                };
                let name = self.src[name_start..name_start + len].to_string();
                self.pos += len + 1;
                match self.names.iter().position(|n| *n == name) {
                    Some(_) => {
                        // Index of the first capture with this name (duplicates share a name).
                        let index = self.capture_index_of(&name);
                        Ok(Esc::Backref(index))
                    }
                    None => Err(Error::syntax(
                        self.span_from(start),
                        "Invalid named capture referenced",
                    )),
                }
            }
            'p' | 'P' if u => {
                if !self.eat('{') || !self.src[self.pos..].contains('}') {
                    return Err(Error::syntax(
                        self.span_from(start),
                        "Invalid property name",
                    ));
                }
                let len = self.src[self.pos..].find('}').unwrap_or(0);
                let body = self.src[self.pos..self.pos + len].to_string();
                self.pos += len + 1;
                if self.vmode && PROPERTIES_OF_STRINGS.contains(&body.as_str()) && !in_class {
                    if c == 'P' {
                        return Err(Error::syntax(
                            self.span_from(start),
                            "Invalid property name",
                        ));
                    }
                    self.defer(Error::unsupported(
                        self.span_from(start),
                        "properties of strings (emoji sequences) need sequence data (not bundled yet)",
                    ));
                    return Ok(Esc::Set(CharSet::empty()));
                }
                let Some(set) = js_property(&body) else {
                    return Err(Error::syntax(
                        self.span_from(start),
                        "Invalid property name",
                    ));
                };
                Ok(Esc::Set(if c == 'P' { set.complement() } else { set }))
            }
            '-' if u && in_class => ok('-'),
            'B' if in_class && u => Err(bad(self)),
            c if u => {
                if "^$\\.*+?()[]{}|/".contains(c) {
                    ok(c)
                } else {
                    Err(bad(self))
                }
            }
            c => {
                // Annex B identity escape. An astral char splits into code units.
                self.pos -= c.len_utf8();
                Ok(Esc::Char(self.unit().unwrap_or(c as u32)))
            }
        }
    }

    /// The capture index of a named group, counting groups in source order (pre-scan order).
    fn capture_index_of(&self, name: &str) -> u32 {
        let chars: Vec<char> = self.src.chars().collect();
        let (mut count, mut i, mut in_class) = (0u32, 0, false);
        while i < chars.len() {
            match chars[i] {
                '\\' => i += 1,
                '[' => in_class = true,
                ']' => in_class = false,
                '(' if !in_class => {
                    if chars.get(i + 1) != Some(&'?') {
                        count += 1;
                    } else if chars.get(i + 2) == Some(&'<')
                        && !matches!(chars.get(i + 3), Some('=') | Some('!'))
                    {
                        count += 1;
                        let n: String = chars[i + 3..].iter().take_while(|&&c| c != '>').collect();
                        if n == name {
                            return count;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
        0
    }

    /// `\1`…: a backreference if that group exists, else (Annex B) octal or identity.
    fn decimal_escape(&mut self, first: char, start: usize, in_class: bool) -> Result<Esc> {
        let mut digits = first.to_string();
        while let Some(d) = self.peek().filter(char::is_ascii_digit) {
            digits.push(d);
            self.bump();
        }
        if !in_class && cmp_decimal(&digits, &self.group_count.to_string()) != Ordering::Greater {
            return Ok(Esc::Backref(clamp_decimal(&digits)));
        }
        if self.unicode {
            return Err(Error::syntax(self.span_from(start), "Invalid escape"));
        }
        // Annex B: re-read from just after the backslash.
        self.pos = start + 1;
        let first = self.bump().unwrap_or('0');
        if first == '8' || first == '9' {
            return Ok(Esc::Char(first as u32));
        }
        // LegacyOctalEscapeSequence: up to three octal digits, value <= 0o377.
        let mut v = first.to_digit(8).unwrap_or(0);
        let max_more = if first <= '3' { 2 } else { 1 };
        for _ in 0..max_more {
            match self.peek().and_then(|d| d.to_digit(8)) {
                Some(d) => {
                    v = v * 8 + d;
                    self.bump();
                }
                None => break,
            }
        }
        Ok(Esc::Char(v))
    }

    // ------------------------------------------------------------------ classes

    /// One class atom: a char/unit or a class escape set.
    fn class_atom(&mut self) -> Result<Esc> {
        if let Some(lo) = self.pending_unit.take() {
            return Ok(Esc::Char(lo));
        }
        if self.peek() == Some('\\') {
            return match self.escape(true)? {
                Esc::Backslash => Ok(Esc::Char('\\' as u32)),
                e => Ok(e),
            };
        }
        Ok(Esc::Char(self.unit().unwrap_or(0)))
    }

    fn class(&mut self) -> Result<CharSet> {
        let open = self.here();
        self.bump(); // [
        self.enter(open)?;
        let negated = self.eat('^');
        let mut set = CharSet::empty();
        let unit_set = |v: u32| CharSet::from_ranges([(v, v)]);
        loop {
            if self.pending_unit.is_none() {
                if self.eat(']') {
                    break;
                }
                if self.peek().is_none() {
                    self.depth -= 1;
                    return Err(Error::syntax(open, "Unterminated character class"));
                }
            }
            let item_start = self.pos;
            let a = self.class_atom()?;
            let dash_range = self.pending_unit.is_none()
                && self.peek() == Some('-')
                && self.peek_nth(1).is_some_and(|c| c != ']');
            if !dash_range {
                set = set.union(&self.atom_set(&a));
                continue;
            }
            self.bump(); // -
            let b = self.class_atom()?;
            match (&a, &b) {
                (Esc::Char(lo), Esc::Char(hi)) => {
                    if hi < lo {
                        self.depth -= 1;
                        return Err(Error::syntax(
                            self.span_from(item_start),
                            "Range out of order in character class",
                        ));
                    }
                    set = set.union(&self.unit_set(*lo, *hi));
                }
                _ if self.unicode => {
                    self.depth -= 1;
                    return Err(Error::syntax(
                        self.span_from(item_start),
                        "Invalid character class",
                    ));
                }
                _ => {
                    set = set
                        .union(&self.atom_set(&a))
                        .union(&unit_set('-' as u32))
                        .union(&self.atom_set(&b));
                }
            }
        }
        self.depth -= 1;
        let set = self.fold(set);
        Ok(if negated { self.negate(&set) } else { set })
    }

    // ------------------------------------------------------------------ v-mode classes

    /// Emits a `v`-mode class: its strings (longest first, as the engine tries them) and chars.
    fn vset_node(&self, v: VSet, span: Span) -> Node {
        let mut alts: Vec<Node> = Vec::new();
        let mut strings: Vec<&Vec<char>> = v.strings.iter().collect();
        strings.sort_by_key(|s| std::cmp::Reverse(s.len()));
        for st in strings {
            let parts: Vec<Node> = st.iter().map(|&c| self.char_node(c as u32, span)).collect();
            alts.push(match parts.len() {
                0 => Node::new(NodeKind::Empty, span),
                _ => Node::new(NodeKind::Concat(parts), span),
            });
        }
        let chars = Node::new(NodeKind::Class(self.fold(v.chars)), span);
        if alts.is_empty() {
            return chars;
        }
        alts.push(chars);
        Node::new(NodeKind::Alt(alts), span)
    }

    /// Canonical form of a string element under `i` (so set operations compare folded strings).
    fn canon(&self, c: char) -> char {
        if !self.flags.i {
            return c;
        }
        self.fold(CharSet::single(c)).min_char().unwrap_or(c)
    }

    /// `[ ClassContents ]` in `v` mode, cursor at `[`.
    fn class_v(&mut self) -> Result<VSet> {
        let open = self.here();
        self.bump();
        self.enter(open)?;
        let negated = self.eat('^');
        let r = self.class_contents_v(open);
        self.depth -= 1;
        let mut v = r?;
        if !self.eat(']') {
            return Err(Error::syntax(open, "Unterminated character class"));
        }
        if negated {
            if v.may_strings {
                return Err(Error::syntax(
                    open,
                    "Negated character class may contain strings",
                ));
            }
            let folded = self.fold(v.chars);
            v = VSet::chars(folded.complement());
        }
        Ok(v)
    }

    fn class_contents_v(&mut self, open: Span) -> Result<VSet> {
        let invalid_op =
            |p: &Self| Error::syntax(p.here(), "Invalid set operation in character class");
        if self.peek() == Some(']') {
            return Ok(VSet::default());
        }
        if self.rest_starts("&&") || self.rest_starts("--") {
            return Err(invalid_op(self));
        }
        let (first, first_is_range) = self.class_item_v(true)?;
        if self.rest_starts("&&") || self.rest_starts("--") {
            if first_is_range {
                return Err(invalid_op(self));
            }
            let op = self.peek();
            let mut acc = first;
            while (op == Some('&') && self.rest_starts("&&"))
                || (op == Some('-') && self.rest_starts("--"))
            {
                self.pos += 2;
                if op == Some('&') && self.peek() == Some('&') {
                    return Err(Error::syntax(
                        self.here(),
                        "Invalid character in character class",
                    ));
                }
                if self.peek() == Some(']') {
                    return Err(Error::syntax(
                        self.here(),
                        "Invalid character in character class",
                    ));
                }
                let (rhs, is_range) = self.class_item_v(false)?;
                if is_range {
                    return Err(invalid_op(self));
                }
                acc = if op == Some('&') {
                    acc.intersect(rhs)
                } else {
                    acc.subtract(rhs)
                };
            }
            if self.peek() != Some(']') {
                return Err(invalid_op(self));
            }
            return Ok(acc);
        }
        let mut acc = first;
        loop {
            match self.peek() {
                Some(']') => return Ok(acc),
                None => return Err(Error::syntax(open, "Unterminated character class")),
                _ if self.rest_starts("&&") || self.rest_starts("--") => {
                    return Err(invalid_op(self));
                }
                _ => {
                    let (item, _) = self.class_item_v(true)?;
                    acc = acc.union(item);
                }
            }
        }
    }

    fn rest_starts(&self, s: &str) -> bool {
        self.src[self.pos..].starts_with(s)
    }

    /// One operand (nested class, `\q{..}`, class escape) or, when `ranges`, a range.
    /// Returns whether it was a range or a single character (`a-z` / `a`): ranges cannot be
    /// set-operation operands.
    fn class_item_v(&mut self, ranges: bool) -> Result<(VSet, bool)> {
        let start = self.pos;
        match self.peek() {
            Some('[') => return Ok((self.class_v()?, false)),
            Some('\\') if self.rest_starts("\\q{") => {
                self.pos += 3;
                return Ok((self.class_strings()?, false));
            }
            Some('\\') => {
                let next = self.src[self.pos + 1..].chars().next();
                if let Some(c @ ('d' | 'D' | 's' | 'S' | 'w' | 'W' | 'p' | 'P')) = next {
                    if matches!(c, 'p' | 'P') {
                        let body: String = self.src[self.pos + 2..]
                            .strip_prefix('{')
                            .and_then(|r| r.split('}').next())
                            .unwrap_or("")
                            .to_string();
                        if PROPERTIES_OF_STRINGS.contains(&body.as_str()) {
                            self.pos += 3 + body.len() + 1;
                            if c == 'P' {
                                return Err(Error::syntax(
                                    self.span_from(start),
                                    "Invalid property name",
                                ));
                            }
                            self.defer(Error::unsupported(
                                self.span_from(start),
                                "properties of strings (emoji sequences) need sequence data (not bundled yet)",
                            ));
                            return Ok((
                                VSet {
                                    may_strings: true,
                                    ..VSet::default()
                                },
                                false,
                            ));
                        }
                    }
                    let Esc::Set(set) = self.escape(true)? else {
                        return Err(Error::syntax(self.span_from(start), "Invalid escape"));
                    };
                    return Ok((VSet::chars(set), false));
                }
            }
            _ => {}
        }
        let lo = self.class_set_char()?;
        if ranges && self.peek() == Some('-') && !self.rest_starts("--") {
            self.bump();
            let hi = self.class_set_char()?;
            if hi < lo {
                return Err(Error::syntax(
                    self.span_from(start),
                    "Range out of order in character class",
                ));
            }
            return Ok((VSet::chars(CharSet::from_ranges([(lo, hi)])), true));
        }
        Ok((VSet::chars(CharSet::from_ranges([(lo, lo)])), false))
    }

    /// `ClassSetCharacter`.
    fn class_set_char(&mut self) -> Result<u32> {
        let bad = |p: &Self| Error::syntax(p.here(), "Invalid character in character class");
        let Some(c) = self.peek() else {
            return Err(bad(self));
        };
        const DOUBLE: &str = "&!#$%*+,.:;<=>?@^`~";
        if DOUBLE.contains(c) && self.src[self.pos + c.len_utf8()..].starts_with(c) {
            return Err(Error::syntax(
                self.here(),
                "Invalid set operation in character class",
            ));
        }
        if c == '\\' {
            let next = self.src[self.pos + 1..].chars().next();
            match next {
                Some('b') => {
                    self.pos += 2;
                    return Ok(8);
                }
                Some(p) if "&-!#%,:;<=>@`~".contains(p) => {
                    self.pos += 1 + p.len_utf8();
                    return Ok(p as u32);
                }
                _ => {}
            }
            return match self.escape(true)? {
                Esc::Char(v) => Ok(v),
                _ => Err(Error::syntax(self.here(), "Invalid escape")),
            };
        }
        if "()[]{}/-|".contains(c) {
            return Err(bad(self));
        }
        self.bump();
        Ok(c as u32)
    }

    /// After `\\q{`: `ClassString (| ClassString)* }`.
    fn class_strings(&mut self) -> Result<VSet> {
        let mut v = VSet::default();
        let mut cur: Vec<char> = Vec::new();
        loop {
            match self.peek() {
                None => return Err(Error::syntax(self.here(), "Invalid escape")),
                Some('}') => {
                    self.bump();
                    break;
                }
                Some('|') => {
                    self.bump();
                    self.push_string(&mut v, std::mem::take(&mut cur));
                }
                _ => {
                    let c = self.class_set_char()?;
                    cur.push(char::from_u32(c).unwrap_or('\u{FFFD}'));
                }
            }
        }
        self.push_string(&mut v, cur);
        Ok(v)
    }

    fn push_string(&self, v: &mut VSet, s: Vec<char>) {
        if s.len() == 1 {
            v.chars = v.chars.union(&CharSet::single(s[0]));
        } else {
            v.may_strings = true;
            v.strings
                .insert(s.into_iter().map(|c| self.canon(c)).collect());
        }
    }

    fn enter(&mut self, span: Span) -> Result<()> {
        self.depth += 1;
        if self.depth > NEST_LIMIT {
            return Err(Error::unsupported(
                span,
                "nesting too deep to model V8's stack limit",
            ));
        }
        Ok(())
    }

    // ------------------------------------------------------------------ groups

    fn group(&mut self) -> Result<(Node, Last)> {
        let start = self.pos;
        let open = self.here();
        self.bump(); // (
        enum Kind {
            Capture(Option<String>),
            NonCapture(Option<Flags>),
            Look { behind: bool, negated: bool },
        }
        let kind = if self.eat('?') {
            if self.eat(':') {
                Kind::NonCapture(None)
            } else if self.eat('=') {
                Kind::Look {
                    behind: false,
                    negated: false,
                }
            } else if self.eat('!') {
                Kind::Look {
                    behind: false,
                    negated: true,
                }
            } else if self.eat_str("<=") {
                Kind::Look {
                    behind: true,
                    negated: false,
                }
            } else if self.eat_str("<!") {
                Kind::Look {
                    behind: true,
                    negated: true,
                }
            } else if self.eat('<') {
                Kind::Capture(Some(self.group_name(start)?))
            } else {
                Kind::NonCapture(Some(self.modifiers(start)?))
            }
        } else {
            Kind::Capture(None)
        };
        let capture = match &kind {
            Kind::Capture(name) => {
                if let Some(n) = name {
                    let here = self.path.clone();
                    for (other, path) in &self.name_paths {
                        if other == n && !disjoint(path, &here) {
                            return Err(Error::syntax(
                                self.span_from(start),
                                "Duplicate capture group name",
                            ));
                        }
                    }
                    self.name_paths.push((n.clone(), here));
                }
                self.captures.push(name.clone());
                Some(Capture {
                    index: self.captures.len() as u32,
                    name: name.clone(),
                })
            }
            _ => None,
        };
        self.enter(open)?;
        let saved = self.flags;
        if let Kind::NonCapture(Some(f)) = &kind {
            self.flags = *f;
        }
        let sub = self.disjunction();
        self.flags = saved;
        self.depth -= 1;
        let sub = sub?;
        if !self.eat(')') {
            return Err(Error::syntax(open, "Unterminated group"));
        }
        let span = self.span_from(start);
        Ok(match kind {
            Kind::Look { behind, negated } => (
                Node::new(
                    NodeKind::Look(Look {
                        sub: Box::new(sub),
                        behind,
                        negated,
                        window: None,
                        window_empty: false,
                    }),
                    span,
                ),
                // Annex B: look-aheads are quantifiable without `u`.
                if behind || self.unicode {
                    Last::Lookbehind
                } else {
                    Last::Atom
                },
            ),
            _ => (
                Node::new(
                    NodeKind::Group(Group {
                        sub: Box::new(sub),
                        capture,
                        atomic: false,
                    }),
                    span,
                ),
                Last::Atom,
            ),
        })
    }

    /// After `(?<`: an IdentifierName (with `\u` escapes) up to `>`.
    fn group_name(&mut self, start: usize) -> Result<String> {
        let bad = |p: &Self| Error::syntax(p.span_from(start), "Invalid capture group name");
        let mut name = String::new();
        loop {
            let c = match self.peek() {
                None => return Err(bad(self)),
                Some('>') => {
                    self.bump();
                    break;
                }
                Some('\\') => {
                    self.bump();
                    if !self.eat('u') {
                        return Err(bad(self));
                    }
                    let save = self.unicode;
                    self.unicode = true; // \u{...} and surrogate pairs are allowed in names
                    let v = self.unicode_escape(true);
                    self.unicode = save;
                    v.and_then(char::from_u32).ok_or_else(|| bad(self))?
                }
                Some(c) => {
                    self.bump();
                    c
                }
            };
            let table = if name.is_empty() {
                &ID_START
            } else {
                &ID_CONTINUE
            };
            if !table.get().contains(c) {
                return Err(bad(self));
            }
            name.push(c);
        }
        if name.is_empty() {
            return Err(bad(self));
        }
        Ok(name)
    }

    /// After `(?`: `ims-ims:` modifiers.
    fn modifiers(&mut self, start: usize) -> Result<Flags> {
        let invalid = |p: &Self, m: &str| Error::syntax(p.span_from(start), m.to_string());
        let mut f = self.flags;
        let mut seen = String::new();
        let mut removing = false;
        let (mut add_any, mut del_any) = (false, false);
        loop {
            match self.bump() {
                Some(':') => break,
                Some('-') if !removing => removing = true,
                Some(c @ ('i' | 'm' | 's')) => {
                    if seen.contains(c) {
                        return Err(invalid(self, "Repeated flag in flag group"));
                    }
                    seen.push(c);
                    let v = !removing;
                    if removing {
                        del_any = true;
                    } else {
                        add_any = true;
                    }
                    match c {
                        'i' => f.i = v,
                        'm' => f.m = v,
                        _ => f.s = v,
                    }
                }
                _ => return Err(invalid(self, "Invalid group")),
            }
        }
        if !add_any && !del_any {
            return Err(invalid(self, "Invalid flag group"));
        }
        Ok(f)
    }
}

/// A `v`-mode class value: characters plus strings of length != 1 (ES2024 ClassSetExpression).
#[derive(Clone, Default)]
struct VSet {
    chars: CharSet,
    strings: std::collections::BTreeSet<Vec<char>>,
    may_strings: bool,
}

impl VSet {
    fn chars(c: CharSet) -> VSet {
        VSet {
            chars: c,
            ..VSet::default()
        }
    }
    fn union(mut self, o: VSet) -> VSet {
        self.chars = self.chars.union(&o.chars);
        self.strings.extend(o.strings);
        self.may_strings |= o.may_strings;
        self
    }
    fn intersect(mut self, o: VSet) -> VSet {
        self.chars = self.chars.intersect(&o.chars);
        self.strings.retain(|s| o.strings.contains(s));
        self.may_strings &= o.may_strings;
        self
    }
    fn subtract(mut self, o: VSet) -> VSet {
        self.chars = self.chars.difference(&o.chars);
        self.strings.retain(|s| !o.strings.contains(s));
        self
    }
}

/// ES2024 properties of strings (sets of emoji sequences).
const PROPERTIES_OF_STRINGS: &[&str] = &[
    "Basic_Emoji",
    "Emoji_Keycap_Sequence",
    "RGI_Emoji_Modifier_Sequence",
    "RGI_Emoji_Flag_Sequence",
    "RGI_Emoji_Tag_Sequence",
    "RGI_Emoji_ZWJ_Sequence",
    "RGI_Emoji",
];

/// `\p{body}` in u mode, from tables measured on V8 (exact-case names).
fn js_property(body: &str) -> Option<CharSet> {
    use crate::tables::js_props as p;
    let find = |list: &[(&str, &[(u32, u32)])], name: &str| {
        list.iter()
            .find(|(n, _)| *n == name)
            .map(|(_, r)| CharSet::from_ranges(r.iter().copied()))
    };
    match body.split_once('=') {
        Some(("General_Category" | "gc", v)) => find(p::GC, v),
        Some(("Script" | "sc", v)) => find(p::SC, v),
        Some(("Script_Extensions" | "scx", v)) => find(p::SCX, v),
        Some(_) => None,
        None => find(p::LONE, body),
    }
}

/// Two alternative paths are disjoint when they take different branches of a shared
/// disjunction: then groups on them can never both participate.
fn disjoint(a: &[(u32, u32)], b: &[(u32, u32)]) -> bool {
    a.iter().zip(b).any(|(x, y)| x.0 == y.0 && x.1 != y.1)
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
    fn annex_b_versus_unicode() {
        assert_eq!(ok("a{,2}"), "(cat 'a' '{' ',' '2' '}')");
        assert_eq!(ok("]"), "']'");
        assert_eq!(ok(r"\u{2}"), "(rep{2,2} 'u')");
        assert_eq!(ok(r"/\u{41}/u"), "'A'");
        assert!(ok(r"[\d-z]").len() > 3);
        assert_eq!(ok("[]"), "[]");
        assert_eq!(ok(r"\8"), "'8'");
        assert_eq!(ok(r"\1(a)"), r"(cat \1 (cap1 'a'))");
        for p in [
            "/]/u",
            r"/\8/u",
            r"/[\d-z]/u",
            "/(?=a)*/u",
            "(?<=a)*",
            "a**",
            "^*",
            "{2}",
            "a{2,1}",
            "(?i-i:a)",
            "(?<n>a)(?<n>b)",
            r"\k(?<n>a)",
            "(?i)",
            "[z-a]",
            "/a/uv",
            "/a/ii",
        ] {
            syntax(p);
        }
        assert!(!ok("(?<n>a)|(?<n>b)").is_empty());
    }

    #[test]
    fn dollar_and_domain() {
        assert_eq!(ok("a$"), "(cat 'a' TextEnd)");
        assert_eq!(parse("a").unwrap().domain, Domain::Utf16);
        assert_eq!(parse("/a/u").unwrap().domain, Domain::Scalars);
    }
}
