//! Front-end for Python `re` (CPython 3.15, `str` patterns). Spec: `spec/flavours/python.md`.
//!
//! The control flow deliberately mirrors CPython's `Lib/re/_parser.py` (`_parse_sub`, `_parse`,
//! `_escape`, `_class_escape`, `_parse_flags`) so that acceptance matches the engine rule for
//! rule; the compile-time look-behind width check from `_compiler.py` is applied at the end.
//! Classes and case-insensitivity use tables measured from the engine (`tables::python`).

use crate::charset::CharSet;
use crate::error::{Error, Result, Span};
use crate::flavor::{Flavor, MatchPolicy, Profile};
use crate::ir::{
    Assertion, Capture, Conditional, Group, LineTerms, Look, Node, NodeKind, Regex, Repeat, WordSet,
};
use crate::tables::{self, Table, python as t};
use crate::unicode;

pub static PROFILE: Profile = Profile {
    flavor: Flavor::Python,
    engine: "CPython re",
    unicode: "measured from CPython 3.15.2 (Unicode 17.0.0)",
    dot_excludes: "\\n",
    dollar: "end of haystack or before a final \\n",
    unicode_classes: true,
    case_folding: "CPython re (?i) relation, measured",
    match_policy: MatchPolicy::LeftmostFirst,
    lookaround: true,
    backreferences: true,
    linear_time: false,
    spec: "spec/flavours/python.md",
};

static WORD: Table = Table::new(t::WORD);
static DIGIT: Table = Table::new(t::DIGIT);
static SPACE: Table = Table::new(t::SPACE);
static IDENT_START: Table = Table::new(t::IDENT_START);
static IDENT_CONTINUE: Table = Table::new(t::IDENT_CONTINUE);

/// `_sre.MAXREPEAT`: counts must be strictly below it.
const MAXREPEAT: u64 = 4_294_967_295;
/// CPython's parser recurses per nesting level; beyond this we do not try to model where its
/// recursion limit falls and report the pattern as unsupported rather than guess.
const NEST_LIMIT: u32 = 200;

#[derive(Clone, Copy, Debug, Default)]
struct Flags {
    i: bool,
    m: bool,
    s: bool,
    x: bool,
    ascii: bool,
}

/// A token as CPython's tokenizer sees it: one char, or a backslash plus the next char.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tok {
    Char(char),
    Esc(char),
}

/// What the last item of a sequence was, for "nothing to repeat" / "multiple repeat".
#[derive(Clone, Copy, PartialEq, Eq)]
enum Last {
    Anchor,
    Repeat,
    Other,
}

/// Result of an escape inside a class.
enum ClassItem {
    Lit(u32),
    Set(CharSet),
}

struct Parser<'a> {
    pat: &'a str,
    pos: usize,
    flags: Flags,
    /// Per capture group: closed yet?
    closed: Vec<bool>,
    names: Vec<(String, u32)>,
    captures: Vec<Option<String>>,
    /// Numeric conditional references, validated once all groups are known.
    cond_refs: Vec<(u32, Span)>,
    global_u: bool,
    depth: u32,
    /// The first construct we cannot analyse. Parsing continues so that a later syntax error
    /// still wins (the engine would reject the pattern); reported at the end otherwise.
    deferred: Option<Error>,
}

pub fn parse(pattern: &str) -> Result<Regex> {
    let mut p = Parser {
        pat: pattern,
        pos: 0,
        flags: Flags::default(),
        closed: Vec::new(),
        names: Vec::new(),
        captures: Vec::new(),
        cond_refs: Vec::new(),
        global_u: false,
        depth: 0,
        deferred: None,
    };
    let root = p.parse_sub(0)?;
    if p.peek() == Some(')') {
        return Err(Error::syntax(p.span_here(), "unbalanced parenthesis"));
    }
    let groups = p.closed.len() as u32;
    for &(g, span) in &p.cond_refs {
        if g > groups {
            return Err(Error::syntax(span, format!("invalid group reference {g}")));
        }
    }
    if p.flags.ascii && p.global_u {
        return Err(Error::syntax(
            Span::new(0, pattern.len()),
            "ASCII and UNICODE flags are incompatible",
        ));
    }
    check_lookbehind_widths(&root)?;
    if let Some(e) = p.deferred {
        return Err(e);
    }
    Ok(Regex {
        flavor: Flavor::Python,
        pattern: pattern.to_string(),
        root,
        captures: p.captures,
        domain: crate::ir::Domain::Scalars,
        quirks: crate::ir::Quirks::default(),
    })
}

