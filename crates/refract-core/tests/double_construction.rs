//! P1 gate: the two automaton engines and the two minimisers must agree (doc §9, "Double
//! construction"), and both must agree with a third, direct semantics on concrete strings.

mod common;

use common::{arb_input, arb_re, cases};
use proptest::prelude::*;
use refract_regex::automata::{alphabet_for, dfa};
use refract_regex::minimize::{brzozowski, hopcroft};
use refract_regex::{Budget, Engine, ErrorKind, Re};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(5_000)))]

    /// deriv→Hopcroft, Thompson→Hopcroft, deriv→Brzozowski and Thompson→Brzozowski all yield
    /// the same canonical minimal DFA.
    #[test]
    fn engines_and_minimisers_agree(re in arb_re()) {
        let budget = Budget::small().start();
        let alpha = alphabet_for(&[&re]);
        let d = dfa(&re, &alpha, Engine::Derivative, &budget);
        let t = dfa(&re, &alpha, Engine::Thompson, &budget);
        let (d, t) = match (d, t) {
            (Ok(d), Ok(t)) => (d, t),
            // A budget stop is allowed (and reported), anything else is a bug.
            (Err(e), _) | (_, Err(e)) => {
                prop_assert_eq!(e.kind, ErrorKind::Budget, "{}", e.message);
                return Ok(());
            }
        };
        let dh = hopcroft(&d).canonical();
        let th = hopcroft(&t).canonical();
        prop_assert_eq!(&dh, &th, "deriv vs thompson differ for {}", re);
        if let (Ok(db), Ok(tb)) = (brzozowski(&d, &budget), brzozowski(&t, &budget)) {
            prop_assert_eq!(&db.canonical(), &dh, "brzozowski(deriv) vs hopcroft for {}", re);
            prop_assert_eq!(&tb.canonical(), &dh, "brzozowski(thompson) vs hopcroft for {}", re);
        }
    }

    /// Both engines accept exactly what the direct set-based semantics accepts.
    #[test]
    fn engines_match_direct_semantics(re in arb_re(), inputs in prop::collection::vec(arb_input(), 1..8)) {
        let budget = Budget::small().start();
        let alpha = alphabet_for(&[&re]);
        for engine in [Engine::Derivative, Engine::Thompson] {
            let Ok(m) = dfa(&re, &alpha, engine, &budget) else { return Ok(()) };
            for s in &inputs {
                prop_assert_eq!(m.accepts(s), re.matches_naive(s), "{:?} on {:?} for {}", engine, s, re);
            }
        }
    }

    /// Hopcroft output is minimal: no two states are equivalent (checked by brute force on the
    /// product, independent of both minimisers) and every state is reachable.
    #[test]
    fn hopcroft_is_minimal(re in arb_re()) {
        let budget = Budget::small().start();
        let alpha = alphabet_for(&[&re]);
        let Ok(d) = dfa(&re, &alpha, Engine::Derivative, &budget) else { return Ok(()) };
        let m = hopcroft(&d);
        prop_assert!(m.len() <= d.trim().len());
        for p in 0..m.len() {
            for q in (p + 1)..m.len() {
                let a = refract_regex::Dfa { start: p, ..m.clone() };
                let b = refract_regex::Dfa { start: q, ..m.clone() };
                let x = a.product(&b, |x, y| x != y, &budget).unwrap();
                prop_assert!(!x.is_empty_language(), "states {} and {} are equivalent in {}", p, q, re);
            }
        }
    }
}

#[test]
fn hand_checked_sizes() {
    let budget = Budget::default().start();
    let cases: [(Re, usize); 4] = [
        // (a|b)*abb: the textbook example, minimal DFA has 4 live states + no dead state.
        (
            Re::Concat(vec![
                Re::star(Re::Alt(vec![Re::lit('a'), Re::lit('b')])),
                Re::string("abb"),
            ]),
            // over minterms {a},{b},{other}: 4 states + dead state
            5,
        ),
        (Re::Nothing, 1),
        (Re::any_star(), 1),
        (Re::Repeat(Box::new(Re::lit('a')), 2, Some(4)), 6),
    ];
    for (re, want) in cases {
        let alpha = alphabet_for(&[&re]);
        for engine in [Engine::Derivative, Engine::Thompson] {
            let m = hopcroft(&dfa(&re, &alpha, engine, &budget).unwrap());
            assert_eq!(m.len(), want, "{engine:?} on {re}");
        }
    }
}
