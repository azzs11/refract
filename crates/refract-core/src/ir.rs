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
    /// Haystacks the analysis covers (ADR-0007).
    pub domain: Domain,
    /// Engine behaviour that deviates from the flavour's specification but that a verdict about
    /// the real engine must model (docs/findings.md).
    pub quirks: Quirks,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Quirks {
    /// V8 with `u`: search also tries positions inside a surrogate pair, where `^ $ \b` are
    /// false, `\B` is true and look-arounds see no characters (docs/findings.md).
    pub mid_pair_search: bool,
    /// JS `y` (sticky): `test()` from `lastIndex` 0 only tries index 0.
    pub sticky: bool,
}

/// The set of haystacks a verdict is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Domain {
    /// All strings of Unicode scalar values.
    Scalars,
    /// JavaScript without `u`/`v`: the pattern matches UTF-16 code units. Lowering yields a
    /// language over code units (surrogates as pseudo code points); `utf16::scalar_dfa` lifts it
    /// to haystacks of scalar values exactly (ADR-0007).
    Utf16,
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
    /// Backreference to a capture group (1-based), with the engine's comparison rule. Not
    /// regular: analysed by the bounded interpreter (ADR-0011).
    Backref(u32, BackrefCase),
    /// `(?(group)yes|no)`: match `yes` if the group participated, else `no` (Python, PCRE).
    Conditional(Conditional),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    pub sub: Box<Node>,
    pub behind: bool,
    pub negated: bool,
    /// Java only: the look-behind tries start positions `[i - max, i - min]` from its compile-time
    /// length estimate (`TreeInfo`), so its body is restricted to lengths in `min..=max` (`None`
    /// = unbounded). An empty window (`max < min`, after Java's int overflow) never matches.
    pub window: Option<(u32, Option<u32>)>,
    pub window_empty: bool,
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
    /// Start of a line: haystack start or right after a line terminator (see [`LineTerms`]).
    LineStart {
        terms: LineTerms,
    },
    /// End of a line: haystack end or right before a line terminator (see [`LineTerms`]).
    LineEnd {
        terms: LineTerms,
    },
    /// Java `$` without multi-line (and `\Z`): at the end, or before a final line terminator
    /// (`\r\n`, or one of `\n \r \u{85} \u{2028} \u{2029}`, never between `\r` and `\n`).
    /// With `unix` (`(?d)`), only `\n` terminates lines.
    JavaDollar {
        unix: bool,
    },
    /// Java multi-line `$`: before any line terminator (not between `\r\n`) or at the end.
    JavaLineEnd {
        unix: bool,
    },
    /// Java multi-line `^`: at the start or after a line terminator (not between `\r\n`), and
    /// never at the very end of the input.
    JavaLineStart {
        unix: bool,
    },
    WordBoundary {
        word: WordSet,
    },
    NotWordBoundary {
        word: WordSet,
    },
    /// `\<` / `\b{start}`: word char on the right, non-word on the left.
    WordStart {
        word: WordSet,
    },
    /// `\>` / `\b{end}`.
    WordEnd {
        word: WordSet,
    },
    /// `\b{start-half}`: non-word (or nothing) on the left.
    WordStartHalf {
        word: WordSet,
    },
    /// `\b{end-half}`: non-word (or nothing) on the right.
    WordEndHalf {
        word: WordSet,
    },
}

/// The word-character set behind `\b`, `\B` and friends; each engine defines `\w` differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WordSet {
    /// `[0-9A-Za-z_]` (JS, Go, Java; Rust `-u`; Python `a`).
    Ascii,
    /// UTS #18 word characters (Rust Unicode mode).
    Rust,
    /// CPython's `\w` (`str.isalnum()` or `_`), measured.
    Python,
    /// JS `\w` under `iu`: ASCII word chars plus U+017F and U+212A.
    JsIgnoreCaseUnicode,
    /// Java: ASCII word chars, plus a non-spacing mark that follows (through other marks) a
    /// Unicode letter or digit. Context-dependent, so lowering builds it specially.
    Java,
}

