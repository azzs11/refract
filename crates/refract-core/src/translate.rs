//! Verified translation (plan §5.8, ADR-0013): print the IR in the target flavour, parse the
//! printed text with the target's own front-end, and prove it equivalent to the source under
//! both operations. The printer is never trusted; only the proof makes a translation
//! "verified".

use crate::budget::Budget;
use crate::equiv::{Verdict, compare};
use crate::error::Result;
use crate::flavor::{Flavor, parse};
use crate::ir::Regex;
use crate::print::print;
use crate::regular::Op;

/// The check of one printed translation under one operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub op: Op,
    pub verdict: Verdict,
    pub product_states: usize,
}

/// What became of a translation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Printed and checked under both operations (each check says proven, bounded or different).
    Printed { text: String, checks: Vec<Check> },
    /// The target has no form for a construct; `construct` names it (with its span in the
    /// source).
    Unsupported {
        construct: String,
        span: crate::error::Span,
    },
    /// The printed text does not parse in the target: a printer bug, reported, never hidden.
    PrinterBug { text: String, error: String },
}

impl Outcome {
    /// Verified = proven equivalent under every operation (bounded checks do not count).
    pub fn verified(&self) -> bool {
        matches!(self, Outcome::Printed { checks, .. }
            if checks.iter().all(|c| c.verdict == Verdict::Equivalent))
    }
}

/// Which operations a translation must preserve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Both,
    Only(Op),
}

impl Scope {
    fn ops(self) -> Vec<Op> {
        match self {
            Scope::Both => vec![Op::FullMatch, Op::Search],
            Scope::Only(op) => vec![op],
        }
    }
}

/// Translates `src` into `to` and validates the result under both operations.
pub fn translate(src: &Regex, to: Flavor, budget: &Budget) -> Result<Outcome> {
    translate_for(src, to, Scope::Both, budget)
}

/// Translates for `scope` only. For full match, an end assertion in tail position (`$`,
/// `(?m)$`, Java's `$`) can only hold at the end of the text, so it is printed as the text end;
/// the proof covers the rewrite like everything else.
pub fn translate_for(src: &Regex, to: Flavor, scope: Scope, budget: &Budget) -> Result<Outcome> {
    let simplified;
    let src = if scope == Scope::Only(Op::FullMatch) {
        let mut r = src.clone();
        tail_ends(&mut r.root);
        simplified = r;
        &simplified
    } else {
        src
    };
    let text = match print(src, to) {
        Ok(t) => t,
        Err(e) if e.kind == crate::ErrorKind::Unsupported => {
            return Ok(Outcome::Unsupported {
                construct: e.message,
                span: e.span,
            });
        }
        Err(e) => return Err(e),
    };
    let dst = match parse(to, &text) {
        Ok(d) => d,
        Err(e) => {
            return Ok(Outcome::PrinterBug {
                text,
                error: e.message,
            });
        }
    };
    let mut checks = Vec::new();
    for op in scope.ops() {
        let rep = compare(src, &dst, op, budget)?;
        checks.push(Check {
            op,
            verdict: rep.verdict,
            product_states: rep.product_states,
        });
    }
    Ok(Outcome::Printed { text, checks })
}

fn tail_ends(n: &mut crate::ir::Node) {
    use crate::ir::{Assertion::*, NodeKind};
    match &mut n.kind {
        NodeKind::Concat(xs) => {
            if let Some(last) = xs.last_mut() {
                tail_ends(last);
            }
        }
        NodeKind::Alt(xs) => xs.iter_mut().for_each(tail_ends),
        NodeKind::Group(g) if !g.atomic => tail_ends(&mut g.sub),
        NodeKind::Assert(
            a @ (TextEndOrFinalNewline | LineEnd { .. } | JavaDollar { .. } | JavaLineEnd { .. }),
        ) => {
            *a = TextEnd;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(from: Flavor, p: &str, to: Flavor) -> Outcome {
        translate(&parse(from, p).unwrap(), to, &Budget::default()).unwrap()
    }

    #[test]
    fn email_demo_python_to_js_is_verified() {
        let o = go(Flavor::Python, r"^\w+@\w+\.com$", Flavor::JavaScript);
        assert!(o.verified(), "{o:?}");
    }

    #[test]
    fn missing_construct_is_named() {
        match go(Flavor::Python, r"a(?=b)", Flavor::Go) {
            Outcome::Unsupported { construct, .. } => assert!(construct.contains("look-around")),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn tail_dollar_translates_for_full_match() {
        let src = parse(Flavor::Python, r"^\w+@\w+\.com$").unwrap();
        let o = translate_for(
            &src,
            Flavor::Go,
            Scope::Only(Op::FullMatch),
            &Budget::default(),
        )
        .unwrap();
        assert!(o.verified(), "{o:?}");
    }

    #[test]
    fn backreferences_are_bounded_not_verified() {
        let o = go(Flavor::Python, r"(a|b)\1", Flavor::Java);
        assert!(!o.verified());
        let Outcome::Printed { checks, .. } = o else {
            panic!()
        };
        assert!(
            checks
                .iter()
                .all(|c| matches!(c.verdict, Verdict::NoDifferenceUpTo { .. }))
        );
    }
}