/// `_compiler.py`: "look-behind requires fixed-width pattern".
fn check_lookbehind_widths(root: &Node) -> Result<()> {
    let mut err = None;
    root.walk(&mut |n| {
        if let NodeKind::Look(l) = &n.kind
            && l.behind
            && err.is_none()
        {
            let (lo, hi) = l.sub.width();
            if hi != Some(lo) {
                err = Some(Error::syntax(
                    n.span,
                    "look-behind requires fixed-width pattern",
                ));
            }
        }
    });
    err.map_or(Ok(()), Err)
}

impl<'a> Parser<'a> {
    // ------------------------------------------------------------------ tokenizer

    fn peek(&self) -> Option<char> {
        self.pat[self.pos..].chars().next()
    }

    fn span_here(&self) -> Span {
        let len = self.peek().map_or(0, char::len_utf8);
        Span::new(self.pos, self.pos + len)
    }

    fn span_from(&self, start: usize) -> Span {
        Span::new(start, self.pos)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    /// `source.get()`.
    fn get(&mut self) -> Result<Option<Tok>> {
        let start = self.pos;
        match self.bump() {
            None => Ok(None),
            Some('\\') => match self.bump() {
                Some(c) => Ok(Some(Tok::Esc(c))),
                None => Err(Error::syntax(
                    self.span_from(start),
                    "bad escape (end of pattern)",
                )),
            },
            Some(c) => Ok(Some(Tok::Char(c))),
        }
    }

    /// `source.match(c)`: consume the next token if it is the plain char `c`.
    fn eat(&mut self, c: char) -> bool {
        if c != '\\' && self.peek() == Some(c) {
            self.pos += c.len_utf8();
            true
        } else {
            false
        }
    }

    /// `source.getwhile(n, charset)`.
    fn get_while(&mut self, n: usize, pred: impl Fn(char) -> bool) -> String {
        let mut out = String::new();
        while out.chars().count() < n {
            match self.peek() {
                Some(c) if pred(c) => {
                    out.push(c);
                    self.bump();
                }
                _ => break,
            }
        }
        out
    }

    /// `source.getuntil(terminator, name)`.
    fn get_until(&mut self, term: char, what: &str, start: usize) -> Result<String> {
        let mut out = String::new();
        loop {
            match self.bump() {
                None => {
                    return Err(Error::syntax(
                        self.span_from(start),
                        format!("missing {term}, unterminated name"),
                    ));
                }
                Some(c) if c == term => {
                    if out.is_empty() {
                        return Err(Error::syntax(
                            self.span_from(start),
                            format!("missing {what}"),
                        ));
                    }
                    return Ok(out);
                }
                Some(c) => out.push(c),
            }
        }
    }

    fn enter(&mut self, span: Span) -> Result<()> {
        self.depth += 1;
        if self.depth > NEST_LIMIT {
            return Err(Error::unsupported(
                span,
                format!(
                    "nesting deeper than {NEST_LIMIT} levels (CPython's recursion limit is not modelled)"
                ),
            ));
        }
        Ok(())
    }

    // ------------------------------------------------------------------ structure

    /// `_parse_sub`: alternatives.
    fn parse_sub(&mut self, nested: u32) -> Result<Node> {
        let start = self.pos;
        let mut items = Vec::new();
        loop {
            let first = nested == 0 && items.is_empty();
            items.push(self.parse_seq(nested, first)?);
            if !self.eat('|') {
                break;
            }
        }
        Ok(if items.len() == 1 {
            items
                .pop()
                .unwrap_or(Node::new(NodeKind::Empty, self.span_from(start)))
        } else {
            Node::new(NodeKind::Alt(items), self.span_from(start))
        })
    }

    /// `_parse`: one sequence, stopping before `|` or `)`.
    fn parse_seq(&mut self, nested: u32, first: bool) -> Result<Node> {
        let start = self.pos;
        let mut items: Vec<(Node, Last)> = Vec::new();
        loop {
            match self.peek() {
                None | Some('|') | Some(')') => break,
                _ => {}
            }
            let tok_start = self.pos;
            let Some(tok) = self.get()? else { break };
            if self.flags.x {
                match tok {
                    Tok::Char(' ' | '\t' | '\n' | '\r' | '\x0B' | '\x0C') => continue,
                    Tok::Char('#') => {
                        // CPython skips the comment token by token: `\` still forms an escape
                        // (so `\` + newline does not end the comment, and a trailing `\` is an
                        // error).
                        loop {
                            match self.get()? {
                                None | Some(Tok::Char('\n')) => break,
                                Some(_) => {}
                            }
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            match tok {
                Tok::Esc(c) => {
                    let (node, last) = self.escape(c, tok_start)?;
                    items.push((node, last));
                }
                Tok::Char('[') => {
                    let set = self.parse_class(tok_start)?;
                    items.push((
                        Node::new(NodeKind::Class(set), self.span_from(tok_start)),
                        Last::Other,
                    ));
                }
                Tok::Char(c @ ('*' | '+' | '?' | '{')) => {
                    let here = self.pos;
                    let (min, max) = match c {
                        '?' => (0, Some(1)),
                        '*' => (0, None),
                        '+' => (1, None),
                        _ => match self.parse_brace(tok_start)? {
                            Some(b) => b,
                            None => {
                                // Not a quantifier: a literal `{`, re-read what follows.
                                self.pos = here;
                                let node = self.literal('{' as u32, self.span_from(tok_start));
                                items.push((node, Last::Other));
                                continue;
                            }
                        },
                    };
                    let Some((item, last)) = items.pop() else {
                        return Err(Error::syntax(
                            self.span_from(tok_start),
                            "nothing to repeat",
                        ));
                    };
                    match last {
                        Last::Anchor => {
                            return Err(Error::syntax(
                                self.span_from(tok_start),
                                "nothing to repeat",
                            ));
                        }
                        Last::Repeat => {
                            return Err(Error::syntax(
                                self.span_from(tok_start),
                                "multiple repeat",
                            ));
                        }
                        Last::Other => {}
                    }
                    let (greedy, possessive) = if self.eat('?') {
                        (false, false)
                    } else if self.eat('+') {
                        (true, true)
                    } else {
                        (true, false)
                    };
                    let span = item.span.to(self.span_from(tok_start));
                    let node = Node::new(
                        NodeKind::Repeat(Repeat {
                            sub: Box::new(item),
                            min,
                            max,
                            greedy,
                            possessive,
                        }),
                        span,
                    );
                    items.push((node, Last::Repeat));
                }
                Tok::Char('.') => {
                    let set = if self.flags.s {
                        CharSet::full()
                    } else {
                        CharSet::full().difference(&CharSet::single('\n'))
                    };
                    items.push((
                        Node::new(NodeKind::Class(set), self.span_from(tok_start)),
                        Last::Other,
                    ));
                }
                Tok::Char('(') => {
                    if let Some(node) =
                        self.parse_group(tok_start, nested, first && items.is_empty())?
                    {
                        items.push((node, Last::Other));
                    }
                }
                Tok::Char('^') => {
                    let a = if self.flags.m {
                        Assertion::LineStart {
                            terms: LineTerms::Lf,
                        }
                    } else {
                        Assertion::TextStart
                    };
                    items.push((
                        Node::new(NodeKind::Assert(a), self.span_from(tok_start)),
                        Last::Anchor,
                    ));
                }
                Tok::Char('$') => {
                    let a = if self.flags.m {
                        Assertion::LineEnd {
                            terms: LineTerms::Lf,
                        }
                    } else {
                        Assertion::TextEndOrFinalNewline
                    };
                    items.push((
                        Node::new(NodeKind::Assert(a), self.span_from(tok_start)),
                        Last::Anchor,
                    ));
                }
                Tok::Char(c) => {
                    let node = self.literal(c as u32, self.span_from(tok_start));
                    items.push((node, Last::Other));
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

    /// After `{`: `Some((min, max))` for a valid quantifier, `None` for a literal `{`.
    fn parse_brace(&mut self, tok_start: usize) -> Result<Option<(u32, Option<u32>)>> {
        if self.peek() == Some('}') {
            return Ok(None);
        }
        let lo = self.get_while(usize::MAX, |c| c.is_ascii_digit());
        let hi = if self.eat(',') {
            Some(self.get_while(usize::MAX, |c| c.is_ascii_digit()))
        } else {
            None
        };
        if !self.eat('}') {
            return Ok(None);
        }
        let too_big = || {
            Error::syntax(
                Span::new(tok_start, tok_start + 1),
                "the repetition number is too large",
            )
        };
        let num = |s: &str| -> Result<u32> {
            let v: u64 = s.parse().map_err(|_| too_big())?;
            if v >= MAXREPEAT {
                return Err(too_big());
            }
            Ok(v as u32)
        };
        let min = if lo.is_empty() { 0 } else { num(&lo)? };
        let max = match &hi {
            None => Some(min),
            Some(h) if h.is_empty() => None,
            Some(h) => Some(num(h)?),
        };
        if let Some(m) = max
            && m < min
        {
            return Err(Error::syntax(
                self.span_from(tok_start),
                "min repeat greater than max repeat",
            ));
        }
        Ok(Some((min, max)))
    }

    fn literal(&self, c: u32, span: Span) -> Node {
        let Some(ch) = char::from_u32(c) else {
            // A lone surrogate: never matches a string of scalar values.
            return Node::new(NodeKind::Class(CharSet::empty()), span);
        };
        if !self.flags.i {
            return Node::new(NodeKind::Literal(ch), span);
        }
        let set = self.fold(CharSet::single(ch));
        if set.len() == 1 {
            Node::new(NodeKind::Literal(ch), span)
        } else {
            Node::new(NodeKind::Class(set), span)
        }
    }

    fn backref_case(&self) -> crate::ir::BackrefCase {
        if self.flags.i {
            crate::ir::BackrefCase::Python {
                ascii: self.flags.ascii,
            }
        } else {
            crate::ir::BackrefCase::Exact
        }
    }

    fn fold(&self, set: CharSet) -> CharSet {
        if !self.flags.i {
            set
        } else if self.flags.ascii {
            unicode::case_fold_ascii(&set)
        } else {
            tables::fold_with(&set, t::FOLD)
        }
    }

    fn category(&self, c: char) -> Option<CharSet> {
        let ascii = self.flags.ascii;
        let base = match c.to_ascii_lowercase() {
            'd' if ascii => unicode::posix_class("digit")?,
            'w' if ascii => unicode::posix_class("word")?,
            's' if ascii => CharSet::from_iter([' ', '\t', '\n', '\r', '\x0B', '\x0C']),
            'd' => DIGIT.get().clone(),
            'w' => WORD.get().clone(),
            's' => SPACE.get().clone(),
            _ => return None,
        };
        Some(if c.is_ascii_uppercase() {
            base.complement()
        } else {
            base
        })
    }

    /// Shared `\x \u \U \N` handling; `None` if `c` is not one of them.
    fn hex_escape(&mut self, c: char, start: usize) -> Option<Result<u32>> {
        let n = match c {
            'x' => 2,
            'u' => 4,
            'U' => 8,
            'N' => {
                return Some((|| {
                    if !self.eat('{') {
                        return Err(Error::syntax(self.span_from(start), "missing {"));
                    }
                    self.get_until('}', "character name", start)?;
                    if self.deferred.is_none() {
                        self.deferred = Some(Error::unsupported(
                            self.span_from(start),
                            "named Unicode escapes \\N{...} need the Unicode name database (not bundled yet)",
                        ));
                    }
                    // Placeholder: a code point that never occurs in scalar-value strings.
                    Ok(0xD800)
                })());
            }
            _ => return None,
        };
        let digits = self.get_while(n, |c| c.is_ascii_hexdigit());
        if digits.chars().count() != n {
            return Some(Err(Error::syntax(
                self.span_from(start),
                format!("incomplete escape \\{c}{digits}"),
            )));
        }
        let v = u32::from_str_radix(&digits, 16).unwrap_or(u32::MAX);
        if c == 'U' && v > 0x10_FFFF {
            return Some(Err(Error::syntax(
                self.span_from(start),
                format!("bad escape \\U{digits}"),
            )));
        }
        Some(Ok(v))
    }

    /// `_escape`: an escape outside a class.
    fn escape(&mut self, c: char, start: usize) -> Result<(Node, Last)> {
        let span = |p: &Self| p.span_from(start);
        let assert = |p: &Self, a| Ok((Node::new(NodeKind::Assert(a), span(p)), Last::Anchor));
        let unicode = !self.flags.ascii;
        match c {
            'A' => return assert(self, Assertion::TextStart),
            'Z' | 'z' => return assert(self, Assertion::TextEnd),
            'b' => {
                return assert(
                    self,
                    Assertion::WordBoundary {
                        word: WordSet::python(unicode),
                    },
                );
            }
            'B' => {
                return assert(
                    self,
                    Assertion::NotWordBoundary {
                        word: WordSet::python(unicode),
                    },
                );
            }
            'd' | 'D' | 's' | 'S' | 'w' | 'W' => {
                let set = self.category(c).unwrap_or_default();
                return Ok((Node::new(NodeKind::Class(set), span(self)), Last::Other));
            }
            _ => {}
        }
        let lit = |p: &Self, v: u32| Ok((p.literal(v, span(p)), Last::Other));
        if let Some(v) = simple_escape(c, false) {
            return lit(self, v);
        }
        if let Some(r) = self.hex_escape(c, start) {
            let v = r?;
            return lit(self, v);
        }
        if c == '0' {
            let digits = self.get_while(2, |c| ('0'..='7').contains(&c));
            return lit(
                self,
                u32::from_str_radix(&format!("0{digits}"), 8).unwrap_or(0),
            );
        }
        if let Some(d) = c.to_digit(10) {
            let mut text = d.to_string();
            if let Some(n) = self.peek().filter(char::is_ascii_digit) {
                self.bump();
                text.push(n);
                let octal = |ch: char| ('0'..='7').contains(&ch);
                if octal(c)
                    && octal(n)
                    && let Some(m) = self.peek().filter(|&m| octal(m))
                {
                    self.bump();
                    text.push(m);
                    let v = u32::from_str_radix(&text, 8).unwrap_or(0);
                    if v > 0o377 {
                        return Err(Error::syntax(
                            span(self),
                            format!("octal escape value \\{text} outside of range 0-0o377"),
                        ));
                    }
                    return lit(self, v);
                }
            }
            let group: u32 = text.parse().unwrap_or(0);
            if group >= 1 && (group as usize) <= self.closed.len() {
                if !self.closed[group as usize - 1] {
                    return Err(Error::syntax(span(self), "cannot refer to an open group"));
                }
                return Ok((
                    Node::new(NodeKind::Backref(group, self.backref_case()), span(self)),
                    Last::Other,
                ));
            }
            return Err(Error::syntax(
                span(self),
                format!("invalid group reference {group}"),
            ));
        }
        if c.is_ascii_alphabetic() {
            return Err(Error::syntax(span(self), format!("bad escape \\{c}")));
        }
        lit(self, c as u32)
    }

    /// `_class_escape`.
    fn class_escape(&mut self, c: char, start: usize) -> Result<ClassItem> {
        if let Some(v) = simple_escape(c, true) {
            return Ok(ClassItem::Lit(v));
        }
        if let Some(set) = self.category(c) {
            return Ok(ClassItem::Set(set));
        }
        if let Some(r) = self.hex_escape(c, start) {
            return r.map(ClassItem::Lit);
        }
        if ('0'..='7').contains(&c) {
            let digits = self.get_while(2, |c| ('0'..='7').contains(&c));
            let text = format!("{c}{digits}");
            let v = u32::from_str_radix(&text, 8).unwrap_or(0);
            if v > 0o377 {
                return Err(Error::syntax(
                    self.span_from(start),
                    format!("octal escape value \\{text} outside of range 0-0o377"),
                ));
            }
            return Ok(ClassItem::Lit(v));
        }
        if c.is_ascii_digit() || c.is_ascii_alphabetic() {
            return Err(Error::syntax(
                self.span_from(start),
                format!("bad escape \\{c}"),
            ));
        }
        Ok(ClassItem::Lit(c as u32))
    }

    /// After `[`.
    fn parse_class(&mut self, open: usize) -> Result<CharSet> {
        let negate = self.eat('^');
        let mut set = CharSet::empty();
        let mut any = false;
        let unterminated =
            |p: &Self| Error::syntax(p.span_from(open), "unterminated character set");
        loop {
            let item_start = self.pos;
            let Some(tok) = self.get()? else {
                return Err(unterminated(self));
            };
            if tok == Tok::Char(']') && any {
                break;
            }
            any = true;
            let code1 = match tok {
                Tok::Esc(c) => self.class_escape(c, item_start)?,
                Tok::Char(c) => ClassItem::Lit(c as u32),
            };
            if self.eat('-') {
                let that_start = self.pos;
                let Some(that) = self.get()? else {
                    return Err(unterminated(self));
                };
                if that == Tok::Char(']') {
                    set = set.union(&item_set(&code1)).union(&CharSet::single('-'));
                    break;
                }
                let code2 = match that {
                    Tok::Esc(c) => self.class_escape(c, that_start)?,
                    Tok::Char(c) => ClassItem::Lit(c as u32),
                };
                let (ClassItem::Lit(lo), ClassItem::Lit(hi)) = (&code1, &code2) else {
                    return Err(Error::syntax(
                        self.span_from(item_start),
                        "bad character range",
                    ));
                };
                if hi < lo {
                    return Err(Error::syntax(
                        self.span_from(item_start),
                        "bad character range",
                    ));
                }
                set = set.union(&CharSet::from_ranges([(*lo, *hi)]));
            } else {
                set = set.union(&item_set(&code1));
            }
        }
        let set = self.fold(set);
        Ok(if negate { set.complement() } else { set })
    }

    /// After `(`. Returns `None` for comments and global flag groups.
    fn parse_group(&mut self, start: usize, nested: u32, at_start: bool) -> Result<Option<Node>> {
        let open = Span::new(start, start + 1);
        let mut capture = true;
        let mut atomic = false;
        let mut name: Option<String> = None;
        let mut scoped: Option<Flags> = None;
        if self.eat('?') {
            let Some(c) = self.bump() else {
                return Err(Error::syntax(
                    self.span_from(start),
                    "unexpected end of pattern",
                ));
            };
            match c {
                'P' => {
                    if self.eat('<') {
                        let n = self.get_until('>', "group name", start)?;
                        if !is_identifier(&n) {
                            return Err(Error::syntax(
                                self.span_from(start),
                                format!("bad character in group name '{n}'"),
                            ));
                        }
                        name = Some(n);
                    } else if self.eat('=') {
                        let n = self.get_until(')', "group name", start)?;
                        if !is_identifier(&n) {
                            return Err(Error::syntax(
                                self.span_from(start),
                                format!("bad character in group name '{n}'"),
                            ));
                        }
                        let Some(&(_, gid)) = self.names.iter().find(|(m, _)| *m == n) else {
                            return Err(Error::syntax(
                                self.span_from(start),
                                format!("unknown group name '{n}'"),
                            ));
                        };
                        if !self.closed[gid as usize - 1] {
                            return Err(Error::syntax(
                                self.span_from(start),
                                "cannot refer to an open group",
                            ));
                        }
                        return Ok(Some(Node::new(
                            NodeKind::Backref(gid, self.backref_case()),
                            self.span_from(start),
                        )));
                    } else {
                        let next = self.bump();
                        return Err(Error::syntax(
                            self.span_from(start),
                            match next {
                                None => "unexpected end of pattern".to_string(),
                                Some(n) => format!("unknown extension ?P{n}"),
                            },
                        ));
                    }
                }
                ':' => capture = false,
                '#' => {
                    loop {
                        match self.bump() {
                            None => {
                                return Err(Error::syntax(
                                    self.span_from(start),
                                    "missing ), unterminated comment",
                                ));
                            }
                            Some(')') => break,
                            Some(_) => {}
                        }
                    }
                    return Ok(None);
                }
                '=' | '!' | '<' => {
                    let (behind, negated) = if c == '<' {
                        match self.bump() {
                            Some('=') => (true, false),
                            Some('!') => (true, true),
                            None => {
                                return Err(Error::syntax(
                                    self.span_from(start),
                                    "unexpected end of pattern",
                                ));
                            }
                            Some(n) => {
                                return Err(Error::syntax(
                                    self.span_from(start),
                                    format!("unknown extension ?<{n}"),
                                ));
                            }
                        }
                    } else {
                        (false, c == '!')
                    };
                    self.enter(open)?;
                    let sub = self.parse_sub(nested + 1);
                    self.depth -= 1;
                    let sub = sub?;
                    if !self.eat(')') {
                        return Err(Error::syntax(open, "missing ), unterminated subpattern"));
                    }
                    return Ok(Some(Node::new(
                        NodeKind::Look(Look {
                            sub: Box::new(sub),
                            behind,
                            negated,
                            window: None,
                            window_empty: false,
                        }),
                        self.span_from(start),
                    )));
                }
                '(' => return self.parse_conditional(start, nested).map(Some),
                '>' => {
                    capture = false;
                    atomic = true;
                }
                c if "aiLmsux-".contains(c) => match self.parse_flags(c, start)? {
                    None => {
                        if !at_start {
                            return Err(Error::syntax(
                                self.span_from(start),
                                "global flags not at the start of the expression",
                            ));
                        }
                        return Ok(None);
                    }
                    Some(f) => {
                        capture = false;
                        scoped = Some(f);
                    }
                },
                c => {
                    return Err(Error::syntax(
                        self.span_from(start),
                        format!("unknown extension ?{c}"),
                    ));
                }
            }
        }
        let cap = if capture {
            if let Some(n) = &name
                && let Some(&(_, old)) = self.names.iter().find(|(m, _)| m == n)
            {
                return Err(Error::syntax(
                    self.span_from(start),
                    format!(
                        "redefinition of group name '{n}' as group {}; was group {old}",
                        self.closed.len() + 1
                    ),
                ));
            }
            self.closed.push(false);
            let index = self.closed.len() as u32;
            if let Some(n) = &name {
                self.names.push((n.clone(), index));
            }
            self.captures.push(name.clone());
            Some(Capture { index, name })
        } else {
            None
        };
        self.enter(open)?;
        let saved = self.flags;
        if let Some(f) = scoped {
            self.flags = f;
        }
        let sub = self.parse_sub(nested + 1);
        self.flags = saved;
        self.depth -= 1;
        let sub = sub?;
        if !self.eat(')') {
            return Err(Error::syntax(open, "missing ), unterminated subpattern"));
        }
        if let Some(c) = &cap {
            self.closed[c.index as usize - 1] = true;
        }
        Ok(Some(Node::new(
            NodeKind::Group(Group {
                sub: Box::new(sub),
                capture: cap,
                atomic,
            }),
            self.span_from(start),
        )))
    }

    /// After `(?(`.
    fn parse_conditional(&mut self, start: usize, nested: u32) -> Result<Node> {
        let name = self.get_until(')', "group name", start)?;
        let span = self.span_from(start);
        let group = if is_identifier(&name) {
            match self.names.iter().find(|(m, _)| *m == name) {
                Some(&(_, g)) => g,
                None => return Err(Error::syntax(span, format!("unknown group name '{name}'"))),
            }
        } else {
            if !name.chars().all(|c| c.is_ascii_digit()) {
                return Err(Error::syntax(
                    span,
                    format!("bad character in group name '{name}'"),
                ));
            }
            let g: u64 = name.parse().unwrap_or(u64::MAX);
            if g == 0 {
                return Err(Error::syntax(span, "bad group number"));
            }
            let g = u32::try_from(g).map_err(|_| Error::syntax(span, "invalid group reference"))?;
            self.cond_refs.push((g, span));
            g
        };
        self.enter(Span::new(start, start + 1))?;
        let yes = self.parse_seq(nested + 1, false);
        let result = yes.and_then(|yes| {
            let no = if self.eat('|') {
                let no = self.parse_seq(nested + 1, false)?;
                if self.peek() == Some('|') {
                    return Err(Error::syntax(
                        self.span_here(),
                        "conditional backref with more than two branches",
                    ));
                }
                Some(Box::new(no))
            } else {
                None
            };
            Ok((yes, no))
        });
        self.depth -= 1;
        let (yes, no) = result?;
        if !self.eat(')') {
            return Err(Error::syntax(
                Span::new(start, start + 1),
                "missing ), unterminated subpattern",
            ));
        }
        Ok(Node::new(
            NodeKind::Conditional(Conditional {
                group,
                yes: Box::new(yes),
                no,
            }),
            self.span_from(start),
        ))
    }

    /// `_parse_flags`. `Ok(None)` for a global flag group `(?flags)`, else the scoped flags.
    fn parse_flags(&mut self, first: char, start: usize) -> Result<Option<Flags>> {
        let err = |p: &Self, m: &str| Error::syntax(p.span_from(start), m.to_string());
        let mut add = Flags::default();
        let (mut add_any, mut add_type) = (false, None::<char>);
        let mut add_set: Vec<char> = Vec::new();
        let mut c = first;
        if c != '-' {
            loop {
                if c == 'L' {
                    return Err(err(
                        self,
                        "bad inline flags: cannot use 'L' flag with a str pattern",
                    ));
                }
                if c == 'a' || c == 'u' {
                    if add_type.is_some_and(|t| t != c) {
                        return Err(err(
                            self,
                            "bad inline flags: flags 'a', 'u' and 'L' are incompatible",
                        ));
                    }
                    add_type = Some(c);
                }
                add_set.push(c);
                add_any = true;
                match c {
                    'i' => add.i = true,
                    'm' => add.m = true,
                    's' => add.s = true,
                    'x' => add.x = true,
                    'a' => add.ascii = true,
                    _ => {}
                }
                match self.bump() {
                    None => return Err(err(self, "missing -, : or )")),
                    Some(n) if ")-:".contains(n) => {
                        c = n;
                        break;
                    }
                    Some(n) if "aiLmsux".contains(n) => c = n,
                    Some(n) => {
                        return Err(err(
                            self,
                            if n.is_alphabetic() {
                                "unknown flag"
                            } else {
                                "missing -, : or )"
                            },
                        ));
                    }
                }
            }
        }
        let _ = add_any;
        if c == ')' {
            // Global: applies to the whole pattern (it can only appear at the start).
            self.flags.i |= add.i;
            self.flags.m |= add.m;
            self.flags.s |= add.s;
            self.flags.x |= add.x;
            self.flags.ascii |= add.ascii;
            self.global_u |= add_type == Some('u');
            return Ok(None);
        }
        let mut del: Vec<char> = Vec::new();
        if c == '-' {
            let mut d = match self.bump() {
                None => return Err(err(self, "missing flag")),
                Some(d) if "aiLmsux".contains(d) => d,
                Some(d) => {
                    return Err(err(
                        self,
                        if d.is_alphabetic() {
                            "unknown flag"
                        } else {
                            "missing flag"
                        },
                    ));
                }
            };
            loop {
                if "aLu".contains(d) {
                    return Err(err(
                        self,
                        "bad inline flags: cannot turn off flags 'a', 'u' and 'L'",
                    ));
                }
                del.push(d);
                match self.bump() {
                    None => return Err(err(self, "missing :")),
                    Some(':') => break,
                    Some(n) if "aiLmsux".contains(n) => d = n,
                    Some(n) => {
                        return Err(err(
                            self,
                            if n.is_alphabetic() {
                                "unknown flag"
                            } else {
                                "missing :"
                            },
                        ));
                    }
                }
            }
        }
        if add_set.iter().any(|f| del.contains(f)) {
            return Err(err(self, "bad inline flags: flag turned on and off"));
        }
        let mut f = self.flags;
        f.i = (f.i || add.i) && !del.contains(&'i');
        f.m = (f.m || add.m) && !del.contains(&'m');
        f.s = (f.s || add.s) && !del.contains(&'s');
        f.x = (f.x || add.x) && !del.contains(&'x');
        f.ascii = f.ascii || add.ascii;
        if add_type == Some('u') {
            f.ascii = false;
        }
        Ok(Some(f))
    }
}

fn item_set(item: &ClassItem) -> CharSet {
    match item {
        ClassItem::Lit(v) => CharSet::from_ranges([(*v, *v)]),
        ClassItem::Set(s) => s.clone(),
    }
}

/// `ESCAPES`: `\a \f \n \r \t \v \\`, plus `\b` = backspace inside classes.
fn simple_escape(c: char, in_class: bool) -> Option<u32> {
    Some(match c {
        'a' => 7,
        'b' if in_class => 8,
        'f' => 0x0C,
        'n' => 0x0A,
        'r' => 0x0D,
        't' => 0x09,
        'v' => 0x0B,
        '\\' => 0x5C,
        _ => return None,
    })
}

/// `str.isidentifier()`, from tables measured on the engine.
fn is_identifier(s: &str) -> bool {
    let mut it = s.chars();
    match it.next() {
        Some(c) if IDENT_START.get().contains(c) => it.all(|c| IDENT_CONTINUE.get().contains(c)),
        _ => false,
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
    fn python_specific_syntax() {
        assert_eq!(ok("a{"), "(cat 'a' '{')");
        assert_eq!(ok("a{}"), "(cat 'a' '{' '}')");
        assert_eq!(ok("a{,3}"), "(rep{0,3} 'a')");
        assert_eq!(ok("a{,}"), "(rep{0,} 'a')");
        assert_eq!(ok("a*+"), "(rep{0,}+ 'a')");
        assert_eq!(ok("a$"), "(cat 'a' TextEndOrFinalNewline)");
        assert_eq!(ok("[a&&b]"), "[&a-b]");
        assert_eq!(ok("[]-a]"), "[\\]-a]");
        assert_eq!(ok("a(?#x)*"), "(rep{0,} 'a')");
        assert_eq!(ok(r"(a)\1"), r"(cat (cap1 'a') \1)");
        assert_eq!(ok(r"\08"), "(cat '\\0' '8')");
        for p in [
            "a**",
            "x*{2}",
            "^*",
            r"\b*",
            "[]",
            "(?#x)*",
            "a(?i)",
            r"\1(a)",
            r"(a\1)",
            "[z-a]",
            "(?<=a|bc)d",
            "(?L)a",
            "(?-a:a)",
            r"\e",
        ] {
            syntax(p);
        }
    }

    #[test]
    fn unicode_tables_are_pythons() {
        // Python's \w is str.isalnum() based; U+00B2 SUPERSCRIPT TWO is alnum in Python.
        let r = parse(r"\w").unwrap();
        let NodeKind::Class(s) = r.root.kind else {
            panic!()
        };
        assert!(s.contains('é') && s.contains('_'));
        let r = parse(r"(?a)\w").unwrap();
        let NodeKind::Class(s) = r.root.kind else {
            panic!()
        };
        assert!(!s.contains('é'));
    }
}
