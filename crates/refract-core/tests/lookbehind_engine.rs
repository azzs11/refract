//! The context-aware derivative engine (ADR-0008) against the direct set-based semantics, on
//! random terms with look-behind atoms. The Thompson engine has no look-behind, so the naive
//! matcher is the independent second implementation here.

mod common;

use common::{arb_input, arb_re_with_behind, cases};
use proptest::prelude::*;
use refract_core::automata::{alphabet_for, dfa};
use refract_core::equiv::{Side, Verdict, equivalent};
use refract_core::minimize::hopcroft;
use refract_core::{Budget, Engine, ErrorKind};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(5_000)))]

    #[test]
    fn behind_dfa_matches_direct_semantics(re in arb_re_with_behind(), inputs in prop::collection::vec(arb_input(), 1..10)) {
        let budget = Budget::small().start();
        let alpha = alphabet_for(&[&re]);
        let m = match dfa(&re, &alpha, Engine::Derivative, &budget) {
            Ok(m) => m,
            Err(e) => {
                prop_assert_eq!(e.kind, ErrorKind::Budget, "{}", e.message);
                return Ok(());
            }
        };
        let min = hopcroft(&m);
        for s in &inputs {
            let want = re.matches_naive(s);
            prop_assert_eq!(m.accepts(s), want, "{:?} for {}", s, re);
            prop_assert_eq!(min.accepts(s), want, "minimised, {:?} for {}", s, re);
        }
    }

    #[test]
    fn behind_witnesses_are_valid(a in arb_re_with_behind(), b in arb_re_with_behind()) {
        let budget = Budget::small().start();
        let Ok(rep) = equivalent(&a, &b, Engine::Derivative, &budget) else { return Ok(()) };
        if let Verdict::Different(w) = rep.verdict {
            let (x, y) = (a.matches_naive(&w.text), b.matches_naive(&w.text));
            prop_assert!(x != y, "witness {:?} does not distinguish {} and {}", w.text, a, b);
            prop_assert_eq!(w.accepted_by == Side::Left, x);
        }
    }
}
