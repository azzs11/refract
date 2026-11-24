//! Front-end for the Rust `regex` crate flavour. Spec: `spec/flavours/rust.md`.
//!
//! A hand-written recursive-descent parser that produces [`crate::ir`] directly, resolving flags
//! as it goes. It never panics on any input; every error carries the span of the offending text.

use crate::charset::CharSet;
use crate::error::{Error, Result, Span};
use crate::flavor::{Flavor, MatchPolicy, Profile};
use crate::ir::{Assertion, Capture, Group, LineTerms, Node, NodeKind, Regex, Repeat, WordSet};
use crate::unicode;

pub static PROFILE: Profile = Profile {
    flavor: Flavor::Rust,
    engine: "regex 1.x (regex-syntax 0.8)",
    unicode: unicode::UNICODE_SOURCE,
    dot_excludes: "\\n",
    dollar: "end of haystack only (\\z)",
    unicode_classes: true,
    case_folding: "Unicode simple case folding",
    match_policy: MatchPolicy::LeftmostFirst,
    lookaround: false,
    backreferences: false,
    linear_time: true,
    spec: "spec/flavours/rust.md",
};

/// Same default as `regex-syntax`.
const NEST_LIMIT: u32 = 250;

#[derive(Clone, Copy, Debug)]
struct Flags {
    i: bool,
    m: bool,
    s: bool,
    swap_greed: bool,
    u: bool,
    x: bool,
    crlf: bool,
}

impl Default for Flags {
    fn default() -> Self {
        Flags {
            i: false,
            m: false,
            s: false,
            swap_greed: false,
            u: true,
            x: false,
            crlf: false,
        }
    }
}

/// Result of parsing one escape sequence.
enum Escape {
    /// A literal character; `hex_x` is true for `\xNN` / `\x{..}` (which mean bytes under `-u`).
    Lit {
        c: char,
        hex_x: bool,
    },
    /// A Perl class (`\d` etc.): not case-folded on its own.
    Perl(CharSet),
    /// A Unicode property class, already folded and negated as required.
    Unicode(CharSet),
    Assert(Assertion),
}

struct Parser<'a> {
    pat: &'a str,
    pos: usize,
    flags: Flags,
    captures: Vec<Option<String>>,
    depth: u32,
}

/// Parses a Rust-flavour pattern with default options.
pub fn parse(pattern: &str) -> Result<Regex> {
    let mut p = Parser {
        pat: pattern,
        pos: 0,
        flags: Flags::default(),
        captures: Vec::new(),
        depth: 0,
    };
    let root = p.parse_alternation()?;
    if let Some(c) = p.peek() {
        // The only way to stop early at top level is an unmatched `)`.
        debug_assert_eq!(c, ')');
        return Err(Error::syntax(
            p.span_here(1),
            "unopened group: `)` has no matching `(`",
        ));
    }
    Ok(Regex {
        flavor: Flavor::Rust,
        pattern: pattern.to_string(),
        root,
        captures: p.captures,
        domain: crate::ir::Domain::Scalars,
        quirks: crate::ir::Quirks::default(),
    })
}

impl<'a> Parser<'a> {
    // ---------------------------------------------------------------- cursor helpers

    fn peek(&self) -> Option<char> {
        self.pat[self.pos..].chars().next()
    }

