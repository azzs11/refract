//! Shared generators for property tests.
#![allow(dead_code)]

use proptest::prelude::*;
use refract_core::{CharSet, Re};

/// Case count: `PROPTEST_CASES` when set, else `default`.
pub fn cases(default: u32) -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// Small character sets over a tiny alphabet, so random terms interact a lot.
pub fn arb_set() -> impl Strategy<Value = CharSet> {
    prop_oneof![
        4 => prop::sample::select(vec!['a', 'b', 'c']).prop_map(CharSet::single),
        2 => (0u8..3, 0u8..3).prop_map(|(x, y)| {
            let (lo, hi) = (x.min(y), x.max(y));
            CharSet::range((b'a' + lo) as char, (b'a' + hi) as char)
        }),
        1 => Just(CharSet::full()),
        1 => Just(CharSet::full().difference(&CharSet::single('a'))),
        1 => Just(CharSet::empty()),
    ]
}

/// Random regular terms with every constructor, including intersection and complement.
pub fn arb_re() -> impl Strategy<Value = Re> {
    let leaf = prop_oneof![
        6 => arb_set().prop_map(Re::Set),
        1 => Just(Re::Eps),
        1 => Just(Re::Nothing),
    ];
    leaf.prop_recursive(4, 24, 3, |inner| {
        prop_oneof![
            3 => prop::collection::vec(inner.clone(), 2..4).prop_map(Re::Concat),
            3 => prop::collection::vec(inner.clone(), 2..4).prop_map(Re::Alt),
            2 => (inner.clone(), 0u32..3, prop::option::of(0u32..3)).prop_map(|(r, min, extra)| {
                Re::Repeat(Box::new(r), min, extra.map(|e| min + e))
            }),
            1 => prop::collection::vec(inner.clone(), 2..3).prop_map(Re::And),
            1 => inner.prop_map(|r| Re::Not(Box::new(r))),
        ]
    })
}

/// Terms without And/Not (the classic regular fragment).
pub fn arb_classic_re() -> impl Strategy<Value = Re> {
    let leaf = prop_oneof![
        6 => arb_set().prop_map(Re::Set),
        1 => Just(Re::Eps),
    ];
    leaf.prop_recursive(4, 24, 3, |inner| {
        prop_oneof![
            3 => prop::collection::vec(inner.clone(), 2..4).prop_map(Re::Concat),
            3 => prop::collection::vec(inner.clone(), 2..4).prop_map(Re::Alt),
            2 => (inner, 0u32..3, prop::option::of(0u32..3)).prop_map(|(r, min, extra)| {
                Re::Repeat(Box::new(r), min, extra.map(|e| min + e))
            }),
        ]
    })
}

/// Short strings over the same tiny alphabet plus one outsider.
pub fn arb_input() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(vec!['a', 'b', 'c', 'x']), 0..7)
        .prop_map(|v| v.into_iter().collect())
}
