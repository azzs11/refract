//! Algebraic laws of regular languages (doc §9). Each must hold for random r, s, t, decided by
//! comparing canonical minimal DFAs over a shared alphabet. P2 re-proves them with the
//! equivalence checker (`equiv_laws.rs`).

mod common;

use common::{arb_re, cases};
use proptest::prelude::*;
use refract_core::automata::{alphabet_for, minimal_dfa};
use refract_core::{Budget, Engine, ErrorKind, Re};

fn same(a: &Re, b: &Re) -> Result<bool, TestCaseError> {
    let budget = Budget::small().start();
    let alpha = alphabet_for(&[a, b]);
    let x = minimal_dfa(a, &alpha, Engine::Derivative, &budget);
    let y = minimal_dfa(b, &alpha, Engine::Derivative, &budget);
    match (x, y) {
        (Ok(x), Ok(y)) => Ok(x.canonical() == y.canonical()),
        (Err(e), _) | (_, Err(e)) => {
            prop_assert_eq!(e.kind, ErrorKind::Budget);
            Ok(true)
        }
    }
}

fn alt(a: &Re, b: &Re) -> Re {
    Re::Alt(vec![a.clone(), b.clone()])
}
fn cat(a: &Re, b: &Re) -> Re {
    Re::Concat(vec![a.clone(), b.clone()])
}
fn not(a: &Re) -> Re {
    Re::Not(Box::new(a.clone()))
}
fn star(a: &Re) -> Re {
    Re::star(a.clone())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(1_000)))]

    #[test]
    fn alt_idempotent(r in arb_re()) {
        prop_assert!(same(&alt(&r, &r), &r)?);
    }

    #[test]
    fn star_star(r in arb_re()) {
        prop_assert!(same(&star(&star(&r)), &star(&r))?);
    }

    #[test]
    fn concat_distributes_over_alt(r in arb_re(), s in arb_re(), t in arb_re()) {
        prop_assert!(same(&cat(&r, &alt(&s, &t)), &alt(&cat(&r, &s), &cat(&r, &t)))?);
    }

    #[test]
    fn double_negation(r in arb_re()) {
        prop_assert!(same(&not(&not(&r)), &r)?);
    }

    #[test]
    fn and_with_complement_is_empty(r in arb_re()) {
        prop_assert!(same(&Re::And(vec![r.clone(), not(&r)]), &Re::Nothing)?);
    }

    #[test]
    fn alt_commutative_associative(r in arb_re(), s in arb_re(), t in arb_re()) {
        prop_assert!(same(&alt(&r, &s), &alt(&s, &r))?);
        prop_assert!(same(&alt(&alt(&r, &s), &t), &alt(&r, &alt(&s, &t)))?);
    }

    #[test]
    fn de_morgan(r in arb_re(), s in arb_re()) {
        prop_assert!(same(&not(&alt(&r, &s)), &Re::And(vec![not(&r), not(&s)]))?);
    }

    #[test]
    fn star_unrolling(r in arb_re()) {
        // r* = ε | r r*
        prop_assert!(same(&star(&r), &alt(&Re::Eps, &cat(&r, &star(&r))))?);
    }

    /// Negative control: the checker must not call everything equal.
    #[test]
    fn concat_not_commutative_in_general(_x in 0..1u8) {
        prop_assert!(!same(&Re::string("ab"), &Re::string("ba"))?);
    }
}
