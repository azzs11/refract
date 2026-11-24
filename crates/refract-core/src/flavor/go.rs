//! Front-end for Go `regexp` (Go 1.27, `syntax.Perl` flags). Spec: `spec/flavours/go.md`.
//!
//! Mirrors `regexp/syntax/parse.go` rule for rule (read from the pinned image); Unicode data
//! comes from Go's own `unicode` package (`tables::go`).

use crate::charset::CharSet;
use crate::error::{Error, Result, Span};
use crate::flavor::{Flavor, MatchPolicy, Profile};
use crate::ir::{
    Assertion, Capture, Domain, Group, LineTerms, Node, NodeKind, Quirks, Regex, Repeat, WordSet,
};
use crate::tables::{self, go as t};
use crate::unicode;
use std::collections::HashSet;

pub static PROFILE: Profile = Profile {
    flavor: Flavor::Go,
    engine: "Go regexp (RE2)",
    unicode: "Go 1.27.4 unicode package (Unicode 17.0.0)",
    dot_excludes: "\\n",
    dollar: "end of haystack only",
    unicode_classes: false,
    case_folding: "unicode.SimpleFold orbits",
    match_policy: MatchPolicy::LeftmostFirst,
    lookaround: false,
    backreferences: false,
    linear_time: true,
    spec: "spec/flavours/go.md",
};

/// Go's parser limits tree height to 1000; we stop well before modelling that exactly.
const NEST_LIMIT: u32 = 500;

#[derive(Clone, Copy, Debug, Default)]
struct Flags {
    i: bool,
    m: bool,
    s: bool,
    ungreedy: bool,
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    flags: Flags,
    captures: Vec<Option<String>>,
    /// Spans of `{n,m}` repetitions, for `repeatIsValid`.
    braces: HashSet<(usize, usize)>,
    depth: u32,
}

pub fn parse(pattern: &str) -> Result<Regex> {
    let mut p = Parser {
        src: pattern,
        pos: 0,
        flags: Flags::default(),
        captures: Vec::new(),
        braces: HashSet::new(),
        depth: 0,
    };
    let root = p.alternation()?;
    if p.peek() == Some(')') {
        return Err(Error::syntax(p.here(), "unexpected )"));
    }
    Ok(Regex {
        flavor: Flavor::Go,
        pattern: pattern.to_string(),
        root,
        captures: p.captures,
        domain: Domain::Scalars,
        quirks: Quirks::default(),
    })
}

/// `canonicalName` from `regexp/syntax`.
fn canonical_name(name: &str) -> String {
    let mut out = String::new();
    let mut first = true;
    for c in name.chars() {
        match c {
            '_' | '-' | ' ' => {}
            c if first => {
                out.push(c.to_ascii_uppercase());
                first = false;
            }
            c => out.push(c.to_ascii_lowercase()),
        }
    }
    out
}

