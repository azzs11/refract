//! IR → pattern text in a target flavour (plan §5.8). The printer is best-effort and never
//! trusted: `refract translate` re-parses its output in the target and proves it equivalent to
//! the source (translation validation, ADR-0013). A construct with no form in the target is an
//! `Unsupported` error naming it.
//!
//! Character sets are printed from their resolved ranges (the IR has no `\w` left), so the
//! output carries no flags except JavaScript's `u` (code-point semantics).

use crate::charset::{CharSet, MAX_CHAR};
use crate::error::{Error, Result};
use crate::flavor::Flavor;
use crate::ir::{Assertion, Group, LineTerms, Look, Node, NodeKind, Regex, Repeat, WordSet};
use std::fmt::Write as _;

/// Prints `re` for `target`. JavaScript output is a literal `/source/u`.
pub fn print(re: &Regex, target: Flavor) -> Result<String> {
    if re.domain == crate::ir::Domain::Utf16 {
        // Code-unit sets (JS without `u`) only print as code units; translate from `u` form.
        let mut pseudo = false;
        walk(&re.root, &mut |n| {
            if let NodeKind::Class(s) = &n.kind {
                pseudo |= s.ranges().last().is_some_and(|r| r.1 > MAX_CHAR);
            }
        });
        if pseudo {
            return Err(Error::unsupported(
                re.root.span,
                "a set of UTF-16 code units (JavaScript without u) has no code-point form",
            ));
        }
    }
    let p = Printer {
        t: target,
        src: re.flavor,
        groups: re.captures.len() as u32,
        named: named_classes(target),
        refs_always_set: refs_set(&re.root, &Default::default()).0,
        open: Default::default(),
    };
    let body = p.node(&re.root, Ctx::Top)?;
    Ok(match target {
        Flavor::JavaScript => format!("/{body}/u"),
        _ => body,
    })
}

