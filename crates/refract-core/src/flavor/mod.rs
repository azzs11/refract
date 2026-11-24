//! Regex flavours: one hand-written front-end per flavour plus an explicit semantics profile.
//!
//! The profile states, in data, the differences that the IR resolves at parse time. It is what
//! reports and the docs show to explain *why* two engines disagree. Each profile mirrors a
//! `spec/flavours/<name>.md` document, which is written before the parser (spec before code).

mod rust;

use crate::error::{Error, Result, Span};
use crate::ir::Regex;
use std::fmt;
use std::str::FromStr;

/// A regex flavour (dialect). Only some have front-ends so far; see [`Flavor::is_implemented`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Flavor {
    Rust,
    JavaScript,
    Python,
    Go,
    Java,
    Pcre2,
    DotNet,
    Ruby,
}

impl Flavor {
    pub const ALL: [Flavor; 8] = [
        Flavor::Rust,
        Flavor::JavaScript,
        Flavor::Python,
        Flavor::Go,
        Flavor::Java,
        Flavor::Pcre2,
        Flavor::DotNet,
        Flavor::Ruby,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Flavor::Rust => "rust",
            Flavor::JavaScript => "js",
            Flavor::Python => "python",
            Flavor::Go => "go",
            Flavor::Java => "java",
            Flavor::Pcre2 => "pcre2",
            Flavor::DotNet => "dotnet",
            Flavor::Ruby => "ruby",
        }
    }

    pub fn is_implemented(self) -> bool {
        matches!(self, Flavor::Rust)
    }

    /// The semantics profile, when the flavour has a front-end.
    pub fn profile(self) -> Option<&'static Profile> {
        match self {
            Flavor::Rust => Some(&rust::PROFILE),
            _ => None,
        }
    }
}

impl fmt::Display for Flavor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Flavor {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "rust" => Flavor::Rust,
            "js" | "javascript" | "ecmascript" => Flavor::JavaScript,
            "python" | "py" => Flavor::Python,
            "go" | "re2" => Flavor::Go,
            "java" => Flavor::Java,
            "pcre2" | "pcre" | "php" => Flavor::Pcre2,
            "dotnet" | ".net" | "csharp" => Flavor::DotNet,
            "ruby" | "onigmo" => Flavor::Ruby,
            other => return Err(format!("unknown flavour `{other}`")),
        })
    }
}

/// How alternation picks among matches during search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchPolicy {
    /// Backtracking order: the first alternative that leads to a match wins.
    LeftmostFirst,
    /// POSIX: the longest match among those starting leftmost.
    LeftmostLongest,
}

/// The semantics profile of one flavour with default options.
#[derive(Clone, Debug)]
pub struct Profile {
    pub flavor: Flavor,
    /// Engine and version this profile was written against.
    pub engine: &'static str,
    /// Where the Unicode tables come from.
    pub unicode: &'static str,
    /// What `.` excludes without the dot-all flag.
    pub dot_excludes: &'static str,
    /// Meaning of `$` without multi-line.
    pub dollar: &'static str,
    /// Whether `\w`, `\d`, `\s` and `\b` are Unicode-aware by default.
    pub unicode_classes: bool,
    /// Case-insensitive matching rule.
    pub case_folding: &'static str,
    pub match_policy: MatchPolicy,
    pub lookaround: bool,
    pub backreferences: bool,
    /// Guaranteed linear-time matching.
    pub linear_time: bool,
    /// The spec document for this flavour.
    pub spec: &'static str,
}

/// Parses `pattern` in `flavor` with default options.
pub fn parse(flavor: Flavor, pattern: &str) -> Result<Regex> {
    match flavor {
        Flavor::Rust => rust::parse(pattern),
        other => Err(Error::unsupported(
            Span::new(0, pattern.len()),
            format!("the {other} front-end is not implemented yet (planned for phase P3)"),
        )),
    }
}