    fn peek_nth(&self, n: usize) -> Option<char> {
        self.pat[self.pos..].chars().nth(n)
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

    fn span_from(&self, start: usize) -> Span {
        Span::new(start, self.pos)
    }

    /// Span of the next `n` characters (clamped to the end).
    fn span_here(&self, n: usize) -> Span {
        let end = self.pat[self.pos..]
            .char_indices()
            .nth(n)
            .map_or(self.pat.len(), |(i, _)| self.pos + i);
        Span::new(self.pos, end)
    }

    /// In `x` mode, skips whitespace and `#` comments.
    fn skip_ws(&mut self) {
        if !self.flags.x {
            return;
        }
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('#') => {
                    while let Some(c) = self.bump() {
                        if c == '\n' {
                            break;
                        }
                    }
                }
                _ => return,
            }
        }
    }

    /// Next character after skipping `x`-mode whitespace, without consuming anything.
    fn peek_after_ws(&self) -> Option<char> {
        if !self.flags.x {
            return self.peek();
        }
        let mut it = self.pat[self.pos..].chars().peekable();
        loop {
            match it.peek().copied() {
                Some(c) if c.is_whitespace() => {
                    it.next();
                }
                Some('#') => {
                    for c in it.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                }
                other => return other,
            }
        }
    }

    fn enter(&mut self, span: Span) -> Result<()> {
        self.depth += 1;
        if self.depth > NEST_LIMIT {
            return Err(Error::syntax(
                span,
                format!("pattern nests deeper than the limit of {NEST_LIMIT}"),
            ));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    // ---------------------------------------------------------------- structure

    fn parse_alternation(&mut self) -> Result<Node> {
        let start = self.pos;
        let mut alts = vec![self.parse_concat()?];
        while self.eat('|') {
            alts.push(self.parse_concat()?);
        }
        Ok(if alts.len() == 1 {
            alts.pop()
                .unwrap_or(Node::new(NodeKind::Empty, self.span_from(start)))
        } else {
            Node::new(NodeKind::Alt(alts), self.span_from(start))
        })
    }

    fn parse_concat(&mut self) -> Result<Node> {
        let start = self.pos;
        let mut items: Vec<Node> = Vec::new();
        // False at the start and right after a flag directive: a repetition there has nothing
        // to repeat.
        let mut can_repeat = false;
        loop {
            self.skip_ws();
            let Some(c) = self.peek() else { break };
            match c {
                '|' | ')' => break,
                '*' | '+' | '?' | '{' => {
                    let op_start = self.pos;
                    let Some(sub) = items.pop().filter(|_| can_repeat) else {
                        return Err(Error::syntax(
                            self.span_here(1),
                            "repetition operator missing expression",
                        ));
                    };
                    let node = self.parse_repetition(sub, op_start)?;
                    items.push(node);
                    can_repeat = true;
                }
                '(' => match self.parse_group()? {
                    Some(node) => {
                        items.push(node);
                        can_repeat = true;
                    }
                    None => can_repeat = false,
                },
                _ => {
                    items.push(self.parse_atom()?);
                    can_repeat = true;
                }
            }
        }
        let span = self.span_from(start);
        Ok(match items.len() {
            0 => Node::new(NodeKind::Empty, span),
            1 => items.pop().unwrap_or(Node::new(NodeKind::Empty, span)),
            _ => Node::new(NodeKind::Concat(items), span),
        })
    }

    fn parse_repetition(&mut self, sub: Node, op_start: usize) -> Result<Node> {
        let (min, max) = match self.bump() {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            Some('{') => self.parse_counted(op_start)?,
            _ => {
                return Err(Error::syntax(
                    self.span_from(op_start),
                    "expected repetition",
                ));
            }
        };
        let lazy = self.eat('?');
        let greedy = lazy == self.flags.swap_greed;
        let span = sub.span.to(self.span_from(op_start));
        Ok(Node::new(
            NodeKind::Repeat(Repeat {
                sub: Box::new(sub),
                min,
                max,
                greedy,
                possessive: false,
            }),
            span,
        ))
    }

    /// After `{`: `n}`, `n,}` or `n,m}`. Like `regex-syntax`, whitespace around the decimals is
    /// skipped even without `x` mode; `{,m}` is not accepted.
    fn parse_counted(&mut self, op_start: usize) -> Result<(u32, Option<u32>)> {
        let min = self.parse_decimal(op_start)?;
        let max = if self.eat(',') {
            self.skip_ws();
            if self.peek() == Some('}') {
                None
            } else {
                Some(self.parse_decimal(op_start)?)
            }
        } else {
            Some(min)
        };
        self.skip_ws();
        if !self.eat('}') {
            return Err(Error::syntax(
                self.span_from(op_start),
                "unclosed counted repetition",
            ));
        }
        if let Some(max) = max
            && min > max
        {
            return Err(Error::syntax(
                self.span_from(op_start),
                format!("invalid repetition range {{{min},{max}}}: min is greater than max"),
            ));
        }
        Ok((min, max))
    }

    /// A decimal with optional surrounding whitespace (always skipped, as in `regex-syntax`).
    fn parse_decimal(&mut self, op_start: usize) -> Result<u32> {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
        let start = self.pos;
        while matches!(self.peek(), Some('0'..='9')) {
            self.bump();
        }
        if start == self.pos {
            let msg = if self.peek().is_none() {
                "unclosed counted repetition"
            } else {
                "repetition quantifier expects a valid decimal"
            };
            return Err(Error::syntax(self.span_from(op_start), msg));
        }
        let digits = &self.pat[start..self.pos];
        let n = digits
            .parse::<u32>()
            .map_err(|_| Error::syntax(self.span_from(start), "repetition count is too large"))?;
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
        Ok(n)
    }

    /// Parses a group starting at `(`. Returns `None` for a flag directive like `(?i)`.
    fn parse_group(&mut self) -> Result<Option<Node>> {
        let start = self.pos;
        self.bump(); // (
        let open = self.span_from(start);
        let mut capture = None;
        let mut scoped_flags = None;
        if self.eat('?') {
            if self.peek() == Some('P') && self.peek_nth(1) == Some('<') {
                self.pos += 2;
                capture = Some(self.parse_capture_name(start)?);
            } else if self.peek() == Some('<') && !matches!(self.peek_nth(1), Some('=') | Some('!'))
            {
                self.pos += 1;
                capture = Some(self.parse_capture_name(start)?);
            } else {
                match self.peek() {
                    Some('=') | Some('!') => {
                        return Err(Error::syntax(
                            self.span_from(start),
                            "look-around is not supported by the Rust regex engine",
                        ));
                    }
                    Some('<') => {
                        return Err(Error::syntax(
                            self.span_from(start),
                            "look-behind is not supported by the Rust regex engine",
                        ));
                    }
                    _ => {}
                }
                let (flags, directive) = self.parse_flags(start)?;
                if directive {
                    // `(?flags)`: applies until the end of the enclosing group.
                    self.flags = flags;
                    return Ok(None);
                }
                scoped_flags = Some(flags);
            }
        } else {
            let index = self.captures.len() as u32 + 1;
            self.captures.push(None);
            capture = Some(Capture { index, name: None });
        }
        self.enter(open)?;
        let saved = self.flags;
        if let Some(f) = scoped_flags {
            self.flags = f;
        }
        let sub = self.parse_alternation();
        self.flags = saved;
        self.leave();
        let sub = sub?;
        if !self.eat(')') {
            return Err(Error::syntax(open, "unclosed group"));
        }
        Ok(Some(Node::new(
            NodeKind::Group(Group {
                sub: Box::new(sub),
                capture,
                atomic: false,
            }),
            self.span_from(start),
        )))
    }

    /// After `(?P<` or `(?<`: reads `name>` and registers the capture.
    fn parse_capture_name(&mut self, group_start: usize) -> Result<Capture> {
        let name_start = self.pos;
        loop {
            match self.peek() {
                None => {
                    return Err(Error::syntax(
                        self.span_from(group_start),
                        "unclosed capture group name",
                    ));
                }
                Some('>') => break,
                Some(c) => {
                    let first = self.pos == name_start;
                    let ok = c == '_'
                        || c.is_alphabetic()
                        || (!first && (c.is_alphanumeric() || matches!(c, '.' | '[' | ']')));
                    if !ok {
                        return Err(Error::syntax(
                            self.span_here(1),
                            "invalid character in capture group name",
                        ));
                    }
                    self.bump();
                }
            }
        }
        let name = &self.pat[name_start..self.pos];
        let name_span = self.span_from(name_start);
        self.bump(); // >
        if name.is_empty() {
            return Err(Error::syntax(name_span, "empty capture group name"));
        }
        if self.captures.iter().flatten().any(|n| n == name) {
            return Err(Error::syntax(
                name_span,
                format!("duplicate capture group name `{name}`"),
            ));
        }
        let index = self.captures.len() as u32 + 1;
        self.captures.push(Some(name.to_string()));
        Ok(Capture {
            index,
            name: Some(name.to_string()),
        })
    }

    /// After `(?`: reads flags up to `)` (directive, returns true) or `:` (scoped group).
    fn parse_flags(&mut self, group_start: usize) -> Result<(Flags, bool)> {
        let mut flags = self.flags;
        let mut seen: Vec<char> = Vec::new();
        let mut negated = false;
        let mut last_was_negation = false;
        let mut any = false;
        loop {
            let at = self.pos;
            let Some(c) = self.bump() else {
                return Err(Error::syntax(
                    self.span_from(group_start),
                    "unclosed group (expected flags followed by `)` or `:`)",
                ));
            };
            match c {
                ':' | ')' => {
                    if last_was_negation {
                        return Err(Error::syntax(
                            Span::new(at - 1, at),
                            "expected a flag after `-`",
                        ));
                    }
                    if c == ')' && !any && !negated {
                        return Err(Error::syntax(
                            self.span_from(group_start),
                            "empty flag group `(?)`",
                        ));
                    }
                    return Ok((flags, c == ')'));
                }
                '-' => {
                    if negated {
                        return Err(Error::syntax(
                            self.span_from(at),
                            "flag negation `-` may appear only once",
                        ));
                    }
                    negated = true;
                    last_was_negation = true;
                }
                'i' | 'm' | 's' | 'U' | 'u' | 'x' | 'R' => {
                    if seen.contains(&c) {
                        return Err(Error::syntax(
                            self.span_from(at),
                            format!("duplicate flag `{c}`"),
                        ));
                    }
                    seen.push(c);
                    let v = !negated;
                    match c {
                        'i' => flags.i = v,
                        'm' => flags.m = v,
                        's' => flags.s = v,
                        'U' => flags.swap_greed = v,
                        'u' => flags.u = v,
                        'x' => flags.x = v,
                        _ => flags.crlf = v,
                    }
                    any = true;
                    last_was_negation = false;
                }
                _ => {
                    return Err(Error::syntax(
                        self.span_from(at),
                        format!("unrecognized flag `{c}`"),
                    ));
                }
            }
        }
    }

    // ---------------------------------------------------------------- atoms

    fn parse_atom(&mut self) -> Result<Node> {
        let start = self.pos;
        let c = self.peek().unwrap_or('\0');
        match c {
            '.' => {
                self.bump();
                let span = self.span_from(start);
                let set = if self.flags.s {
                    self.universe()
                } else if self.flags.crlf {
                    self.universe()
                        .difference(&CharSet::from_iter(['\n', '\r']))
                } else {
                    self.universe().difference(&CharSet::single('\n'))
                };
                Ok(Node::new(
                    NodeKind::Class(self.check_utf8(set, span)?),
                    span,
                ))
            }
            '^' => {
                self.bump();
                let a = if self.flags.m {
                    Assertion::LineStart {
                        terms: self.line_terms(),
                    }
                } else {
                    Assertion::TextStart
                };
                Ok(Node::new(NodeKind::Assert(a), self.span_from(start)))
            }
            '$' => {
                self.bump();
                let a = if self.flags.m {
                    Assertion::LineEnd {
                        terms: self.line_terms(),
                    }
                } else {
                    Assertion::TextEnd
                };
                Ok(Node::new(NodeKind::Assert(a), self.span_from(start)))
            }
            '[' => {
                let set = self.parse_class()?;
                let span = self.span_from(start);
                Ok(Node::new(
                    NodeKind::Class(self.check_utf8(set, span)?),
                    span,
                ))
            }
            '\\' => {
                let esc = self.parse_escape(false)?;
                let span = self.span_from(start);
                match esc {
                    Escape::Lit { c, hex_x } => self.literal_node(c, hex_x, span),
                    Escape::Perl(set) | Escape::Unicode(set) => Ok(Node::new(
                        NodeKind::Class(self.check_utf8(set, span)?),
                        span,
                    )),
                    Escape::Assert(a) => Ok(Node::new(NodeKind::Assert(a), span)),
                }
            }
            _ => {
                self.bump();
                let span = self.span_from(start);
                self.literal_node(c, false, span)
            }
        }
    }

    fn line_terms(&self) -> LineTerms {
        if self.flags.crlf {
            LineTerms::CrLf
        } else {
            LineTerms::Lf
        }
    }

    /// The set a negation is taken against: all scalar values, or bytes under `-u`.
    fn universe(&self) -> CharSet {
        if self.flags.u {
            CharSet::full()
        } else {
            CharSet::from_ranges([(0, 0xFF)])
        }
    }

    /// Under `-u`, anything beyond ASCII would be a non-UTF-8 byte: the `&str` API rejects it.
    /// Inside classes this applies to *every* intermediate set (each item, range, nested class
    /// and negation), not just the final one: `(?-u)[^\D]` is rejected although it equals
    /// `[0-9]` (verified against regex-syntax 0.8.12).
    fn check_utf8(&self, set: CharSet, span: Span) -> Result<CharSet> {
        if !self.flags.u && set.ranges().last().is_some_and(|&(_, hi)| hi > 0x7F) {
            return Err(Error::syntax(span, "pattern can match invalid UTF-8"));
        }
        Ok(set)
    }

    fn literal_node(&self, c: char, hex_x: bool, span: Span) -> Result<Node> {
        if !self.flags.u && hex_x && (0x80..=0xFF).contains(&(c as u32)) {
            return Err(Error::syntax(span, "pattern can match invalid UTF-8"));
        }
        if !self.flags.i {
            return Ok(Node::new(NodeKind::Literal(c), span));
        }
        let set = if self.flags.u {
            unicode::case_fold_simple(&CharSet::single(c))
        } else {
            // `regex-syntax` keeps a non-ASCII literal as-is under `(?i-u)`.
            unicode::case_fold_ascii(&CharSet::single(c))
        };
        Ok(if set.len() == 1 {
            Node::new(NodeKind::Literal(c), span)
        } else {
            Node::new(NodeKind::Class(set), span)
        })
    }

    fn fold(&self, set: CharSet) -> CharSet {
        if !self.flags.i {
            set
        } else if self.flags.u {
            unicode::case_fold_simple(&set)
        } else {
            unicode::case_fold_ascii(&set)
        }
    }

    // ---------------------------------------------------------------- escapes

    /// Parses an escape starting at `\`.
    fn parse_escape(&mut self, in_class: bool) -> Result<Escape> {
        let start = self.pos;
        self.bump(); // backslash
        let Some(c) = self.bump() else {
            return Err(Error::syntax(
                self.span_from(start),
                "incomplete escape sequence at end of pattern",
            ));
        };
        let lit = |c: char| Ok(Escape::Lit { c, hex_x: false });
        match c {
            'a' => lit('\x07'),
            'f' => lit('\x0C'),
            't' => lit('\t'),
            'n' => lit('\n'),
            'r' => lit('\r'),
            'v' => lit('\x0B'),
            'x' => self.parse_hex(start, 2, true),
            'u' => self.parse_hex(start, 4, false),
            'U' => self.parse_hex(start, 8, false),
            'd' | 'D' | 's' | 'S' | 'w' | 'W' => {
                let base = match (c.to_ascii_lowercase(), self.flags.u) {
                    ('d', true) => unicode::perl_digit().clone(),
                    ('s', true) => unicode::perl_space().clone(),
                    ('w', true) => unicode::perl_word().clone(),
                    ('d', false) => unicode::posix_class("digit").unwrap_or_default(),
                    ('s', false) => unicode::posix_class("space").unwrap_or_default(),
                    _ => unicode::posix_class("word").unwrap_or_default(),
                };
                Ok(Escape::Perl(if c.is_ascii_uppercase() {
                    self.universe().difference(&base)
                } else {
                    base
                }))
            }
            'p' | 'P' => self.parse_unicode_class(start, c == 'P'),
            '0'..='9' => Err(Error::syntax(
                self.span_from(start),
                "backreferences are not supported by the Rust regex engine",
            )),
            'A' | 'z' | 'b' | 'B' | '<' | '>' if in_class => Err(Error::syntax(
                self.span_from(start),
                "assertions are not allowed inside a character class",
            )),
            'A' => Ok(Escape::Assert(Assertion::TextStart)),
            'z' => Ok(Escape::Assert(Assertion::TextEnd)),
            '<' => Ok(Escape::Assert(Assertion::WordStart {
                word: WordSet::rust(self.flags.u),
            })),
            '>' => Ok(Escape::Assert(Assertion::WordEnd {
                word: WordSet::rust(self.flags.u),
            })),
            'B' => Ok(Escape::Assert(Assertion::NotWordBoundary {
                word: WordSet::rust(self.flags.u),
            })),
            'b' => self.parse_word_boundary(start),
            c if is_escapeable(c) => lit(c),
            _ => Err(Error::syntax(
                self.span_from(start),
                format!("unrecognized escape sequence `\\{c}`"),
            )),
        }
    }

    /// After `\b`: either `{start}`-style special boundaries or a plain word boundary.
    fn parse_word_boundary(&mut self, start: usize) -> Result<Escape> {
        let unicode = self.flags.u;
        if self.peek() == Some('{') {
            let rest = &self.pat[self.pos + 1..];
            let name_len = rest
                .find(|c: char| !(c.is_ascii_alphabetic() || c == '-'))
                .unwrap_or(rest.len());
            if name_len > 0 && rest[name_len..].starts_with('}') {
                let name = &rest[..name_len];
                let a = match name {
                    "start" => Assertion::WordStart {
                        word: WordSet::rust(unicode),
                    },
                    "end" => Assertion::WordEnd {
                        word: WordSet::rust(unicode),
                    },
                    "start-half" => Assertion::WordStartHalf {
                        word: WordSet::rust(unicode),
                    },
                    "end-half" => Assertion::WordEndHalf {
                        word: WordSet::rust(unicode),
                    },
                    _ => {
                        self.pos += name_len + 2;
                        return Err(Error::syntax(
                            self.span_from(start),
                            format!("unrecognized special word boundary `\\b{{{name}}}`"),
                        ));
                    }
                };
                self.pos += name_len + 2;
                return Ok(Escape::Assert(a));
            }
        }
        Ok(Escape::Assert(Assertion::WordBoundary {
            word: WordSet::rust(unicode),
        }))
    }

    /// After `\x`, `\u` or `\U`: `digits` fixed hex digits, or `{...}`.
    fn parse_hex(&mut self, start: usize, digits: usize, hex_x: bool) -> Result<Escape> {
        let mut text = String::new();
        let braced = self.eat('{');
        if braced {
            loop {
                self.skip_ws();
                match self.bump() {
                    None => {
                        return Err(Error::syntax(
                            self.span_from(start),
                            "unclosed hexadecimal escape",
                        ));
                    }
                    Some('}') => break,
                    Some(c) if c.is_ascii_hexdigit() => text.push(c),
                    Some(_) => {
                        return Err(Error::syntax(
                            self.span_from(start),
                            "invalid hexadecimal digit",
                        ));
                    }
                }
            }
            if text.is_empty() {
                return Err(Error::syntax(
                    self.span_from(start),
                    "empty hexadecimal escape",
                ));
            }
        } else {
            for i in 0..digits {
                if i > 0 {
                    self.skip_ws();
                }
                match self.bump() {
                    Some(c) if c.is_ascii_hexdigit() => text.push(c),
                    None => {
                        return Err(Error::syntax(
                            self.span_from(start),
                            "incomplete hexadecimal escape",
                        ));
                    }
                    Some(_) => {
                        return Err(Error::syntax(
                            self.span_from(start),
                            "invalid hexadecimal digit",
                        ));
                    }
                }
            }
        }
        let c = u32::from_str_radix(&text, 16)
            .ok()
            .and_then(char::from_u32)
            .ok_or_else(|| {
                Error::syntax(
                    self.span_from(start),
                    "hexadecimal escape is not a Unicode scalar value",
                )
            })?;
        Ok(Escape::Lit {
            c,
            hex_x: hex_x && !braced,
        })
    }

    /// After `\p` / `\P`: `X` or `{Name}`.
    fn parse_unicode_class(&mut self, start: usize, negated: bool) -> Result<Escape> {
        let name = if self.eat('{') {
            let name_start = self.pos;
            loop {
                match self.bump() {
                    None => {
                        return Err(Error::syntax(
                            self.span_from(start),
                            "unclosed Unicode class name",
                        ));
                    }
                    Some('}') => break,
                    Some(_) => {}
                }
            }
            self.pat[name_start..self.pos - 1].to_string()
        } else {
            match self.bump() {
                Some(c) => c.to_string(),
                None => {
                    return Err(Error::syntax(
                        self.span_from(start),
                        "incomplete Unicode class escape",
                    ));
                }
            }
        };
        let span = self.span_from(start);
        if !self.flags.u {
            return Err(Error::syntax(
                span,
                "Unicode classes are not allowed when Unicode mode is off (-u)",
            ));
        }
        let set = unicode::property(&name).ok_or_else(|| {
            Error::syntax(span, format!("unknown Unicode property or value `{name}`"))
        })?;
        let set = self.fold(set);
        Ok(Escape::Unicode(if negated {
            set.complement()
        } else {
            set
        }))
    }

    // ---------------------------------------------------------------- bracket classes

    /// Parses `[...]` starting at `[`, returning the final (folded, negated) set.
    fn parse_class(&mut self) -> Result<CharSet> {
        let open_start = self.pos;
        self.bump(); // [
        let open = self.span_from(open_start);
        self.enter(open)?;
        let result = self.parse_class_body(open);
        self.leave();
        result
    }

    fn parse_class_body(&mut self, open: Span) -> Result<CharSet> {
        self.skip_ws();
        let negated = self.eat('^');
        self.skip_ws();
        let mut leading = CharSet::empty();
        // Either a leading `]` or a run of leading `-` is literal, not both: in `[]--]` the
        // `--` after `]` is the difference operator (found by the real-engine oracle test).
        if self.eat(']') {
            leading = leading.union(&CharSet::single(']'));
            self.skip_ws();
        } else {
            while self.peek() == Some('-') {
                self.bump();
                leading = leading.union(&CharSet::single('-'));
                self.skip_ws();
            }
        }
        let mut lhs = leading.union(&self.parse_class_union(open)?);
        loop {
            self.skip_ws();
            match (self.peek(), self.peek_nth(1)) {
                (Some(']'), _) => {
                    self.bump();
                    break;
                }
                (Some(op @ ('&' | '-' | '~')), Some(op2)) if op == op2 => {
                    self.pos += 2;
                    let rhs = self.parse_class_union(open)?;
                    let (l, r) = (self.fold(lhs), self.fold(rhs));
                    lhs = match op {
                        '&' => l.intersect(&r),
                        '-' => l.difference(&r),
                        _ => l.symmetric_difference(&r),
                    };
                }
                (None, _) => return Err(Error::syntax(open, "unclosed character class")),
                // parse_class_union only stops at `]`, an operator, or the end.
                _ => {
                    return Err(Error::syntax(
                        self.span_here(1),
                        "unexpected character in class",
                    ));
                }
            }
        }
        let set = self.fold(lhs);
        let set = if negated {
            self.universe().difference(&set)
        } else {
            set
        };
        self.check_utf8(set, open)
    }

    /// Items up to (not including) `]`, `&&`, `--`, `~~` or the end.
    fn parse_class_union(&mut self, open: Span) -> Result<CharSet> {
        let mut set = CharSet::empty();
        loop {
            self.skip_ws();
            match (self.peek(), self.peek_nth(1)) {
                (None, _) => return Err(Error::syntax(open, "unclosed character class")),
                (Some(']'), _) => return Ok(set),
                (Some(op @ ('&' | '-' | '~')), Some(op2)) if op == op2 => return Ok(set),
                (Some('['), _) => {
                    if let Some(posix) = self.try_posix_class()? {
                        set = set.union(&posix);
                    } else {
                        set = set.union(&self.parse_class()?);
                    }
                }
                _ => set = set.union(&self.parse_class_range()?),
            }
        }
    }

    /// `[:name:]` or `[:^name:]` at the cursor. Leaves the cursor untouched if it is not one.
    fn try_posix_class(&mut self) -> Result<Option<CharSet>> {
        let rest = &self.pat[self.pos..];
        let Some(body) = rest.strip_prefix("[:") else {
            return Ok(None);
        };
        let Some(end) = body.find(":]") else {
            return Ok(None);
        };
        let (negated, name) = match body[..end].strip_prefix('^') {
            Some(n) => (true, n),
            None => (false, &body[..end]),
        };
        let Some(set) = unicode::posix_class(name) else {
            // `regex-syntax` treats an unknown name as a nested class, not an error.
            return Ok(None);
        };
        let start = self.pos;
        self.pos += 2 + end + 2;
        let set = self.fold(set);
        let set = if negated {
            self.universe().difference(&set)
        } else {
            set
        };
        Ok(Some(self.check_utf8(set, self.span_from(start))?))
    }

    /// One class item, possibly a range `a-z`.
    fn parse_class_range(&mut self) -> Result<CharSet> {
        let start = self.pos;
        let lo = self.parse_class_item()?;
        self.skip_ws();
        let is_range = self.peek() == Some('-') && {
            // Look past the `-` (and any x-mode whitespace) for `]` or another `-`.
            let save = self.pos;
            self.pos += 1;
            let next = self.peek_after_ws();
            self.pos = save;
            !matches!(next, Some(']') | Some('-'))
        };
        if !is_range {
            return self.class_item_set(lo, Span::new(start, self.pos));
        }
        self.bump(); // -
        self.skip_ws();
        let hi = self.parse_class_item()?;
        let span = self.span_from(start);
        match (lo, hi) {
            (Escape::Lit { c: a, hex_x: hx_a }, Escape::Lit { c: b, hex_x: hx_b }) => {
                self.check_class_literal(a, hx_a, span)?;
                self.check_class_literal(b, hx_b, span)?;
                if a > b {
                    return Err(Error::syntax(
                        span,
                        "invalid class range: start is greater than end",
                    ));
                }
                self.check_utf8(CharSet::range(a, b), span)
            }
            _ => Err(Error::syntax(
                span,
                "invalid range boundary: both ends must be literal characters",
            )),
        }
    }

    fn parse_class_item(&mut self) -> Result<Escape> {
        match self.peek() {
            Some('\\') => self.parse_escape(true),
            Some(c) => {
                self.bump();
                Ok(Escape::Lit { c, hex_x: false })
            }
            None => Err(Error::syntax(self.span_here(1), "unclosed character class")),
        }
    }

    fn class_item_set(&self, item: Escape, span: Span) -> Result<CharSet> {
        match item {
            Escape::Lit { c, hex_x } => {
                self.check_class_literal(c, hex_x, span)?;
                self.check_utf8(CharSet::single(c), span)
            }
            Escape::Perl(s) | Escape::Unicode(s) => self.check_utf8(s, span),
            Escape::Assert(_) => Err(Error::syntax(
                span,
                "assertions are not allowed inside a character class",
            )),
        }
    }

    /// Under `-u`, a class literal must be a byte: ASCII, or a `\x` escape up to `\xFF`.
    fn check_class_literal(&self, c: char, hex_x: bool, span: Span) -> Result<()> {
        if !self.flags.u && !c.is_ascii() && !(hex_x && (c as u32) <= 0xFF) {
            return Err(Error::syntax(
                span,
                "Unicode not allowed here (non-ASCII class item under -u)",
            ));
        }
        Ok(())
    }
}

/// ASCII characters other than `[0-9A-Za-z<>]` may be escaped to mean themselves.
fn is_escapeable(c: char) -> bool {
    c.is_ascii() && !c.is_ascii_alphanumeric() && c != '<' && c != '>'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(p: &str) -> String {
        match parse(p) {
            Ok(r) => r.root.to_string(),
            Err(e) => panic!("{p:?} should parse: {}", e.render(p)),
        }
    }

    fn err(p: &str) -> Error {
        match parse(p) {
            Ok(r) => panic!("{p:?} should fail, got {}", r.root),
            Err(e) => e,
        }
    }

    #[test]
    fn basics() {
        assert_eq!(ok("a(b|c)*"), "(cat 'a' (rep{0,} (cap1 (alt 'b' 'c'))))");
        assert_eq!(ok(""), "()");
        assert_eq!(ok("a|"), "(alt 'a' ())");
        assert_eq!(ok("(?:)"), "(grp ())");
        assert_eq!(ok("a{2,5}?"), "(rep{2,5}? 'a')");
        assert_eq!(ok("a{3}"), "(rep{3,3} 'a')");
        assert_eq!(ok("^$"), "(cat TextStart TextEnd)");
        assert_eq!(
            ok("(?m)^$"),
            "(cat LineStart { terms: Lf } LineEnd { terms: Lf })"
        );
        assert_eq!(ok("[a-c]"), "[a-c]");
        assert_eq!(ok("[]a]"), "[\\]a]");
        assert_eq!(ok("[a-]"), "[\\-a]");
        assert_eq!(ok(r"\x41\u{42}\U00000043"), "(cat 'A' 'B' 'C')");
    }

    #[test]
    fn flags_scope() {
        // Directive persists across `|` to the end of the group.
        assert_eq!(ok("(?i)a|b"), "(alt [Aa] [Bb])");
        assert_eq!(ok("((?i)a)b"), "(cat (cap1 [Aa]) 'b')");
        assert_eq!(ok("(?i:a)b"), "(cat (grp [Aa]) 'b')");
        assert_eq!(ok("(?U)a*"), "(rep{0,}? 'a')");
        assert_eq!(ok("(?x) a b # comment\n c"), "(cat 'a' 'b' 'c')");
    }

    #[test]
    fn class_ops() {
        assert_eq!(ok("[a-z&&[^aeiou]&&a-f]"), "[b-df]");
        assert_eq!(ok("[a-c--b]"), "[ac]");
        assert_eq!(ok("[a-c~~b-d]"), "[ad]");
        assert_eq!(ok("[[:digit:]x]"), "[0-9x]");
    }

    #[test]
    fn kelvin_folding() {
        let r = parse("(?i)k").unwrap();
        let crate::ir::NodeKind::Class(s) = r.root.kind else {
            panic!()
        };
        assert!(s.contains('\u{212A}'));
    }

    #[test]
    fn errors_have_spans() {
        assert_eq!(err("a)").span, Span::new(1, 2));
        assert_eq!(err("(a").span, Span::new(0, 1));
        assert_eq!(err("*").kind, crate::error::ErrorKind::Syntax);
        assert_eq!(err("a{3,2}").kind, crate::error::ErrorKind::Syntax);
        assert_eq!(err(r"(a)\1").kind, crate::error::ErrorKind::Syntax);
        assert_eq!(err("(?=a)").kind, crate::error::ErrorKind::Syntax);
        assert_eq!(err("[a").kind, crate::error::ErrorKind::Syntax);
        assert_eq!(err("(?i)*").kind, crate::error::ErrorKind::Syntax);
        assert_eq!(err("(?)").kind, crate::error::ErrorKind::Syntax);
        assert_eq!(err("(?-)").kind, crate::error::ErrorKind::Syntax);
        assert_eq!(err("(?ii)").kind, crate::error::ErrorKind::Syntax);
        assert_eq!(
            err("(?P<n>a)(?P<n>b)").kind,
            crate::error::ErrorKind::Syntax
        );
        assert_eq!(err("(?-u:.)").kind, crate::error::ErrorKind::Syntax);
        assert_eq!(err(r"[\d-z]").kind, crate::error::ErrorKind::Syntax);
    }

    /// Fuzzer-found slow inputs (case folding of near-full sets at every nesting level and set
    /// operation). Must stay fast; the bound is generous to tolerate slow CI machines.
    #[test]
    fn case_folding_negated_classes_is_fast() {
        let t = std::time::Instant::now();
        let deep = format!("(?i){}^a{}", "[".repeat(200), "]".repeat(200));
        assert!(parse(&deep).is_ok());
        let ops = format!("(?i)[^a{}]", "~~".repeat(120));
        assert!(parse(&ops).is_ok());
        assert!(parse("(?i)&&[[^^@^@^@^@~~~~~~~~~~~f]~~~~~~~~~~~~~~~~~~~~~~~~~pha:]").is_ok());
        assert!(
            t.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            t.elapsed()
        );
    }

    #[test]
    fn nest_limit_is_an_error_not_a_crash() {
        let deep = "(".repeat(10_000) + &")".repeat(10_000);
        assert_eq!(err(&deep).kind, crate::error::ErrorKind::Syntax);
        let deep = "[".repeat(10_000);
        assert_eq!(err(&deep).kind, crate::error::ErrorKind::Syntax);
    }
}
