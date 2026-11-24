//! P2 gate (doc §9 "Witness validity"): every witness really distinguishes the two sides, no
//! shorter distinguishing string exists (brute force up to length 6), both engines return the
//! same witness, and the algebraic laws are proven by the equivalence checker.

mod common;

use common::{arb_re, cases};
use proptest::prelude::*;
use refract_core::automata::alphabet_for;
use refract_core::equiv::{Side, Verdict, equivalent};
use refract_core::{Budget, Engine, ErrorKind, Re};

/// All strings over the minterm representatives of `a` and `b`, up to `max_len`. Every string
/// is equivalent (class by class) to one of these, so this search is complete for that length.
fn brute_force_distinguishes(a: &Re, b: &Re, max_len: usize) -> Option<String> {
    let alpha = alphabet_for(&[a, b]);
    let reps: Vec<char> = alpha
        .readable_order()
        .iter()
        .map(|&i| alpha.representative(i))
        .collect();
    let mut level = vec![String::new()];
    for len in 0..=max_len {
        for s in &level {
            if a.matches_naive(s) != b.matches_naive(s) {
                return Some(s.clone());
            }
        }
        if len == max_len {
            break;
        }
        level = level
            .iter()
            .flat_map(|s| reps.iter().map(move |&c| format!("{s}{c}")))
            .collect();
        if level.len() > 20_000 {
            return None; // keep the brute force bounded
        }
    }
    None
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(3_000)))]

    #[test]
    fn witnesses_are_valid_and_shortest(a in arb_re(), b in arb_re()) {
        let budget = Budget::small().start();
        let report = match equivalent(&a, &b, Engine::Derivative, &budget) {
            Ok(r) => r,
            Err(e) => {
                prop_assert_eq!(e.kind, ErrorKind::Budget);
                return Ok(());
            }
        };
        match &report.verdict {
            Verdict::Different(w) => {
                let (x, y) = (a.matches_naive(&w.text), b.matches_naive(&w.text));
                prop_assert!(x != y, "witness {:?} does not distinguish {} and {}", w.text, a, b);
                prop_assert_eq!(w.accepted_by == Side::Left, x);
                let n = w.text.chars().count();
                if n > 0 {
                    let shorter = brute_force_distinguishes(&a, &b, (n - 1).min(6));
                    prop_assert!(shorter.is_none(), "{:?} is shorter than witness {:?}", shorter, w.text);
                }
            }
            Verdict::Equivalent => {
                let found = brute_force_distinguishes(&a, &b, 5);
                prop_assert!(found.is_none(), "claimed equivalent but {:?} distinguishes {} and {}", found, a, b);
            }
        }
    }

    /// The witness is defined as the readability-least shortest distinguishing string, so it is
    /// unique: both engines must return exactly the same one.
    #[test]
    fn engines_return_the_same_verdict(a in arb_re(), b in arb_re()) {
        let budget = Budget::small().start();
        let d = equivalent(&a, &b, Engine::Derivative, &budget);
        let t = equivalent(&a, &b, Engine::Thompson, &budget);
        if let (Ok(d), Ok(t)) = (d, t) {
            prop_assert_eq!(d.verdict, t.verdict, "for {} vs {}", a, b);
        }
    }

    #[test]
    fn laws_are_proven(r in arb_re(), s in arb_re(), t in arb_re()) {
        let budget = Budget::small().start();
        let alt = |x: &Re, y: &Re| Re::Alt(vec![x.clone(), y.clone()]);
        let cat = |x: &Re, y: &Re| Re::Concat(vec![x.clone(), y.clone()]);
        let not = |x: &Re| Re::Not(Box::new(x.clone()));
        let laws = [
            (alt(&r, &r), r.clone()),
            (Re::star(Re::star(r.clone())), Re::star(r.clone())),
            (cat(&r, &alt(&s, &t)), alt(&cat(&r, &s), &cat(&r, &t))),
            (not(&not(&r)), r.clone()),
            (Re::And(vec![r.clone(), not(&r)]), Re::Nothing),
        ];
        for (lhs, rhs) in laws {
            match equivalent(&lhs, &rhs, Engine::Derivative, &budget) {
                Ok(rep) => prop_assert_eq!(rep.verdict, Verdict::Equivalent, "{} vs {}", lhs, rhs),
                Err(e) => prop_assert_eq!(e.kind, ErrorKind::Budget),
            }
        }
    }
}
