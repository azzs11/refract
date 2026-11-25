//! Finds regex literals in source code (plan §4, `refract check`). Each language gets a small
//! lexer that skips comments and strings correctly, then call shapes that take a pattern are
//! matched on the token stream:
//!
//! - Python: `re.compile|search|match|fullmatch|findall|finditer|sub|subn|split(<str>`
//! - JavaScript/TypeScript: `/regex/flags` literals, `new RegExp(<str>[, <str>])`
//! - Go: `regexp.MustCompile|Compile|MatchString(<str>` (`CompilePOSIX` is reported unsupported)
//! - Java: `Pattern.compile|matches(<str>`, and `.matches|replaceAll|replaceFirst|split(<str>`
//! - Rust: `Regex::new(<str>`, `RegexBuilder::new(<str>`, `RegexSet::new([<str>, ...])`
//!
//! A comment containing `refract: <flavor>` marks the next string literal (same or next line) as
//! a pattern in that flavour, for patterns built or stored elsewhere (plan §14).
//!
//! String escapes are decoded with the host language's rules, so the pattern is what the engine
//! receives.

use crate::flavor::Flavor;
use crate::regular::Op;

/// A source language Refract can scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Python,
    JavaScript,
    Go,
    Java,
    Rust,
}

impl Lang {
    /// By file extension.
    pub fn from_path(path: &str) -> Option<Lang> {
        let ext = path.rsplit('.').next()?;
        Some(match ext {
            "py" | "pyi" => Lang::Python,
            "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" => Lang::JavaScript,
            "go" => Lang::Go,
            "java" => Lang::Java,
            "rs" => Lang::Rust,
            _ => return None,
        })
    }

    pub fn flavor(self) -> Flavor {
        match self {
            Lang::Python => Flavor::Python,
            Lang::JavaScript => Flavor::JavaScript,
            Lang::Go => Flavor::Go,
            Lang::Java => Flavor::Java,
            Lang::Rust => Flavor::Rust,
        }
    }
}

/// One regex found in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The pattern as the engine receives it (JS: `/source/flags`).
    pub pattern: String,
    pub flavor: Flavor,
    /// The operation the call performs (`re.fullmatch`, Java `matches` → full match).
    pub op: Op,
    /// 1-based line and column (in characters) of the literal.
    pub line: usize,
    pub col: usize,
    /// The call it was found in, e.g. `re.search`.
    pub call: String,
    /// Set when the call uses an engine mode Refract does not model.
    pub unsupported: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Punct(char),
    /// Decoded string value; `None` when it cannot be decoded statically (f-strings, templates
    /// with substitutions).
    Str(Option<String>),
    /// JS `/source/flags`, verbatim.
    Regex(String),
    /// `refract: <flavor>` directive comment.
    Directive(Flavor),
}

#[derive(Clone, Debug)]
struct T {
    tok: Tok,
    line: usize,
    col: usize,
}

