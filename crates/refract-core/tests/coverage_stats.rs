//! How many generated cases are actually decided, rather than skipped by a budget or an
//! unsupported construct? A property test that silently skips most inputs proves little, so
//! these thresholds are part of the test suite. Run with `--nocapture` to see the numbers.

mod common;

use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use refract_regex::automata::{alphabet_for, dfa};
use refract_regex::{Budget, Engine, Flavor, Op, parse, regular::lower};

fn runner() -> TestRunner {
    TestRunner::new_with_rng(
        Config::default(),
        TestRng::from_seed(RngAlgorithm::ChaCha, &[7; 32]),
    )
}

#[test]
fn random_terms_are_mostly_decided() {
    let mut runner = runner();
    let strategy = common::arb_re();
    let (mut ok, mut budget_hit) = (0u32, 0u32);
    for _ in 0..5_000 {
        let re = strategy.new_tree(&mut runner).expect("tree").current();
        let budget = Budget::small().start();
        let alpha = alphabet_for(&[&re]);
        let both = dfa(&re, &alpha, Engine::Derivative, &budget)
            .and_then(|_| dfa(&re, &alpha, Engine::Thompson, &budget));
        match both {
            Ok(_) => ok += 1,
            Err(_) => budget_hit += 1,
        }
    }
    println!("random terms: decided {ok}, budget stops {budget_hit}");
    assert!(
        budget_hit * 100 <= ok,
        "more than 1% of random terms hit the budget"
    );
}

#[test]
fn random_patterns_are_mostly_lowered() {
    // Same token alphabet idea as rust_oracle.rs: how many parsed patterns reach an automaton?
    let tokens: &'static [&str] = &[
        "a", "b", "é", "k", ".", "^", "$", "|", "(", ")", "(?i)", "*", "+", "?", "{2}", "[", "]",
        "[^", "-", "\\d", "\\w", "\\b",
    ];
    let mut runner = runner();
    let strategy =
        proptest::collection::vec(proptest::sample::select(tokens), 1..8).prop_map(|v| v.concat());
    let (mut parsed, mut lowered, mut unsupported) = (0u32, 0u32, 0u32);
    for _ in 0..20_000 {
        let p = strategy.new_tree(&mut runner).expect("tree").current();
        let Ok(ir) = parse(Flavor::Rust, &p) else {
            continue;
        };
        parsed += 1;
        match lower(&ir, Op::FullMatch, &Budget::small()) {
            Ok(_) => lowered += 1,
            Err(_) => unsupported += 1,
        }
    }
    println!("patterns: parsed {parsed}, lowered {lowered}, unsupported assertions {unsupported}");
    assert!(
        lowered * 2 >= parsed,
        "fewer than half of parsed patterns reach an automaton"
    );
}