impl WordSet {
    pub fn rust(unicode: bool) -> WordSet {
        if unicode {
            WordSet::Rust
        } else {
            WordSet::Ascii
        }
    }

    pub fn python(unicode: bool) -> WordSet {
        if unicode {
            WordSet::Python
        } else {
            WordSet::Ascii
        }
    }

    pub fn chars(self) -> CharSet {
        match self {
            WordSet::Ascii => crate::unicode::posix_class("word").unwrap_or_default(),
            WordSet::Rust => crate::unicode::perl_word().clone(),
            WordSet::Python => crate::tables::python_word().clone(),
            WordSet::JsIgnoreCaseUnicode => crate::tables::js_word_iu().clone(),
            // The context-free part; `regular` adds the mark rule.
            WordSet::Java => crate::unicode::posix_class("word").unwrap_or_default(),
        }
    }
}

/// How a backreference compares characters (the flags at the reference).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BackrefCase {
    Exact,
    /// Python `(?i)`: the measured case-insensitive relation (ASCII-only under `(?a)`).
    Python {
        ascii: bool,
    },
    /// JS `i`: Canonicalize (toUpperCase-based without `u`, simple case folding with `u`).
    Js {
        unicode: bool,
    },
    /// Java `(?i)`: ASCII, or `toUpperCase`/`toLowerCase` comparison with `(?u)`.
    Java {
        unicode: bool,
    },
}

/// Which characters end a line for multi-line `^` / `$`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineTerms {
    /// `\n` only (Rust, Go, Python).
    Lf,
    /// `\r` or `\n`, but never between the two of a `\r\n` (Rust `R` flag).
    CrLf,
    /// Each of `\n \r \u{2028} \u{2029}` (ECMAScript LineTerminator).
    Js,
}

impl LineTerms {
    /// The terminator characters, when a single character always ends a line.
    pub fn chars(self) -> Option<&'static [char]> {
        match self {
            LineTerms::Lf => Some(&['\n']),
            LineTerms::Js => Some(&['\n', '\r', '\u{2028}', '\u{2029}']),
            LineTerms::CrLf => None,
        }
    }
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
            | NodeKind::Backref(..) => {}
        }
    }

    /// (minimum, maximum) number of characters this node can consume; `None` = unbounded.
    /// A backreference is (0, unbounded): its width depends on what the group matched.
    pub fn width(&self) -> (usize, Option<usize>) {
        match &self.kind {
            NodeKind::Empty | NodeKind::Assert(_) => (0, Some(0)),
            NodeKind::Literal(_) => (1, Some(1)),
            NodeKind::Class(_) => (1, Some(1)),
            NodeKind::Group(g) => g.sub.width(),
            NodeKind::Look(_) => (0, Some(0)),
            // Unknown statically: the referenced group can match anything.
            NodeKind::Backref(..) => (0, None),
            NodeKind::Conditional(c) => {
                let y = c.yes.width();
                let n = c.no.as_ref().map_or((0, Some(0)), |n| n.width());
                (y.0.min(n.0), y.1.zip(n.1).map(|(a, b)| a.max(b)))
            }
            NodeKind::Concat(xs) => xs.iter().fold((0, Some(0)), |(a, b), x| {
                let (c, d) = x.width();
                (
                    a.saturating_add(c),
                    b.zip(d).map(|(b, d)| b.saturating_add(d)),
                )
            }),
            NodeKind::Alt(xs) => {
                let ws: Vec<_> = xs.iter().map(Node::width).collect();
                let min = ws.iter().map(|w| w.0).min().unwrap_or(0);
                let max = ws.iter().try_fold(0usize, |acc, w| w.1.map(|m| acc.max(m)));
                (min, max)
            }
            NodeKind::Repeat(r) => {
                let (a, b) = r.sub.width();
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
            NodeKind::Backref(i, _) => {
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
            NodeKind::Backref(i, _) => write!(f, "\\{i}"),
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