/// All regexes in `src`, a file in `lang`.
pub fn extract(src: &str, lang: Lang) -> Vec<Found> {
    let toks = lex(src, lang);
    let mut out = Vec::new();
    let flavor = lang.flavor();
    let ident = |i: usize| match toks.get(i).map(|t| &t.tok) {
        Some(Tok::Ident(s)) => Some(s.as_str()),
        _ => None,
    };
    let punct =
        |i: usize, c: char| matches!(toks.get(i).map(|t| &t.tok), Some(Tok::Punct(p)) if *p == c);
    let string = |i: usize| match toks.get(i) {
        Some(T {
            tok: Tok::Str(Some(s)),
            line,
            col,
        }) => Some((s.clone(), *line, *col)),
        _ => None,
    };
    let mut directed = std::collections::HashSet::new();
    for (i, t) in toks.iter().enumerate() {
        if let Tok::Directive(f) = t.tok {
            // The next string literal within the next line.
            if let Some((j, s)) = toks[i + 1..]
                .iter()
                .enumerate()
                .find(|(_, u)| matches!(u.tok, Tok::Str(_)))
                .map(|(k, u)| (i + 1 + k, u))
                && s.line <= t.line + 1
                && let Some((p, line, col)) = string(j)
            {
                directed.insert(j);
                let pattern = if f == Flavor::JavaScript && !p.starts_with('/') {
                    format!("/{p}/")
                } else {
                    p
                };
                out.push(Found {
                    pattern,
                    flavor: f,
                    op: Op::Search,
                    line,
                    col,
                    call: "refract: directive".into(),
                    unsupported: None,
                });
            }
        }
    }
    // `flags_at`: the call's `(` and the position of its flags argument (Python, Java).
    let push = |out: &mut Vec<Found>,
                j: usize,
                call: String,
                op: Op,
                unsupported: Option<String>,
                pattern_of: &dyn Fn(String) -> String,
                flags_at: Option<(usize, usize)>| {
        if directed.contains(&j) {
            return;
        }
        let Some((line, col)) = toks.get(j).map(|t| (t.line, t.col)) else {
            return;
        };
        let Some((p, partial)) = literal(&toks, j, lang) else {
            return;
        };
        let mut unsupported = unsupported;
        if partial && unsupported.is_none() {
            unsupported =
                Some("the pattern is built at run time (concatenation or formatting)".into());
        }
        let mut inline = String::new();
        if let Some((open, pos)) = flags_at {
            match flags(&toks, open, pos, lang) {
                Ok(f) => inline = f,
                Err(why) => {
                    if unsupported.is_none() {
                        unsupported = Some(why);
                    }
                }
            }
        }
        let p = pattern_of(p);
        out.push(Found {
            pattern: if inline.is_empty() {
                p
            } else {
                format!("(?{inline}){p}")
            },
            flavor,
            op,
            line,
            col,
            call,
            unsupported,
        });
    };
    let same = |p: String| p;
    for i in 0..toks.len() {
        match lang {
            Lang::Python => {
                if ident(i) == Some("re")
                    && punct(i + 1, '.')
                    && let Some(f) = ident(i + 2)
                    && matches!(
                        f,
                        "compile"
                            | "search"
                            | "match"
                            | "fullmatch"
                            | "findall"
                            | "finditer"
                            | "sub"
                            | "subn"
                            | "split"
                    )
                    && punct(i + 3, '(')
                {
                    let op = if f == "fullmatch" {
                        Op::FullMatch
                    } else {
                        Op::Search
                    };
                    // `re.match` anchors at the start only; analysed as search (a superset).
                    let pos = match f {
                        "compile" => 1,
                        "split" => 3,
                        "sub" | "subn" => 4,
                        _ => 2,
                    };
                    push(
                        &mut out,
                        i + 4,
                        format!("re.{f}"),
                        op,
                        None,
                        &same,
                        Some((i + 3, pos)),
                    );
                }
            }
            Lang::JavaScript => {
                if let Tok::Regex(r) = &toks[i].tok
                    && !directed.contains(&i)
                {
                    out.push(Found {
                        pattern: r.clone(),
                        flavor,
                        op: Op::Search,
                        line: toks[i].line,
                        col: toks[i].col,
                        call: "regex literal".into(),
                        unsupported: None,
                    });
                }
                if ident(i) == Some("RegExp") && punct(i + 1, '(') {
                    let flags = if punct(i + 3, ',') {
                        string(i + 4).map(|s| s.0)
                    } else {
                        Some(String::new())
                    };
                    if let Some(flags) = flags {
                        push(
                            &mut out,
                            i + 2,
                            "new RegExp".into(),
                            Op::Search,
                            None,
                            &|p: String| format!("/{p}/{flags}"),
                            None,
                        );
                    }
                }
            }
            Lang::Go => {
                if ident(i) == Some("regexp")
                    && punct(i + 1, '.')
                    && let Some(f) = ident(i + 2)
                    && punct(i + 3, '(')
                {
                    match f {
                        "MustCompile" | "Compile" | "MatchString" | "Match" => {
                            push(
                                &mut out,
                                i + 4,
                                format!("regexp.{f}"),
                                Op::Search,
                                None,
                                &same,
                                None,
                            );
                        }
                        "MustCompilePOSIX" | "CompilePOSIX" => push(
                            &mut out,
                            i + 4,
                            format!("regexp.{f}"),
                            Op::Search,
                            Some("POSIX leftmost-longest syntax is not modelled".into()),
                            &same,
                            None,
                        ),
                        _ => {}
                    }
                }
            }
            Lang::Java => {
                if ident(i) == Some("Pattern")
                    && punct(i + 1, '.')
                    && let Some(f) = ident(i + 2)
                    && matches!(f, "compile" | "matches")
                    && punct(i + 3, '(')
                {
                    let op = if f == "matches" {
                        Op::FullMatch
                    } else {
                        Op::Search
                    };
                    let flags_at = (f == "compile").then_some((i + 3, 1));
                    push(
                        &mut out,
                        i + 4,
                        format!("Pattern.{f}"),
                        op,
                        None,
                        &same,
                        flags_at,
                    );
                } else if punct(i, '.')
                    && ident(i.wrapping_sub(1)) != Some("Pattern")
                    && let Some(f) = ident(i + 1)
                    && matches!(f, "matches" | "replaceAll" | "replaceFirst" | "split")
                    && punct(i + 2, '(')
                {
                    let op = if f == "matches" {
                        Op::FullMatch
                    } else {
                        Op::Search
                    };
                    push(
                        &mut out,
                        i + 3,
                        format!("String.{f}"),
                        op,
                        None,
                        &same,
                        None,
                    );
                }
            }
            Lang::Rust => {
                if let Some(ty) = ident(i)
                    && matches!(ty, "Regex" | "RegexBuilder")
                    && punct(i + 1, ':')
                    && punct(i + 2, ':')
                    && ident(i + 3) == Some("new")
                    && punct(i + 4, '(')
                {
                    push(
                        &mut out,
                        i + 5,
                        format!("{ty}::new"),
                        Op::Search,
                        None,
                        &same,
                        None,
                    );
                }
                if ident(i) == Some("RegexSet")
                    && punct(i + 1, ':')
                    && punct(i + 2, ':')
                    && ident(i + 3) == Some("new")
                {
                    let mut j = i + 4;
                    while j < toks.len() && !punct(j, ')') {
                        if string(j).is_some() {
                            push(
                                &mut out,
                                j,
                                "RegexSet::new".into(),
                                Op::Search,
                                None,
                                &same,
                                None,
                            );
                        }
                        j += 1;
                    }
                }
            }
        }
    }
    out.sort_by_key(|f| (f.line, f.col));
    out.dedup_by(|a, b| a.line == b.line && a.col == b.col);
    out
}