fn set_of(r: &[(u32, u32)]) -> CharSet {
    CharSet::from_ranges(r.iter().copied())
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn here(&self) -> Span {
        Span::new(self.pos, self.pos + self.peek().map_or(0, char::len_utf8))
    }

    fn span_from(&self, start: usize) -> Span {
        Span::new(start, self.pos)
    }

    fn alternation(&mut self) -> Result<Node> {
        let start = self.pos;
        let mut alts = vec![self.sequence()?];
        while self.peek() == Some('|') {
            self.bump();
            alts.push(self.sequence()?);
        }
        Ok(if alts.len() == 1 {
            alts.pop()
                .unwrap_or(Node::new(NodeKind::Empty, self.span_from(start)))
        } else {
            Node::new(NodeKind::Alt(alts), self.span_from(start))
        })
    }

    fn sequence(&mut self) -> Result<Node> {
        let start = self.pos;
        let mut items: Vec<Node> = Vec::new();
        let mut last_was_repeat = false;
        while let Some(c) = self.peek() {
            let at = self.pos;
            let mut is_repeat = false;
            match c {
                '|' | ')' => break,
                '(' => {
                    if let Some(n) = self.group()? {
                        items.push(n);
                    }
                }
                '^' | '$' => {
                    self.bump();
                    let a = match (c, self.flags.m) {
                        ('^', false) => Assertion::TextStart,
                        ('^', true) => Assertion::LineStart {
                            terms: LineTerms::Lf,
                        },
                        (_, false) => Assertion::TextEnd,
                        (_, true) => Assertion::LineEnd {
                            terms: LineTerms::Lf,
                        },
                    };
                    items.push(Node::new(NodeKind::Assert(a), self.span_from(at)));
                }
                '.' => {
                    self.bump();
                    let set = if self.flags.s {
                        CharSet::full()
                    } else {
                        CharSet::full().difference(&CharSet::single('\n'))
                    };
                    items.push(Node::new(NodeKind::Class(set), self.span_from(at)));
                }
                '[' => {
                    let set = self.class()?;
                    items.push(Node::new(NodeKind::Class(set), self.span_from(at)));
                }
                '*' | '+' | '?' | '{' => {
                    let (min, max, brace) = match c {
                        '*' => (0, None, false),
                        '+' => (1, None, false),
                        '?' => (0, Some(1), false),
                        _ => match self.brace()? {
                            Some((a, b)) => (a, b, true),
                            None => {
                                self.bump();
                                items.push(self.literal('{', self.span_from(at)));
                                last_was_repeat = false;
                                continue;
                            }
                        },
                    };
                    if !brace {
                        self.bump();
                    }
                    let lazy = self.peek() == Some('?');
                    if lazy {
                        self.bump();
                    }
                    if last_was_repeat {
                        return Err(Error::syntax(
                            self.span_from(at),
                            "invalid nested repetition operator",
                        ));
                    }
                    let Some(sub) = items.pop() else {
                        return Err(Error::syntax(
                            self.span_from(at),
                            "missing argument to repetition operator",
                        ));
                    };
                    let span = sub.span.to(self.span_from(at));
                    let node = Node::new(
                        NodeKind::Repeat(Repeat {
                            sub: Box::new(sub),
                            min,
                            max,
                            greedy: lazy == self.flags.ungreedy,
                            possessive: false,
                        }),
                        span,
                    );
                    if brace {
                        self.braces.insert((span.start, span.end));
                        if (min >= 2 || max.is_some_and(|m| m >= 2))
                            && !self.repeat_is_valid(&node, 1000)
                        {
                            return Err(Error::syntax(self.span_from(at), "invalid repeat count"));
                        }
                    }
                    items.push(node);
                    is_repeat = true;
                }
                '\\' => {
                    if let Some(nodes) = self.escape_outside()? {
                        items.extend(nodes);
                    }
                }
                _ => {
                    self.bump();
                    items.push(self.literal(c, self.span_from(at)));
                }
            }
            last_was_repeat = is_repeat;
        }
        let span = self.span_from(start);
        Ok(match items.len() {
            0 => Node::new(NodeKind::Empty, span),
            1 => items.pop().unwrap_or(Node::new(NodeKind::Empty, span)),
            _ => Node::new(NodeKind::Concat(items), span),
        })
    }

    /// `repeatIsValid`: nested `{n,m}` counts may not multiply past `n`.
    fn repeat_is_valid(&self, node: &Node, mut n: u32) -> bool {
        if let NodeKind::Repeat(r) = &node.kind
            && self.braces.contains(&(node.span.start, node.span.end))
        {
            let m = r.max.unwrap_or(r.min);
            if m == 0 {
                return true;
            }
            if m > n {
                return false;
            }
            n /= m;
        }
        match &node.kind {
            NodeKind::Concat(xs) | NodeKind::Alt(xs) => {
                xs.iter().all(|x| self.repeat_is_valid(x, n))
            }
            NodeKind::Repeat(r) => self.repeat_is_valid(&r.sub, n),
            NodeKind::Group(g) => self.repeat_is_valid(&g.sub, n),
            _ => true,
        }
    }

    /// `parseRepeat` + range checks. `None`: not a repetition, `{` is a literal.
    fn brace(&mut self) -> Result<Option<(u32, Option<u32>)>> {
        let s = self.rest();
        let int = |s: &'a str| -> Option<(i64, &'a str)> {
            let len = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
            if len == 0 || (len >= 2 && s.starts_with('0')) {
                return None;
            }
            let digits = &s[..len];
            let v = if len > 8 {
                -1
            } else {
                digits.parse::<i64>().unwrap_or(-1)
            };
            let v = if v >= 100_000_000 { -1 } else { v };
            Some((v, &s[len..]))
        };
        let Some((min, after)) = int(&s[1..]) else {
            return Ok(None);
        };
        let (min, max, after) = if let Some(r) = after.strip_prefix(',') {
            if r.starts_with('}') {
                (min, -1, r)
            } else {
                let Some((max, r2)) = int(r) else {
                    return Ok(None);
                };
                (if max < 0 { -1 } else { min }, max, r2)
            }
        } else {
            (min, min, after)
        };
        let Some(after) = after.strip_prefix('}') else {
            return Ok(None);
        };
        let start = self.pos;
        self.pos = self.src.len() - after.len();
        if !(0..=1000).contains(&min) || max > 1000 || (max >= 0 && min > max) {
            return Err(Error::syntax(self.span_from(start), "invalid repeat count"));
        }
        Ok(Some((
            min as u32,
            if max < 0 { None } else { Some(max as u32) },
        )))
    }

    fn literal(&self, c: char, span: Span) -> Node {
        if !self.flags.i {
            return Node::new(NodeKind::Literal(c), span);
        }
        let set = tables::fold_with(&CharSet::single(c), t::FOLD);
        if set.len() == 1 {
            Node::new(NodeKind::Literal(c), span)
        } else {
            Node::new(NodeKind::Class(set), span)
        }
    }

    fn fold(&self, set: CharSet) -> CharSet {
        if self.flags.i {
            tables::fold_with(&set, t::FOLD)
        } else {
            set
        }
    }

    /// `\d \s \w` and negations (folded under `(?i)` before negating, like `appendGroup`).
    fn perl_class(&self, c: char) -> Option<CharSet> {
        let base = match c.to_ascii_lowercase() {
            'd' => CharSet::range('0', '9'),
            's' => CharSet::from_iter(['\t', '\n', '\x0C', '\r', ' ']),
            'w' => unicode::posix_class("word")?,
            _ => return None,
        };
        let base = self.fold(base);
        Some(if c.is_ascii_uppercase() {
            base.complement()
        } else {
            base
        })
    }

    /// At `\p` / `\P`: `parseUnicodeClass`.
    fn unicode_class(&mut self) -> Result<CharSet> {
        let start = self.pos;
        self.pos += 1; // backslash
        let mut negated = self.bump() == Some('P');
        let name = if self.peek() == Some('{') {
            let Some(end) = self.rest().find('}') else {
                return Err(Error::syntax(
                    self.span_from(start),
                    "invalid character class range",
                ));
            };
            let name = self.rest()[1..end].to_string();
            self.pos += end + 1;
            name
        } else {
            match self.bump() {
                Some(c) => c.to_string(),
                None => String::new(),
            }
        };
        let name = match name.strip_prefix('^') {
            Some(n) => {
                negated = !negated;
                n.to_string()
            }
            None => name,
        };
        let canon = canonical_name(&name);
        let Some(&(_, tab, fold, flip)) = t::PROPERTIES.iter().find(|(n, ..)| *n == canon) else {
            return Err(Error::syntax(
                self.span_from(start),
                "invalid character class range",
            ));
        };
        if flip {
            negated = !negated;
        }
        let mut set = set_of(tab);
        if self.flags.i && !fold.is_empty() {
            set = set.union(&set_of(fold));
        }
        Ok(if negated { set.complement() } else { set })
    }

    /// `parseEscape`: one escaped rune.
    fn escape_char(&mut self) -> Result<char> {
        let start = self.pos;
        self.bump(); // backslash
        let bad = |p: &Self| Error::syntax(p.span_from(start), "invalid escape sequence");
        let Some(c) = self.bump() else {
            return Err(Error::syntax(
                self.span_from(start),
                "trailing backslash at end of expression",
            ));
        };
        let octal = |ch: Option<char>| ch.is_some_and(|d| ('0'..='7').contains(&d));
        match c {
            '1'..='7' if !octal(self.peek()) => Err(bad(self)),
            '0'..='7' => {
                let mut v = c as u32 - '0' as u32;
                for _ in 1..3 {
                    if !octal(self.peek()) {
                        break;
                    }
                    v = v * 8 + (self.bump().unwrap_or('0') as u32 - '0' as u32);
                }
                Ok(char::from_u32(v).unwrap_or('\0'))
            }
            'x' => {
                if self.peek() == Some('{') {
                    self.bump();
                    let mut v: u32 = 0;
                    let mut n = 0;
                    loop {
                        match self.bump() {
                            None => return Err(bad(self)),
                            Some('}') => break,
                            Some(h) => {
                                let d = h.to_digit(16).ok_or_else(|| bad(self))?;
                                v = v * 16 + d;
                                if v > 0x10_FFFF {
                                    return Err(bad(self));
                                }
                                n += 1;
                            }
                        }
                    }
                    if n == 0 {
                        return Err(bad(self));
                    }
                    // A surrogate code point never matches a string of scalar values.
                    return Ok(char::from_u32(v).unwrap_or('\u{FFFF}'));
                }
                let x = self.bump().and_then(|h| h.to_digit(16));
                let y = self.bump().and_then(|h| h.to_digit(16));
                match (x, y) {
                    (Some(x), Some(y)) => Ok(char::from_u32(x * 16 + y).unwrap_or('\0')),
                    _ => Err(bad(self)),
                }
            }
            'a' => Ok('\x07'),
            'f' => Ok('\x0C'),
            'n' => Ok('\n'),
            'r' => Ok('\r'),
            't' => Ok('\t'),
            'v' => Ok('\x0B'),
            c if c.is_ascii() && !c.is_ascii_alphanumeric() => Ok(c),
            _ => Err(bad(self)),
        }
    }

    /// An escape outside a class. `None` for an empty `\Q\E`.
    fn escape_outside(&mut self) -> Result<Option<Vec<Node>>> {
        let start = self.pos;
        let next = self.rest()[1..].chars().next();
        let assert = |p: &mut Self, a| {
            p.pos += 2;
            Ok(Some(vec![Node::new(
                NodeKind::Assert(a),
                p.span_from(start),
            )]))
        };
        match next {
            Some('A') => return assert(self, Assertion::TextStart),
            Some('z') => return assert(self, Assertion::TextEnd),
            Some('b') => {
                return assert(
                    self,
                    Assertion::WordBoundary {
                        word: WordSet::Ascii,
                    },
                );
            }
            Some('B') => {
                return assert(
                    self,
                    Assertion::NotWordBoundary {
                        word: WordSet::Ascii,
                    },
                );
            }
            Some('C') => {
                self.pos += 2;
                return Err(Error::syntax(
                    self.span_from(start),
                    "invalid escape sequence",
                ));
            }
            Some('Q') => {
                self.pos += 2;
                let rest = self.rest();
                let (lit, used) = match rest.find("\\E") {
                    Some(i) => (&rest[..i], i + 2),
                    None => (rest, rest.len()),
                };
                let nodes: Vec<Node> = lit
                    .char_indices()
                    .map(|(k, c)| {
                        let at = self.pos + k;
                        self.literal(c, Span::new(at, at + c.len_utf8()))
                    })
                    .collect();
                self.pos += used;
                return Ok(if nodes.is_empty() { None } else { Some(nodes) });
            }
            Some('p') | Some('P') => {
                let set = self.unicode_class()?;
                return Ok(Some(vec![Node::new(
                    NodeKind::Class(set),
                    self.span_from(start),
                )]));
            }
            Some(c) if self.perl_class(c).is_some() => {
                self.pos += 2;
                let set = self.perl_class(c).unwrap_or_default();
                return Ok(Some(vec![Node::new(
                    NodeKind::Class(set),
                    self.span_from(start),
                )]));
            }
            _ => {}
        }
        let c = self.escape_char()?;
        Ok(Some(vec![self.literal(c, self.span_from(start))]))
    }

    /// `parseClass`.
    fn class(&mut self) -> Result<CharSet> {
        let open = self.here();
        self.bump(); // [
        let negated = self.peek() == Some('^');
        if negated {
            self.bump();
        }
        let missing = |p: &Self| Error::syntax(open.to(p.here()), "missing closing ]");
        let mut set = CharSet::empty();
        let mut first = true;
        loop {
            match self.peek() {
                Some(']') if !first => {
                    self.bump();
                    break;
                }
                None => return Err(missing(self)),
                _ => {}
            }
            first = false;
            let rest = self.rest();
            if rest.len() > 2
                && rest.starts_with("[:")
                && let Some(i) = rest[2..].find(":]")
            {
                {
                    let name = &rest[2..2 + i];
                    let (neg, base) = match name.strip_prefix('^') {
                        Some(n) => (true, n),
                        None => (false, name),
                    };
                    let Some(cls) = unicode::posix_class(base) else {
                        return Err(Error::syntax(self.here(), "invalid character class range"));
                    };
                    self.pos += 2 + i + 2;
                    let cls = self.fold(cls);
                    set = set.union(&if neg { cls.complement() } else { cls });
                    continue;
                }
            }
            if rest.starts_with("\\p") || rest.starts_with("\\P") {
                set = set.union(&self.unicode_class()?);
                continue;
            }
            if rest.starts_with('\\')
                && let Some(c) = rest[1..].chars().next()
                && let Some(cls) = self.perl_class(c)
            {
                self.pos += 2;
                set = set.union(&cls);
                continue;
            }
            let lo = self.class_char().map_err(|e| {
                if self.peek().is_none() {
                    missing(self)
                } else {
                    e
                }
            })?;
            let mut hi = lo;
            let rest = self.rest();
            if rest.len() >= 2 && rest.starts_with('-') && !rest[1..].starts_with(']') {
                let range_start = self.pos;
                self.bump();
                if self.peek().is_none() {
                    return Err(missing(self));
                }
                hi = self.class_char()?;
                if hi < lo {
                    return Err(Error::syntax(
                        self.span_from(range_start),
                        "invalid character class range",
                    ));
                }
            }
            set = set.union(&self.fold(CharSet::range(lo, hi)));
        }
        Ok(if negated { set.complement() } else { set })
    }

    fn class_char(&mut self) -> Result<char> {
        match self.peek() {
            Some('\\') => self.escape_char(),
            Some(c) => {
                self.bump();
                Ok(c)
            }
            None => Err(Error::syntax(self.here(), "missing closing ]")),
        }
    }

    /// At `(`. `None` for a flag directive like `(?i)`.
    fn group(&mut self) -> Result<Option<Node>> {
        let start = self.pos;
        let open = self.here();
        let rest = self.rest();
        let mut capture = None;
        let mut scoped = None;
        if let Some(after) = rest.strip_prefix("(?") {
            let named_p = rest.len() > 4 && after.starts_with("P<");
            let named = rest.len() > 3 && after.starts_with('<');
            if named_p || named {
                let Some(end) = rest.find('>') else {
                    return Err(Error::syntax(
                        Span::new(start, self.src.len()),
                        "invalid named capture",
                    ));
                };
                let name = &rest[if named_p { 4 } else { 3 }..end];
                if name.is_empty() || !name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric()) {
                    return Err(Error::syntax(
                        Span::new(start, start + end + 1),
                        "invalid named capture",
                    ));
                }
                self.pos += end + 1;
                self.captures.push(Some(name.to_string()));
                capture = Some(Capture {
                    index: self.captures.len() as u32,
                    name: Some(name.to_string()),
                });
            } else {
                self.pos += 2;
                let mut f = self.flags;
                let mut negating = false;
                let mut saw = false;
                loop {
                    let Some(c) = self.bump() else {
                        return Err(Error::syntax(
                            self.span_from(start),
                            "invalid or unsupported Perl syntax",
                        ));
                    };
                    let v = !negating;
                    match c {
                        'i' => f.i = v,
                        'm' => f.m = v,
                        's' => f.s = v,
                        'U' => f.ungreedy = v,
                        '-' if !negating => {
                            negating = true;
                            saw = false;
                            continue;
                        }
                        ':' | ')' => {
                            if negating && !saw {
                                return Err(Error::syntax(
                                    self.span_from(start),
                                    "invalid or unsupported Perl syntax",
                                ));
                            }
                            if c == ')' {
                                self.flags = f;
                                return Ok(None);
                            }
                            scoped = Some(f);
                            break;
                        }
                        _ => {
                            return Err(Error::syntax(
                                self.span_from(start),
                                "invalid or unsupported Perl syntax",
                            ));
                        }
                    }
                    saw = true;
                }
            }
        } else {
            self.bump();
            self.captures.push(None);
            capture = Some(Capture {
                index: self.captures.len() as u32,
                name: None,
            });
        }
        self.depth += 1;
        if self.depth > NEST_LIMIT {
            return Err(Error::unsupported(
                open,
                "nesting too deep to model Go's height limit",
            ));
        }
        let saved = self.flags;
        if let Some(f) = scoped {
            self.flags = f;
        }
        let sub = self.alternation();
        self.flags = saved;
        self.depth -= 1;
        let sub = sub?;
        if self.peek() != Some(')') {
            return Err(Error::syntax(
                Span::new(start, self.src.len()),
                "missing closing )",
            ));
        }
        self.bump();
        Ok(Some(Node::new(
            NodeKind::Group(Group {
                sub: Box::new(sub),
                capture,
                atomic: false,
            }),
            self.span_from(start),
        )))
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
    fn go_rules() {
        assert_eq!(ok("a{"), "(cat 'a' '{')");
        assert_eq!(ok("a{01}"), "(cat 'a' '{' '0' '1' '}')");
        assert_eq!(ok("a(?i)*"), "(rep{0,} 'a')");
        assert_eq!(ok(r"\Q*+"), "(cat '*' '+')");
        assert_eq!(ok(r"\12"), "'\\n'");
        assert!(ok("(?P<n>a)(?P<n>b)").contains("cap2"));
        for p in [
            "a**",
            "a*?*",
            "a{2}{3}",
            "a{1001}",
            "(a{2}){600}",
            r"\1",
            r"\8",
            "[]",
            "(?-)",
            "(?=a)",
            r"\Z",
            r"\p{Script=Greek}",
            "[[:foo:]]",
            "(?i)*",
            "x|*",
            ")",
            "[a-\\d]",
        ] {
            syntax(p);
        }
        assert!(parse(r"\p{greek}").is_ok());
    }
}