fn walk(n: &Node, f: &mut dyn FnMut(&Node)) {
    f(n);
    match &n.kind {
        NodeKind::Concat(xs) | NodeKind::Alt(xs) => xs.iter().for_each(|x| walk(x, f)),
        NodeKind::Repeat(r) => walk(&r.sub, f),
        NodeKind::Group(g) => walk(&g.sub, f),
        NodeKind::Look(l) => walk(&l.sub, f),
        NodeKind::Conditional(c) => {
            walk(&c.yes, f);
            if let Some(x) = &c.no {
                walk(x, f);
            }
        }
        _ => {}
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Ctx {
    Top,
    /// Inside a concatenation: alternations need a group.
    Seq,
    /// Operand of a quantifier: anything but a single atom needs a group.
    Atom,
}

struct Printer {
    t: Flavor,
    src: Flavor,
    groups: u32,
    /// The target's own shorthand classes with the sets *it* gives them (parsed by the
    /// target's front-end), so `\\w` is only printed where it means the same set.
    named: Vec<(&'static str, CharSet)>,
    /// Every back-reference refers to a group that has surely matched when it is reached, so
    /// JS's rule for unset groups (match empty) and everyone else's (fail) never apply.
    refs_always_set: bool,
    /// Capture groups being printed (a reference to one is a reference from inside itself).
    open: std::cell::RefCell<Vec<u32>>,
}

/// (no back-reference can meet an unset group, groups surely set after `n`).
fn refs_set(
    n: &Node,
    before: &std::collections::BTreeSet<u32>,
) -> (bool, std::collections::BTreeSet<u32>) {
    match &n.kind {
        NodeKind::Backref(i, _) => (before.contains(i), before.clone()),
        NodeKind::Concat(xs) => {
            let mut cur = before.clone();
            let mut ok = true;
            for x in xs {
                let (o, after) = refs_set(x, &cur);
                ok &= o;
                cur = after;
            }
            (ok, cur)
        }
        NodeKind::Alt(xs) => {
            let mut ok = true;
            let mut common: Option<std::collections::BTreeSet<u32>> = None;
            for x in xs {
                let (o, after) = refs_set(x, before);
                ok &= o;
                common = Some(match common {
                    None => after,
                    Some(c) => c.intersection(&after).copied().collect(),
                });
            }
            (ok, common.unwrap_or_else(|| before.clone()))
        }
        NodeKind::Repeat(r) => {
            let (ok, after) = refs_set(&r.sub, before);
            (ok, if r.min >= 1 { after } else { before.clone() })
        }
        NodeKind::Group(g) => {
            let (ok, mut after) = refs_set(&g.sub, before);
            if let Some(c) = &g.capture {
                after.insert(c.index);
            }
            (ok, after)
        }
        NodeKind::Look(l) => (refs_set(&l.sub, before).0, before.clone()),
        NodeKind::Conditional(_) => (false, before.clone()),
        _ => (true, before.clone()),
    }
}

/// Shorthands and Unicode properties, as the target engine resolves them.
fn named_classes(target: Flavor) -> Vec<(&'static str, CharSet)> {
    const NAMES: &[&str] = &[
        "\\w", "\\d", "\\s", "\\p{L}", "\\p{N}", "\\p{M}", "\\p{P}", "\\p{S}", "\\p{Z}", "\\p{Lu}",
        "\\p{Ll}", "\\p{Lo}", "\\p{Nd}", "\\p{Mn}",
    ];
    NAMES
        .iter()
        .filter_map(|&name| {
            let text = match target {
                Flavor::JavaScript => format!("/[{name}]/u"),
                _ => format!("[{name}]"),
            };
            let re = crate::flavor::parse(target, &text).ok()?;
            match &re.root.kind {
                NodeKind::Class(set) if !set.is_empty() => Some((name, set.clone())),
                _ => None,
            }
        })
        .collect()
}

impl Printer {
    fn unsupported(&self, n: &Node, what: &str) -> Error {
        Error::unsupported(n.span, format!("{} has no {what}", self.t))
    }

    fn lookaround(&self) -> bool {
        !matches!(self.t, Flavor::Go | Flavor::Rust)
    }

    fn node(&self, n: &Node, ctx: Ctx) -> Result<String> {
        let s = match &n.kind {
            NodeKind::Empty => {
                return Ok(if ctx == Ctx::Top {
                    String::new()
                } else {
                    "(?:)".into()
                });
            }
            NodeKind::Literal(c) => self.char(*c as u32, false),
            NodeKind::Class(set) => self.set(set),
            NodeKind::Concat(xs) => {
                let mut out = String::new();
                for x in xs {
                    out.push_str(&self.node(x, Ctx::Seq)?);
                }
                return Ok(if ctx == Ctx::Atom && xs.len() > 1 {
                    format!("(?:{out})")
                } else {
                    out
                });
            }
            NodeKind::Alt(xs) => {
                let parts: Result<Vec<String>> =
                    xs.iter().map(|x| self.node(x, Ctx::Top)).collect();
                let out = parts?.join("|");
                return Ok(if ctx == Ctx::Top {
                    out
                } else {
                    format!("(?:{out})")
                });
            }
            NodeKind::Repeat(r) => {
                let q = self.repeat(n, r)?;
                return Ok(if ctx == Ctx::Atom {
                    format!("(?:{q})")
                } else {
                    q
                });
            }
            NodeKind::Group(g) => self.group(n, g)?,
            NodeKind::Assert(a) => self.assert(n, *a)?,
            NodeKind::Look(l) => self.look(n, l)?,
            NodeKind::Backref(i, case) => {
                if matches!(self.t, Flavor::Go | Flavor::Rust) {
                    return Err(
                        self.unsupported(n, "back-references (bounded check only in the source)")
                    );
                }
                if (self.src == Flavor::JavaScript) != (self.t == Flavor::JavaScript)
                    && !self.refs_always_set
                {
                    // An unset group matches the empty string in JS and fails elsewhere.
                    return Err(self.unsupported(
                        n,
                        "back-references with the source's rule for unset groups",
                    ));
                }
                if self.t == Flavor::Python && self.open.borrow().contains(i) {
                    return Err(self.unsupported(n, "references to a group from inside itself"));
                }
                if *i > self.groups {
                    // Only reachable outside JS (where it is a syntax error): it never matches.
                    return Ok("(?!)".into());
                }
                if *case != crate::ir::BackrefCase::Exact {
                    return Err(
                        self.unsupported(n, "form for this case-insensitive back-reference")
                    );
                }
                // `(?:)` after the number stops a following digit joining it.
                format!("\\{i}(?:)")
            }
            NodeKind::Conditional(c) => {
                if self.t != Flavor::Python {
                    return Err(self.unsupported(n, "conditional groups"));
                }
                let yes = self.node(&c.yes, Ctx::Top)?;
                let no = match &c.no {
                    Some(x) => format!("|{}", self.node(x, Ctx::Top)?),
                    None => String::new(),
                };
                format!("(?({}){yes}{no})", c.group)
            }
        };
        Ok(s)
    }

    fn group(&self, n: &Node, g: &Group) -> Result<String> {
        if let Some(c) = &g.capture {
            self.open.borrow_mut().push(c.index);
        }
        let body = self.node(&g.sub, Ctx::Top);
        if g.capture.is_some() {
            self.open.borrow_mut().pop();
        }
        let body = body?;
        if g.atomic {
            if !matches!(self.t, Flavor::Python | Flavor::Java) {
                return Err(self.unsupported(n, "atomic groups"));
            }
            return Ok(format!("(?>{body})"));
        }
        Ok(match &g.capture {
            None => format!("(?:{body})"),
            Some(c) => match &c.name {
                None => format!("({body})"),
                Some(name) => match self.t {
                    Flavor::Python | Flavor::Go | Flavor::Rust => format!("(?P<{name}>{body})"),
                    _ => format!("(?<{name}>{body})"),
                },
            },
        })
    }

    fn repeat(&self, n: &Node, r: &Repeat) -> Result<String> {
        let mut atom = self.node(&r.sub, Ctx::Atom)?;
        if matches!(r.sub.kind, NodeKind::Assert(_) | NodeKind::Look(_)) && !atom.starts_with("(?:")
        {
            // Quantified assertions are syntax errors in some engines (JS `u`); a group is not.
            atom = format!("(?:{atom})");
        }
        let mut q = match (r.min, r.max) {
            (0, None) => "*".to_string(),
            (1, None) => "+".to_string(),
            (0, Some(1)) => "?".to_string(),
            (a, None) => format!("{{{a},}}"),
            (a, Some(b)) if a == b => format!("{{{a}}}"),
            (a, Some(b)) => format!("{{{a},{b}}}"),
        };
        if matches!(self.t, Flavor::Go | Flavor::Rust) && r.max.unwrap_or(r.min) > 1000 {
            return Err(self.unsupported(n, "repetition counts above 1000"));
        }
        if r.possessive {
            if !matches!(self.t, Flavor::Python | Flavor::Java) {
                return Err(self.unsupported(n, "possessive quantifiers"));
            }
            q.push('+');
        } else if !r.greedy {
            q.push('?');
        }
        Ok(format!("{atom}{q}"))
    }

    fn text_start(&self) -> &'static str {
        match self.t {
            Flavor::JavaScript => "^",
            _ => "\\A",
        }
    }

    fn text_end(&self) -> &'static str {
        match self.t {
            Flavor::JavaScript => "$",
            Flavor::Python => "\\Z",
            _ => "\\z",
        }
    }

    fn term_class(&self, terms: LineTerms) -> Option<String> {
        terms
            .chars()
            .map(|cs| self.set(&CharSet::from_iter(cs.iter().copied())))
    }

    fn assert(&self, n: &Node, a: Assertion) -> Result<String> {
        use Assertion::*;
        let look_needed = |s: String| -> Result<String> {
            if self.lookaround() {
                Ok(s)
            } else {
                Err(self.unsupported(n, "look-around, which this assertion needs"))
            }
        };
        Ok(match a {
            TextStart => self.text_start().into(),
            TextEnd => self.text_end().into(),
            TextEndOrFinalNewline => match self.t {
                Flavor::Python => "$".into(),
                _ => look_needed(format!("(?=\\n?{})", self.text_end()))?,
            },
            LineStart { terms } => match (self.t, terms) {
                (Flavor::Python | Flavor::Go | Flavor::Rust, LineTerms::Lf)
                | (Flavor::JavaScript, LineTerms::Js) => "(?m:^)".into(),
                (Flavor::Rust, LineTerms::CrLf) => "(?mR:^)".into(),
                _ => {
                    let cls = self
                        .term_class(terms)
                        .ok_or_else(|| self.unsupported(n, "form for this line start"))?;
                    look_needed(format!("(?:{}|(?<={cls}))", self.text_start()))?
                }
            },
            LineEnd { terms } => match (self.t, terms) {
                (Flavor::Python | Flavor::Go | Flavor::Rust, LineTerms::Lf)
                | (Flavor::JavaScript, LineTerms::Js) => "(?m:$)".into(),
                (Flavor::Rust, LineTerms::CrLf) => "(?mR:$)".into(),
                _ => {
                    let cls = self
                        .term_class(terms)
                        .ok_or_else(|| self.unsupported(n, "form for this line end"))?;
                    look_needed(format!("(?={}|{cls})", self.text_end()))?
                }
            },
            JavaDollar { unix } => match self.t {
                Flavor::Java => {
                    if unix {
                        "(?d:$)".into()
                    } else {
                        "$".into()
                    }
                }
                _ => return Err(self.unsupported(n, "form of Java's `$`")),
            },
            JavaLineEnd { unix } => match self.t {
                Flavor::Java => {
                    if unix {
                        "(?md:$)".into()
                    } else {
                        "(?m:$)".into()
                    }
                }
                _ => return Err(self.unsupported(n, "form of Java's `(?m)$`")),
            },
            JavaLineStart { unix } => match self.t {
                Flavor::Java => {
                    if unix {
                        "(?md:^)".into()
                    } else {
                        "(?m:^)".into()
                    }
                }
                _ => return Err(self.unsupported(n, "form of Java's `(?m)^`")),
            },
            WordBoundary { word } => self.word_assert(n, word, false)?,
            NotWordBoundary { word } => self.word_assert(n, word, true)?,
            WordStart { .. } | WordEnd { .. } | WordStartHalf { .. } | WordEndHalf { .. } => {
                return Err(self.unsupported(n, "form for this word assertion"));
            }
        })
    }

    /// `\b` / `\B` over `word`: the target's own form when its word set is the same, else
    /// look-around over the explicit set.
    fn word_assert(&self, n: &Node, word: WordSet, negated: bool) -> Result<String> {
        let b = if negated { "\\B" } else { "\\b" };
        let native = match (self.t, word) {
            (Flavor::JavaScript | Flavor::Go, WordSet::Ascii)
            | (Flavor::Python, WordSet::Python)
            | (Flavor::Java, WordSet::Java)
            | (Flavor::Rust, WordSet::Rust) => Some(b.to_string()),
            (Flavor::Python, WordSet::Ascii) => Some(format!("(?a:{b})")),
            (Flavor::Rust, WordSet::Ascii) => Some(format!("(?-u:{b})")),
            _ => None,
        };
        if let Some(s) = native {
            return Ok(s);
        }
        if word == WordSet::Java || !self.lookaround() {
            return Err(self.unsupported(n, "word boundary over this source's word characters"));
        }
        let w = self.set(&word.chars());
        Ok(if negated {
            format!("(?:(?<={w})(?={w})|(?<!{w})(?!{w}))")
        } else {
            format!("(?:(?<={w})(?!{w})|(?<!{w})(?={w}))")
        })
    }

    fn look(&self, n: &Node, l: &Look) -> Result<String> {
        if !self.lookaround() {
            return Err(self.unsupported(n, "look-around"));
        }
        if l.behind {
            let (lo, hi) = l.sub.width();
            if self.t == Flavor::Python && hi != Some(lo) {
                return Err(self.unsupported(n, "variable-width look-behind"));
            }
            if self.t == Flavor::Java && hi.is_none() {
                return Err(self.unsupported(n, "unbounded look-behind"));
            }
        }
        let body = self.node(&l.sub, Ctx::Top)?;
        let open = match (l.behind, l.negated) {
            (false, false) => "(?=",
            (false, true) => "(?!",
            (true, false) => "(?<=",
            (true, true) => "(?<!",
        };
        Ok(format!("{open}{body})"))
    }

    /// One code point, outside (`in_class = false`) or inside a class.
    fn char(&self, c: u32, in_class: bool) -> String {
        let safe = char::from_u32(c).is_some_and(|ch| {
            ch.is_ascii_alphanumeric()
                || ch == '_'
                || (!in_class && " ,:;<=>@!\"'%~#".contains(ch))
                || (in_class
                    && " ,:;<=>@!\"'%~#.*+?()|{}$/".contains(ch)
                    && self.t != Flavor::JavaScript)
        });
        if safe && !(self.t == Flavor::JavaScript && c == '/' as u32) {
            return char::from_u32(c).map(String::from).unwrap_or_default();
        }
        // Syntax characters escape the same way in every flavour (JS `u` included).
        if char::from_u32(c)
            .is_some_and(|ch| "^$\\.*+?()[]{}|/".contains(ch) || (in_class && ch == '-'))
        {
            return format!("\\{}", char::from_u32(c).unwrap_or_default());
        }
        match self.t {
            Flavor::JavaScript => format!("\\u{{{c:X}}}"),
            Flavor::Python => {
                if c <= 0xFF {
                    format!("\\x{c:02x}")
                } else if c <= 0xFFFF {
                    format!("\\u{c:04x}")
                } else {
                    format!("\\U{c:08x}")
                }
            }
            _ => format!("\\x{{{c:X}}}"),
        }
    }

    fn set(&self, set: &CharSet) -> String {
        let full = CharSet::full();
        if set.is_empty() {
            return match self.t {
                Flavor::Go | Flavor::Rust => "[^\\x{0}-\\x{10FFFF}]".into(),
                _ => "(?!)".into(),
            };
        }
        if set == &full {
            return match self.t {
                Flavor::JavaScript => "[\\s\\S]".into(),
                _ => "(?s:.)".into(),
            };
        }
        let r = set.ranges();
        if r.len() == 1 && r[0].0 == r[0].1 {
            return self.char(r[0].0, false);
        }
        if let Some((name, _)) = self.named.iter().find(|(_, n)| n == set) {
            return (*name).to_string();
        }
        let pos = self.class_body(set);
        let neg = self.class_body(&set.complement());
        let mut out = if neg.len() < pos.len() {
            format!("[^{neg}")
        } else {
            format!("[{pos}")
        };
        let _ = write!(out, "]");
        out
    }

    /// The inside of a class for `set`: target shorthands that are subsets of it (chosen
    /// greedily while they shorten the text), then the remaining ranges.
    fn class_body(&self, set: &CharSet) -> String {
        let mut rest = set.clone();
        let mut out = String::new();
        loop {
            let best = self
                .named
                .iter()
                .filter(|(_, n)| n.is_subset(set) && rest.intersects(n))
                .map(|(name, n)| (rest.difference(n).ranges().len() + name.len() / 4, name, n))
                .min_by_key(|x| x.0);
            match best {
                Some((cost, name, n)) if cost < rest.ranges().len() => {
                    out.push_str(name);
                    rest = rest.difference(n);
                }
                _ => break,
            }
        }
        for &(lo, hi) in rest.ranges() {
            // Surrogates are not scalar values: a range across them is printed in two.
            let parts: Vec<(u32, u32)> = if lo < 0xD800 && hi > 0xDFFF {
                vec![(lo, 0xD7FF), (0xE000, hi)]
            } else {
                vec![(lo, hi)]
            };
            for (a, b) in parts {
                out.push_str(&self.char(a, true));
                if b > a {
                    if b > a + 1 {
                        out.push('-');
                    }
                    out.push_str(&self.char(b, true));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Budget, Op, equiv::Verdict, parse};

    /// Print in every target and prove the result equivalent (or report why not).
    fn roundtrip(
        from: Flavor,
        p: &str,
        to: Flavor,
        op: Op,
    ) -> std::result::Result<Verdict, String> {
        let src = parse(from, p).map_err(|e| e.message)?;
        let text = print(&src, to).map_err(|e| format!("unsupported: {}", e.message))?;
        let dst = parse(to, &text).map_err(|e| format!("PRINTER BUG {text}: {}", e.message))?;
        let rep =
            crate::equiv::compare(&src, &dst, op, &Budget::default()).map_err(|e| e.message)?;
        Ok(rep.verdict)
    }

    #[test]
    fn common_patterns_translate_exactly() {
        let all = [
            Flavor::JavaScript,
            Flavor::Python,
            Flavor::Go,
            Flavor::Java,
            Flavor::Rust,
        ];
        for p in [
            r"\d{3}-\d{4}",
            r"[a-z_][a-z0-9_]*",
            r"(?:ab|cd)+e?",
            r"[^,\n]+",
            r"x{2,5}?y",
            r"(a)(?:b|c)",
        ] {
            for from in [Flavor::Python, Flavor::Rust] {
                for to in all {
                    let v = roundtrip(from, p, to, Op::FullMatch);
                    assert_eq!(v, Ok(Verdict::Equivalent), "{from}:{p} -> {to}");
                }
            }
        }
    }

    #[test]
    fn resolved_semantics_travel() {
        // Python's Unicode \w becomes explicit ranges: still the same language in JS.
        assert_eq!(
            roundtrip(Flavor::Python, r"^\w+$", Flavor::JavaScript, Op::FullMatch),
            Ok(Verdict::Equivalent)
        );
        // Python's `$` (before a final \n) needs a look-ahead elsewhere.
        assert_eq!(
            roundtrip(Flavor::Python, r"^a$", Flavor::Java, Op::Search),
            Ok(Verdict::Equivalent)
        );
        assert!(
            roundtrip(Flavor::Python, r"^a$", Flavor::Go, Op::Search)
                .unwrap_err()
                .contains("look-around")
        );
    }

    #[test]
    fn printed_output_always_parses() {
        let all = [
            Flavor::JavaScript,
            Flavor::Python,
            Flavor::Go,
            Flavor::Java,
            Flavor::Rust,
        ];
        for p in [
            r"(?i)straße",
            r"[\s\S]",
            r"\bword\b",
            r"(?<=a)b(?!c)",
            r"(?m)^x$",
            r"[\u{1F600}-\u{1F64F}]",
            r"[\[\]\\^-]",
            r"a/b",
        ] {
            for to in all {
                if let Err(e) = roundtrip(Flavor::Rust, p, to, Op::Search) {
                    assert!(!e.contains("PRINTER BUG"), "rust:{p} -> {to}: {e}");
                }
            }
        }
    }
}