/// The string starting at token `j`, joined with adjacent (Python) or `+`-concatenated literals,
/// and whether it is only part of the pattern (concatenated with or formatted by run-time values).
fn literal(toks: &[T], j: usize, lang: Lang) -> Option<(String, bool)> {
    let mut s = match &toks.get(j)?.tok {
        Tok::Str(Some(s)) => s.clone(),
        Tok::Str(None) => return Some((String::new(), true)),
        _ => return None,
    };
    let mut k = j + 1;
    loop {
        match toks.get(k).map(|t| &t.tok) {
            Some(Tok::Str(Some(t))) if lang == Lang::Python => {
                s.push_str(t);
                k += 1;
            }
            Some(Tok::Punct('+')) => match toks.get(k + 1).map(|t| &t.tok) {
                Some(Tok::Str(Some(t))) => {
                    s.push_str(t);
                    k += 2;
                }
                _ => return Some((s, true)),
            },
            Some(Tok::Punct('%')) if lang == Lang::Python => return Some((s, true)),
            Some(Tok::Punct('.')) => {
                // `"...".format(...)`, `"...".formatted(...)`, `"..." .replace(...)`.
                return Some((s, true));
            }
            Some(Tok::Str(None)) => return Some((s, true)),
            _ => return Some((s, false)),
        }
    }
}

/// Inline flag letters for the call's flags argument (`pos`, 0-based, or the `flags=` keyword),
/// or why it cannot be read statically. No flags argument → no letters.
fn flags(toks: &[T], open: usize, pos: usize, lang: Lang) -> Result<String, String> {
    // Top-level argument token ranges of the call.
    let mut args: Vec<(usize, usize)> = Vec::new();
    let (mut depth, mut start) = (0usize, open + 1);
    let mut k = open;
    while k < toks.len() {
        match toks[k].tok {
            Tok::Punct('(' | '[' | '{') => depth += 1,
            Tok::Punct(')' | ']' | '}') => {
                depth -= 1;
                if depth == 0 {
                    if k > start {
                        args.push((start, k));
                    }
                    break;
                }
            }
            Tok::Punct(',') if depth == 1 => {
                args.push((start, k));
                start = k + 1;
            }
            _ => {}
        }
        k += 1;
    }
    let is =
        |i: usize, w: &str| matches!(toks.get(i).map(|t| &t.tok), Some(Tok::Ident(x)) if x == w);
    let range = args
        .iter()
        .find(|(a, _)| {
            is(*a, "flags") && matches!(toks.get(a + 1).map(|t| &t.tok), Some(Tok::Punct('=')))
        })
        .map(|(a, b)| (a + 2, *b))
        .or_else(|| {
            args.get(pos)
                .filter(|(a, _)| !matches!(toks.get(a + 1).map(|t| &t.tok), Some(Tok::Punct('='))))
                .copied()
        });
    let Some((a, b)) = range else {
        return Ok(String::new());
    };
    let mut letters = String::new();
    for t in &toks[a..b] {
        let name = match &t.tok {
            Tok::Ident(w) if w == "re" || w == "Pattern" => continue,
            Tok::Punct('.' | '|' | '(' | ')') => continue,
            Tok::Ident(w) if w == "0" => continue,
            Tok::Ident(w) => w.as_str(),
            _ => return Err("the flags argument is not a constant".into()),
        };
        let l = match (lang, name) {
            (Lang::Python, "I" | "IGNORECASE") | (Lang::Java, "CASE_INSENSITIVE") => 'i',
            (Lang::Python, "M" | "MULTILINE") | (Lang::Java, "MULTILINE") => 'm',
            (Lang::Python, "S" | "DOTALL") | (Lang::Java, "DOTALL") => 's',
            (Lang::Python, "X" | "VERBOSE") | (Lang::Java, "COMMENTS") => 'x',
            (Lang::Python, "A" | "ASCII") => 'a',
            (Lang::Python, "U" | "UNICODE") => continue,
            (Lang::Java, "UNIX_LINES") => 'd',
            (Lang::Java, "UNICODE_CASE") => 'u',
            (Lang::Java, "UNICODE_CHARACTER_CLASS") => 'U',
            _ => return Err(format!("flag {name} is not analysed")),
        };
        if !letters.contains(l) {
            letters.push(l);
        }
    }
    Ok(letters)
}

