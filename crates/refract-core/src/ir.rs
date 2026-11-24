//! The flavour-independent intermediate representation (see `spec/ir.md`).
//!
//! Front-ends resolve everything that depends on the flavour *before* producing IR: character
//! classes become explicit [`CharSet`]s (Unicode tables, case folding, `.` semantics), and `^`,
//! `$`, `\b` become precise [`Assertion`]s. After parsing, no flag or flavour lookup is needed to
//! know what a node means.

use crate::charset::CharSet;
use crate::error::Span;
use crate::flavor::Flavor;
use std::fmt::{self, Write as _};

/// A parsed regex: its IR tree plus metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Regex {
    pub flavor: Flavor,
    pub pattern: String,
    pub root: Node,
    /// Capture groups in order of their opening parenthesis; index 0 is group 1.
    pub captures: Vec<Option<String>>,
}

/// One IR node with the span of pattern text it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub kind: NodeKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// Matches the empty string.
    Empty,
    /// One exact character (case-sensitive).
    Literal(char),
    /// Any one character from the set. An empty set never matches.
    Class(CharSet),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat(Repeat),
    Group(Group),
    /// A zero-width assertion.
    Assert(Assertion),
    /// Look-ahead / look-behind, possibly negated. Not regular in general; analysed in P4.
    Look(Look),
    /// Backreference to a capture group (1-based). Not regular; bounded analysis in P4.
    Backref(u32),
    /// `(?(group)yes|no)`: match `yes` if the group participated, else `no` (Python, PCRE).
    Conditional(Conditional),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    pub sub: Box<Node>,
    pub behind: bool,
    pub negated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conditional {
    pub group: u32,
    pub yes: Box<Node>,
    pub no: Option<Box<Node>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repeat {
    pub sub: Box<Node>,
    pub min: u32,
    /// `None` means unbounded.
    pub max: Option<u32>,
    pub greedy: bool,
    /// Possessive (`a*+`): never gives back characters. Changes the language.
    pub possessive: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub sub: Box<Node>,
    pub capture: Option<Capture>,
    /// Atomic group `(?>...)`: once matched, never backtracked into. Changes the language.
    pub atomic: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capture {
    /// 1-based capture index.
    pub index: u32,
    pub name: Option<String>,
}

/// Zero-width assertions, each with one precise meaning independent of flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Assertion {
    /// Start of the haystack (`\A`; `^` without multi-line).
    TextStart,
    /// End of the haystack (`\z`; Rust/Go/JS `$` without multi-line).
    TextEnd,
    /// End of the haystack or just before a final `\n` (Python/PCRE2/Java/.NET/Ruby `$`, `\Z`).
    TextEndOrFinalNewline,
    /// Start of a line: haystack start or after `\n` (after `\r` or `\n`, not between `\r\n`,
    /// when `crlf`).
    LineStart {
        crlf: bool,
    },
    /// End of a line: haystack end or before `\n` (before `\r` or `\n`, not between `\r\n`,
    /// when `crlf`).
    LineEnd {
        crlf: bool,
    },
    WordBoundary {
        unicode: bool,
    },
    NotWordBoundary {
        unicode: bool,
    },
    /// `\<` / `\b{start}`: word char on the right, non-word on the left.
    WordStart {
        unicode: bool,
    },
    /// `\>` / `\b{end}`.
    WordEnd {
        unicode: bool,
    },
    /// `\b{start-half}`: non-word (or nothing) on the left.
    WordStartHalf {
        unicode: bool,
    },
    /// `\b{end-half}`: non-word (or nothing) on the right.
    WordEndHalf {
        unicode: bool,
    },
}

impl Node {
    pub fn new(kind: NodeKind, span: Span) -> Self {
        Node { kind, span }
    }

    /// Visits every node in pre-order.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Node)) {
        f(self);
        match &self.kind {
            NodeKind::Concat(xs) | NodeKind::Alt(xs) => xs.iter().for_each(|x| x.walk(f)),
            NodeKind::Repeat(r) => r.sub.walk(f),
            NodeKind::Group(g) => g.sub.walk(f),
            NodeKind::Look(l) => l.sub.walk(f),
            NodeKind::Conditional(c) => {
                c.yes.walk(f);
                if let Some(n) = &c.no {
                    n.walk(f);
                }
            }
            NodeKind::Empty
            | NodeKind::Literal(_)
            | NodeKind::Class(_)
            | NodeKind::Assert(_)
            | NodeKind::Backref(_) => {}
        }
    }

    /// An indented, one-node-per-line rendering used by `refract parse` and snapshot tests.
    pub fn pretty(&self) -> String {
        let mut out = String::new();
        self.pretty_into(&mut out, 0);
        out
    }

    fn pretty_into(&self, out: &mut String, depth: usize) {
        let pad = "  ".repeat(depth);
        let _ = write!(out, "{pad}");
        match &self.kind {
            NodeKind::Empty => out.push_str("empty"),
            NodeKind::Literal(c) => {
                let _ = write!(out, "lit {c:?}");
            }
            NodeKind::Class(s) => {
                let _ = write!(out, "class {s}");
            }
            NodeKind::Concat(_) => out.push_str("concat"),
            NodeKind::Alt(_) => out.push_str("alt"),
            NodeKind::Repeat(r) => {
                let max = r.max.map_or("inf".to_string(), |m| m.to_string());
                let lazy = match (r.possessive, r.greedy) {
                    (true, _) => " possessive",
                    (false, true) => "",
                    (false, false) => " lazy",
                };
                let _ = write!(out, "repeat {{{},{max}}}{lazy}", r.min);
            }
            NodeKind::Group(g) => match &g.capture {
                Some(Capture {
                    index,
                    name: Some(n),
                }) => {
                    let _ = write!(out, "group #{index} <{n}>");
                }
                Some(Capture { index, name: None }) => {
                    let _ = write!(out, "group #{index}");
                }
                None if g.atomic => out.push_str("atomic"),
                None => out.push_str("group"),
            },
            NodeKind::Look(l) => {
                let _ = write!(
                    out,
                    "look{}{}",
                    if l.behind { "behind" } else { "ahead" },
                    if l.negated { " negated" } else { "" }
                );
            }
            NodeKind::Backref(i) => {
                let _ = write!(out, "backref #{i}");
            }
            NodeKind::Conditional(c) => {
                let _ = write!(out, "if-group #{}", c.group);
            }
            NodeKind::Assert(a) => {
                let _ = write!(out, "assert {a:?}");
            }
        }
        let _ = writeln!(out, "  @{}", self.span);
        match &self.kind {
            NodeKind::Concat(xs) | NodeKind::Alt(xs) => {
                xs.iter().for_each(|x| x.pretty_into(out, depth + 1));
            }
            NodeKind::Repeat(r) => r.sub.pretty_into(out, depth + 1),
            NodeKind::Group(g) => g.sub.pretty_into(out, depth + 1),
            NodeKind::Look(l) => l.sub.pretty_into(out, depth + 1),
            NodeKind::Conditional(c) => {
                c.yes.pretty_into(out, depth + 1);
                if let Some(n) = &c.no {
                    n.pretty_into(out, depth + 1);
                }
            }
            _ => {}
        }
    }
}

