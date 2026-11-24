//! Span-carrying errors. Every function that takes user input returns `Result<_, Error>`; nothing
//! on a user-input path panics.

use std::fmt;

/// A half-open byte range `[start, end)` into the original pattern text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    /// Smallest span covering both.
    pub fn to(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// What went wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The pattern is not valid in the chosen flavour: the real engine rejects it too.
    Syntax,
    /// The pattern is valid in its flavour (the real engine accepts it) but uses a construct this
    /// analysis does not handle yet. Never used for something the engine itself rejects.
    Unsupported,
    /// An analysis hit its state or time budget.
    Budget,
}

impl ErrorKind {
    /// True when the error still means "the real engine accepts this pattern".
    pub fn engine_accepts(&self) -> bool {
        !matches!(self, ErrorKind::Syntax)
    }
}

/// A user-facing error with a precise location.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{kind:?} error at {span}: {message}")]
pub struct Error {
    pub kind: ErrorKind,
    pub span: Span,
    pub message: String,
}

impl Error {
    pub fn syntax(span: Span, message: impl Into<String>) -> Self {
        Error {
            kind: ErrorKind::Syntax,
            span,
            message: message.into(),
        }
    }

    pub fn unsupported(span: Span, message: impl Into<String>) -> Self {
        Error {
            kind: ErrorKind::Unsupported,
            span,
            message: message.into(),
        }
    }

    pub fn budget(message: impl Into<String>) -> Self {
        Error {
            kind: ErrorKind::Budget,
            span: Span::default(),
            message: message.into(),
        }
    }

    /// Renders the error with a caret line under the offending part of `pattern`.
    pub fn render(&self, pattern: &str) -> String {
        let kind = match self.kind {
            ErrorKind::Syntax => "syntax error",
            ErrorKind::Unsupported => "unsupported",
            ErrorKind::Budget => "budget exceeded",
        };
        if self.kind == ErrorKind::Budget {
            return format!("{kind}: {}", self.message);
        }
        let start = pattern
            .get(..self.span.start.min(pattern.len()))
            .map_or(0, |s| s.chars().count());
        let width = pattern
            .get(self.span.start.min(pattern.len())..self.span.end.min(pattern.len()))
            .map_or(1, |s| s.chars().count().max(1));
        format!(
            "{kind}: {}\n  {pattern}\n  {}{}",
            self.message,
            " ".repeat(start),
            "^".repeat(width)
        )
    }
}

pub type Result<T> = std::result::Result<T, Error>;