// ------------------------------------------------------------------ lexer

struct Lexer<'a> {
    s: Vec<char>,
    i: usize,
    line: usize,
    col: usize,
    lang: Lang,
    out: Vec<T>,
    _src: &'a str,
}

fn lex(src: &str, lang: Lang) -> Vec<T> {
    let mut lx = Lexer {
        s: src.chars().collect(),
        i: 0,
        line: 1,
        col: 1,
        lang,
        out: Vec::new(),
        _src: src,
    };
    lx.run();
    lx.out
}

impl Lexer<'_> {
    fn peek(&self, k: usize) -> Option<char> {
        self.s.get(self.i + k).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.s.get(self.i).copied()?;
        self.i += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn push(&mut self, tok: Tok, line: usize, col: usize) {
        self.out.push(T { tok, line, col });
    }

    fn comment(&mut self, text: &str, line: usize, col: usize) {
        if let Some(rest) = text.split("refract:").nth(1) {
            let name: String = rest
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if let Ok(f) = name.parse::<Flavor>() {
                self.push(Tok::Directive(f), line, col);
            }
        }
    }

    /// Whether a `/` here starts a JS regex literal (from the previous significant token).
    fn regex_allowed(&self) -> bool {
        match self.out.last().map(|t| &t.tok) {
            None | Some(Tok::Directive(_)) => true,
            Some(Tok::Punct(c)) => !matches!(c, ')' | ']' | '}'),
            Some(Tok::Ident(w)) => matches!(
                w.as_str(),
                "return"
                    | "typeof"
                    | "case"
                    | "do"
                    | "else"
                    | "in"
                    | "of"
                    | "new"
                    | "delete"
                    | "void"
                    | "throw"
                    | "yield"
                    | "await"
            ),
            _ => false,
        }
    }

    fn run(&mut self) {
        // `#!/usr/bin/env node`: not a regex literal.
        if self.peek(0) == Some('#') && self.peek(1) == Some('!') {
            while let Some(c) = self.peek(0)
                && c != '\n'
            {
                self.bump();
            }
        }
        while let Some(c) = self.peek(0) {
            let (line, col) = (self.line, self.col);
            if c.is_whitespace() {
                self.bump();
                continue;
            }
            let line_comment = match self.lang {
                Lang::Python => c == '#',
                _ => c == '/' && self.peek(1) == Some('/'),
            };
            if line_comment {
                let mut text = String::new();
                while let Some(c) = self.peek(0)
                    && c != '\n'
                {
                    text.push(c);
                    self.bump();
                }
                self.comment(&text, line, col);
                continue;
            }
            if self.lang != Lang::Python && c == '/' && self.peek(1) == Some('*') {
                self.bump();
                self.bump();
                let mut depth = 1;
                let mut text = String::new();
                while depth > 0 && self.peek(0).is_some() {
                    if self.peek(0) == Some('*') && self.peek(1) == Some('/') {
                        self.bump();
                        self.bump();
                        depth -= 1;
                    } else if self.lang == Lang::Rust
                        && self.peek(0) == Some('/')
                        && self.peek(1) == Some('*')
                    {
                        self.bump();
                        self.bump();
                        depth += 1;
                    } else {
                        text.extend(self.bump());
                    }
                }
                self.comment(&text, line, col);
                continue;
            }
            if c.is_alphanumeric() || c == '_' || c == '$' {
                let mut w = String::new();
                while let Some(c) = self.peek(0)
                    && (c.is_alphanumeric() || c == '_' || c == '$')
                {
                    w.push(c);
                    self.bump();
                }
                // String prefixes.
                if let Some(q) = self.peek(0) {
                    let lw = w.to_ascii_lowercase();
                    match self.lang {
                        Lang::Python
                            if (q == '"' || q == '\'')
                                && matches!(
                                    lw.as_str(),
                                    "r" | "u" | "b" | "br" | "rb" | "f" | "fr" | "rf"
                                ) =>
                        {
                            let v = self.py_string(lw.contains('r'));
                            self.push(Tok::Str(if lw.contains('f') { None } else { v }), line, col);
                            continue;
                        }
                        Lang::Rust
                            if matches!(w.as_str(), "r" | "br") && (q == '"' || q == '#') =>
                        {
                            let v = self.rust_raw();
                            self.push(Tok::Str(v), line, col);
                            continue;
                        }
                        Lang::Rust if w == "b" && q == '"' => {
                            let v = self.escaped('"', Lang::Rust);
                            self.push(Tok::Str(v), line, col);
                            continue;
                        }
                        _ => {}
                    }
                }
                self.push(Tok::Ident(w), line, col);
                continue;
            }
            match (self.lang, c) {
                (Lang::Python, '"' | '\'') => {
                    let v = self.py_string(false);
                    self.push(Tok::Str(v), line, col);
                }
                (Lang::JavaScript, '"' | '\'') => {
                    self.bump();
                    let v = self.escaped(c, Lang::JavaScript);
                    self.push(Tok::Str(v), line, col);
                }
                (Lang::JavaScript, '`') => {
                    self.bump();
                    let v = self.template();
                    self.push(Tok::Str(v), line, col);
                }
                (Lang::JavaScript, '/') if self.regex_allowed() => {
                    let v = self.js_regex();
                    self.push(Tok::Regex(v), line, col);
                }
                (Lang::Go, '`') => {
                    self.bump();
                    let mut v = String::new();
                    while let Some(c) = self.bump()
                        && c != '`'
                    {
                        if c != '\r' {
                            v.push(c);
                        }
                    }
                    self.push(Tok::Str(Some(v)), line, col);
                }
                (Lang::Go | Lang::Java | Lang::Rust, '"') => {
                    if self.lang == Lang::Java
                        && self.peek(1) == Some('"')
                        && self.peek(2) == Some('"')
                    {
                        let v = self.java_text_block();
                        self.push(Tok::Str(v), line, col);
                    } else {
                        self.bump();
                        let v = self.escaped('"', self.lang);
                        self.push(Tok::Str(v), line, col);
                    }
                }
                (Lang::Go | Lang::Java, '\'') => {
                    self.bump();
                    let _ = self.escaped('\'', self.lang);
                    self.push(Tok::Punct('\''), line, col);
                }
                (Lang::Rust, '\'') => {
                    // A char literal (`'x'`, `'\n'`) or a lifetime (`'a`).
                    self.bump();
                    if self.peek(0) == Some('\\') || self.peek(1) == Some('\'') {
                        let _ = self.escaped('\'', Lang::Rust);
                    }
                    self.push(Tok::Punct('\''), line, col);
                }
                _ => {
                    self.bump();
                    self.push(Tok::Punct(c), line, col);
                }
            }
        }
    }

    /// At an opening quote (after any prefix): a Python string, single or triple quoted.
    fn py_string(&mut self, raw: bool) -> Option<String> {
        let q = self.bump()?;
        let triple = self.peek(0) == Some(q) && self.peek(1) == Some(q);
        if triple {
            self.bump();
            self.bump();
        }
        let mut body = String::new();
        loop {
            let c = self.bump()?;
            if c == '\\' {
                body.push(c);
                body.extend(self.bump());
                continue;
            }
            if c == q && (!triple || (self.peek(0) == Some(q) && self.peek(1) == Some(q))) {
                if triple {
                    self.bump();
                    self.bump();
                }
                break;
            }
            if c == '\n' && !triple {
                return None;
            }
            body.push(c);
        }
        Some(if raw {
            body
        } else {
            decode(&body, Lang::Python)?
        })
    }

    /// After the opening quote: up to the unescaped `close`, decoded with `lang`'s escapes.
    fn escaped(&mut self, close: char, lang: Lang) -> Option<String> {
        let mut body = String::new();
        loop {
            let c = self.bump()?;
            if c == '\\' {
                body.push(c);
                body.extend(self.bump());
                continue;
            }
            if c == close {
                break;
            }
            if c == '\n' && lang != Lang::Rust {
                return None;
            }
            body.push(c);
        }
        decode(&body, lang)
    }

    fn rust_raw(&mut self) -> Option<String> {
        let mut hashes = 0;
        while self.peek(0) == Some('#') {
            self.bump();
            hashes += 1;
        }
        if self.bump()? != '"' {
            return None;
        }
        let mut body = String::new();
        loop {
            let c = self.bump()?;
            if c == '"' && (0..hashes).all(|k| self.peek(k) == Some('#')) {
                for _ in 0..hashes {
                    self.bump();
                }
                return Some(body);
            }
            body.push(c);
        }
    }

    fn template(&mut self) -> Option<String> {
        let mut body = String::new();
        let mut dynamic = false;
        loop {
            let c = self.bump()?;
            match c {
                '\\' => {
                    body.push(c);
                    body.extend(self.bump());
                }
                '`' => break,
                '$' if self.peek(0) == Some('{') => {
                    dynamic = true;
                    let mut depth = 0;
                    while let Some(c) = self.bump() {
                        match c {
                            '{' => depth += 1,
                            '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => body.push(c),
            }
        }
        if dynamic {
            None
        } else {
            decode(&body, Lang::JavaScript)
        }
    }

    fn js_regex(&mut self) -> String {
        let mut v = String::new();
        v.extend(self.bump()); // '/'
        let mut class = false;
        while let Some(c) = self.peek(0) {
            if c == '\n' {
                break;
            }
            self.bump();
            v.push(c);
            match c {
                '\\' => v.extend(self.bump()),
                '[' => class = true,
                ']' => class = false,
                '/' if !class => break,
                _ => {}
            }
        }
        while let Some(c) = self.peek(0)
            && c.is_ascii_alphabetic()
        {
            v.push(c);
            self.bump();
        }
        v
    }

    fn java_text_block(&mut self) -> Option<String> {
        for _ in 0..3 {
            self.bump();
        }
        // The content starts after the line break that follows the opening delimiter.
        while let Some(c) = self.bump()
            && c != '\n'
        {}
        let mut lines: Vec<String> = vec![String::new()];
        loop {
            if self.peek(0) == Some('"') && self.peek(1) == Some('"') && self.peek(2) == Some('"') {
                for _ in 0..3 {
                    self.bump();
                }
                break;
            }
            let c = self.bump()?;
            if c == '\\' {
                lines.last_mut()?.push(c);
                lines.last_mut()?.extend(self.bump());
            } else if c == '\n' {
                lines.push(String::new());
            } else {
                lines.last_mut()?.push(c);
            }
        }
        // Incidental indentation: the smallest indent of non-blank lines (and the last line).
        let last_blank = lines.last().is_some_and(|l| l.trim().is_empty());
        let indent = lines
            .iter()
            .enumerate()
            .filter(|(i, l)| !l.trim().is_empty() || *i == lines.len() - 1)
            .map(|(_, l)| l.chars().take_while(|c| *c == ' ' || *c == '\t').count())
            .min()
            .unwrap_or(0);
        let n = lines.len();
        let body: Vec<String> = lines
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !(last_blank && *i == n - 1))
            .map(|(_, l)| {
                l.chars()
                    .skip(indent)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        let mut text = body.join("\n");
        if last_blank {
            text.push('\n');
        }
        decode(&text, Lang::Java)
    }
}

fn hex(digits: &str) -> Option<char> {
    char::from_u32(u32::from_str_radix(digits, 16).ok()?)
}

/// Decodes a string literal body with `lang`'s escape rules. `None` for escapes that cannot be
/// resolved statically (Python `\N{...}`).
fn decode(body: &str, lang: Lang) -> Option<String> {
    let s: Vec<char> = body.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let take = |i: &mut usize, n: usize| -> Option<String> {
        let t: String = s.get(*i..*i + n)?.iter().collect();
        *i += n;
        Some(t)
    };
    while i < s.len() {
        let c = s[i];
        i += 1;
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(&e) = s.get(i) else {
            out.push('\\');
            break;
        };
        i += 1;
        let simple = match e {
            'n' => Some('\n'),
            't' => Some('\t'),
            'r' => Some('\r'),
            '\\' => Some('\\'),
            '"' => Some('"'),
            '\'' => Some('\''),
            '0' if lang == Lang::Rust => Some('\0'),
            'b' if lang != Lang::Rust => Some('\u{8}'),
            'f' if lang != Lang::Rust => Some('\u{c}'),
            'v' if matches!(lang, Lang::Python | Lang::JavaScript | Lang::Go) => Some('\u{b}'),
            'a' if matches!(lang, Lang::Python | Lang::Go) => Some('\u{7}'),
            's' if lang == Lang::Java => Some(' '),
            '`' if lang == Lang::JavaScript => Some('`'),
            '\n' if matches!(lang, Lang::Python | Lang::JavaScript | Lang::Java) => {
                continue;
            }
            '\n' if lang == Lang::Rust => {
                while s.get(i).is_some_and(|c| c.is_whitespace()) {
                    i += 1;
                }
                continue;
            }
            _ => None,
        };
        if let Some(ch) = simple {
            out.push(ch);
            continue;
        }
        match e {
            'x' => {
                let ch = if lang == Lang::Rust && s.get(i) == Some(&'{') {
                    None
                } else {
                    hex(&take(&mut i, 2)?)
                };
                out.push(ch?);
            }
            'u' if s.get(i) == Some(&'{') && matches!(lang, Lang::JavaScript | Lang::Rust) => {
                let end = s[i..].iter().position(|&c| c == '}')? + i;
                let digits: String = s[i + 1..end].iter().filter(|&&c| c != '_').collect();
                i = end + 1;
                out.push(hex(&digits)?);
            }
            'u' if lang != Lang::Rust => {
                if lang == Lang::Java {
                    while s.get(i) == Some(&'u') {
                        i += 1;
                    }
                }
                let hi = u32::from_str_radix(&take(&mut i, 4)?, 16).ok()?;
                // JS and Java strings are UTF-16: join a surrogate pair.
                if (0xD800..0xDC00).contains(&hi)
                    && s.get(i) == Some(&'\\')
                    && s.get(i + 1) == Some(&'u')
                {
                    let save = i;
                    i += 2;
                    match take(&mut i, 4).and_then(|d| u32::from_str_radix(&d, 16).ok()) {
                        Some(lo) if (0xDC00..0xE000).contains(&lo) => {
                            out.push(char::from_u32(
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00),
                            )?);
                            continue;
                        }
                        _ => i = save,
                    }
                }
                out.push(char::from_u32(hi)?);
            }
            'U' if matches!(lang, Lang::Python | Lang::Go) => out.push(hex(&take(&mut i, 8)?)?),
            '0'..='7'
                if matches!(
                    lang,
                    Lang::Python | Lang::Go | Lang::Java | Lang::JavaScript
                ) =>
            {
                let mut digits = e.to_string();
                let max = 3;
                while digits.len() < max && s.get(i).is_some_and(|c| ('0'..='7').contains(c)) {
                    digits.push(s[i]);
                    i += 1;
                }
                out.push(char::from_u32(u32::from_str_radix(&digits, 8).ok()?)?);
            }
            'N' if lang == Lang::Python => return None,
            // Unknown escapes: Python keeps the backslash; JS drops it ("\d" is "d").
            _ if lang == Lang::Python => {
                out.push('\\');
                out.push(e);
            }
            _ => out.push(e),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pats(src: &str, lang: Lang) -> Vec<(String, usize)> {
        extract(src, lang)
            .into_iter()
            .map(|f| (f.pattern, f.line))
            .collect()
    }

    #[test]
    fn python_calls_raw_and_cooked() {
        let src = "import re\n# re.compile('not me')\nA = re.compile(r'^\\w+$')\nre.fullmatch(\"a\\\\d\", s)\nre.search('\\d', s)\nre.sub(f'{x}', '', s)\n";
        assert_eq!(
            pats(src, Lang::Python),
            vec![
                ("^\\w+$".into(), 3),
                ("a\\d".into(), 4),
                ("\\d".into(), 5),
                ("".into(), 6)
            ]
        );
        assert_eq!(extract(src, Lang::Python)[1].op, Op::FullMatch);
    }

    #[test]
    fn js_literals_and_constructor() {
        let src = "const a = /ab+c/gi; // /x/\nconst r = x / 2 / y;\nif (/^\\d+$/.test(s)) {}\nnew RegExp(\"\\\\d+\", \"u\");\nnew RegExp('\\d');\nconst t = `a${b}`;\n";
        assert_eq!(
            pats(src, Lang::JavaScript),
            vec![
                ("/ab+c/gi".into(), 1),
                ("/^\\d+$/".into(), 3),
                ("/\\d+/u".into(), 4),
                ("/d/".into(), 5)
            ]
        );
    }

    #[test]
    fn go_java_rust() {
        let go = "var r = regexp.MustCompile(`^\\d+$`)\nvar s = regexp.MustCompile(\"\\\\bx\")\nregexp.MustCompilePOSIX(`a|ab`)\n";
        let g = extract(go, Lang::Go);
        assert_eq!(
            g.iter().map(|f| f.pattern.as_str()).collect::<Vec<_>>(),
            ["^\\d+$", "\\bx", "a|ab"]
        );
        assert!(g[2].unsupported.is_some());
        let java = "Pattern p = Pattern.compile(\"\\\\w+\");\nboolean b = s.matches(\"[a-z]+\");\nString t = \"\"\"\n    a\\\\d\n    \"\"\";\n";
        let j = extract(java, Lang::Java);
        assert_eq!(
            j.iter()
                .map(|f| (f.pattern.as_str(), f.op))
                .collect::<Vec<_>>(),
            [("\\w+", Op::Search), ("[a-z]+", Op::FullMatch)]
        );
        let rust = "let r = Regex::new(r\"^\\d+$\").unwrap();\nlet s = Regex::new(r#\"\"\\w\"\"#)?;\nlet c = 'a'; fn f<'a>() {}\nlet t = RegexSet::new([\"a\", r\"b\"]);\n";
        assert_eq!(
            pats(rust, Lang::Rust),
            vec![
                ("^\\d+$".into(), 1),
                ("\"\\w\"".into(), 2),
                ("a".into(), 4),
                ("b".into(), 4)
            ]
        );
    }

    #[test]
    fn concatenation_and_flags() {
        let py = "re.compile(r'a' r'b')\nre.compile('(' + x + ')')\nre.compile('x', re.I | re.VERBOSE)\nre.sub('a', 'b', s, flags=re.M)\nre.search('y', s, FLAGS)\nre.compile('%s+' % x)\n";
        let f = extract(py, Lang::Python);
        let got: Vec<_> = f
            .iter()
            .map(|x| (x.pattern.as_str(), x.unsupported.is_some()))
            .collect();
        assert_eq!(
            got,
            [
                ("ab", false),
                ("(", true),
                ("(?ix)x", false),
                ("(?m)a", false),
                ("y", true),
                ("%s+", true)
            ]
        );
        let java = "Pattern.compile(\"a\" + \"b\", Pattern.CASE_INSENSITIVE | Pattern.COMMENTS);\nPattern.compile(\"c\", Pattern.LITERAL);";
        let j: Vec<_> = extract(java, Lang::Java)
            .into_iter()
            .map(|x| (x.pattern, x.unsupported.is_some()))
            .collect();
        assert_eq!(j, [("(?ix)ab".to_string(), false), ("c".to_string(), true)]);
        assert!(extract("#!/usr/bin/env node\nconst a = 1;", Lang::JavaScript).is_empty());
    }

    #[test]
    fn directives_mark_strings() {
        let src = "# refract: js\nPATTERN = '^\\\\w+$'\n";
        let f = extract(src, Lang::Python);
        assert_eq!(f.len(), 1);
        assert_eq!(
            (f[0].pattern.as_str(), f[0].flavor),
            ("/^\\w+$/", Flavor::JavaScript)
        );
    }

    #[test]
    fn surrogate_pairs_and_escapes() {
        assert_eq!(decode("\\uD83D\\uDE00", Lang::JavaScript).unwrap(), "😀");
        assert_eq!(
            decode("\\x41\\101\\u{1F600}", Lang::JavaScript).unwrap(),
            "AA😀"
        );
        assert_eq!(decode("\\d\\x41", Lang::Python).unwrap(), "\\dA");
        assert_eq!(decode("\\u{1F600}", Lang::Rust).unwrap(), "😀");
    }

    #[test]
    fn never_panics_on_garbage() {
        for lang in [
            Lang::Python,
            Lang::JavaScript,
            Lang::Go,
            Lang::Java,
            Lang::Rust,
        ] {
            for src in [
                "re.compile(",
                "\"\\",
                "r#\"",
                "/[",
                "`${",
                "'''",
                "\"\"\"\n",
                "\\u{",
                "/* refract: js",
                "Regex::new(r\"",
                "'\\u",
            ] {
                let _ = extract(src, lang);
            }
        }
    }
}