/// Compact S-expression form, e.g. `(cat 'a' (cap1 (alt 'b' 'c')))`.
impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |f: &mut fmt::Formatter<'_>, head: &str, xs: &[Node]| {
            write!(f, "({head}")?;
            for x in xs {
                write!(f, " {x}")?;
            }
            f.write_str(")")
        };
        match &self.kind {
            NodeKind::Empty => f.write_str("()"),
            NodeKind::Literal(c) => write!(f, "{c:?}"),
            NodeKind::Class(s) => write!(f, "{s}"),
            NodeKind::Concat(xs) => list(f, "cat", xs),
            NodeKind::Alt(xs) => list(f, "alt", xs),
            NodeKind::Repeat(r) => {
                let max = r.max.map_or(String::new(), |m| m.to_string());
                let lazy = match (r.possessive, r.greedy) {
                    (true, _) => "+",
                    (false, true) => "",
                    (false, false) => "?",
                };
                write!(f, "(rep{{{},{max}}}{lazy} {})", r.min, r.sub)
            }
            NodeKind::Group(g) => match &g.capture {
                Some(c) => write!(f, "(cap{} {})", c.index, g.sub),
                None if g.atomic => write!(f, "(atomic {})", g.sub),
                None => write!(f, "(grp {})", g.sub),
            },
            NodeKind::Assert(a) => write!(f, "{a:?}"),
            NodeKind::Look(l) => write!(
                f,
                "({}{} {})",
                if l.negated { "not-" } else { "" },
                if l.behind { "behind" } else { "ahead" },
                l.sub
            ),
            NodeKind::Backref(i) => write!(f, "\\{i}"),
            NodeKind::Conditional(c) => match &c.no {
                Some(n) => write!(f, "(if{} {} {})", c.group, c.yes, n),
                None => write!(f, "(if{} {})", c.group, c.yes),
            },
        }
    }
}

impl Regex {
    /// True when the regex contains any zero-width assertion.
    pub fn has_assertions(&self) -> bool {
        let mut found = false;
        self.root
            .walk(&mut |n| found |= matches!(n.kind, NodeKind::Assert(_)));
        found
    }
}
