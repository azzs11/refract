//! Translation validation at scale (ADR-0013): random patterns in every source flavour are
//! printed in every target. The printed text must parse in the target (no printer bugs) and
//! must never be *different* from the source; a construct the target lacks must be named.

mod common;

use common::cases;
use proptest::prelude::*;
use refract_core::equiv::Verdict;
use refract_core::translate::{Outcome, translate};
use refract_core::{Budget, Flavor, parse};

const TOKENS: &[&str] = &[
    "a", "b", "é", "K", "ß", "1", "😀", " ", "/", "-", ".", "^", "$", "|", "(", ")", "(?:", "(?i)",
    "(?m)", "(?s)", "(?<n>", "(?P<n>", "(?=", "(?!", "(?<=", "(?<!", "(?>", "*", "+", "?", "*?",
    "++", "{2}", "{1,3}", "{2,}", "[a-c]", "[^a]", "[-a]", "[\\]^]", "[\\w-]", "\\d", "\\D", "\\w",
    "\\W", "\\s", "\\S", "\\b", "\\B", "\\A", "\\z", "\\Z", "\\n", "\\.", "\\\\", "\\x41",
    "\\u00e9", "\\p{L}", "\\pN", "\\1",
];

const FLAVORS: [Flavor; 5] = [
    Flavor::JavaScript,
    Flavor::Python,
    Flavor::Go,
    Flavor::Java,
    Flavor::Rust,
];

fn pattern() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(TOKENS), 1..8).prop_map(|v| v.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(5_000)))]

    #[test]
    fn printed_translations_parse_and_never_differ(p in pattern(), from in 0usize..5) {
        let from = FLAVORS[from];
        let src_text = if from == Flavor::JavaScript { format!("/{p}/u") } else { p.clone() };
        let Ok(src) = parse(from, &src_text) else { return Ok(()) };
        for to in FLAVORS {
            let budget = Budget::small().start();
            let Ok(out) = translate(&src, to, &budget) else { continue }; // budget
            match out {
                Outcome::PrinterBug { text, error } => {
                    return Err(TestCaseError::fail(format!(
                        "{from}:{src_text} -> {to}: printed {text:?} does not parse: {error}"
                    )));
                }
                Outcome::Printed { text, checks } => {
                    // V8's mid-surrogate-pair search positions (docs/findings.md) have no form in
                    // other engines: a search difference there is real, not a printer bug.
                    let quirk = src.quirks.mid_pair_search
                        || parse(to, &text).is_ok_and(|d| d.quirks.mid_pair_search);
                    for c in checks {
                        if quirk && c.op == refract_core::Op::Search {
                            continue;
                        }
                        if let Verdict::Different(w) = c.verdict {
                            return Err(TestCaseError::fail(format!(
                                "{from}:{src_text} -> {to}:{text} differs under {} on {:?}",
                                c.op.name(), w.text
                            )));
                        }
                    }
                }
                Outcome::Unsupported { construct, .. } => prop_assert!(!construct.is_empty()),
            }
        }
    }
}
