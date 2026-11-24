//! Minterm alphabets (ADR-0003).
//!
//! Given every character set that occurs in the terms being compared, the minterms are the
//! finest partition of the code-point space such that each set is a union of minterms. Automata
//! then run over minterm *indices*, which keeps them small (a handful of symbols instead of 1.1
//! million) and makes DFAs built by different engines directly comparable: both must use the
//! same [`Alphabet`] (doc review item 7).

use crate::charset::{CharSet, MAX_PSEUDO};
use crate::unicode;
use std::collections::HashMap;

/// A fixed-size set of small integers (minterm indices or automaton states).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct BitSet {
    words: Vec<u64>,
}

impl BitSet {
    pub fn new(len: usize) -> Self {
        BitSet {
            words: vec![0; len.div_ceil(64)],
        }
    }

    pub fn insert(&mut self, i: usize) {
        self.words[i / 64] |= 1 << (i % 64);
    }

    pub fn contains(&self, i: usize) -> bool {
        self.words
            .get(i / 64)
            .is_some_and(|w| w & (1 << (i % 64)) != 0)
    }

    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|&w| w == 0)
    }

    /// The raw 64-bit words (for fast multi-way intersection tests).
    pub fn words(&self) -> &[u64] {
        &self.words
    }

    pub fn union_with(&mut self, other: &BitSet) {
        for (a, b) in self.words.iter_mut().zip(&other.words) {
            *a |= b;
        }
    }

    pub fn intersect_with(&mut self, other: &BitSet) {
        for (a, b) in self.words.iter_mut().zip(&other.words) {
            *a &= b;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.words.iter().enumerate().flat_map(|(wi, &w)| {
            (0..64)
                .filter(move |b| w & (1 << b) != 0)
                .map(move |b| wi * 64 + b)
        })
    }
}

/// How readable a character is in a witness string; lower is better.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Readability {
    LowerAscii,
    UpperAscii,
    Digit,
    Punctuation,
    Space,
    ReadableUnicode,
    Newline,
    Control,
    Other,
}

/// A minterm partition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alphabet {
    classes: Vec<CharSet>,
    reps: Vec<(Readability, u32)>,
    /// Class indices sorted from most to least readable representative.
    order: Vec<usize>,
    /// Sorted, disjoint `(lo, hi, class)` intervals covering every scalar value.
    intervals: Vec<(u32, u32, usize)>,
}

impl Alphabet {
    /// Builds the minterms of `sets`. With no sets the alphabet has a single class: everything.
    pub fn new(sets: &[CharSet]) -> Self {
        // Distinct sets only: duplicates do not refine the partition.
        let mut uniq: Vec<&CharSet> = sets.iter().collect();
        uniq.sort();
        uniq.dedup();

        // Pseudo code units (JS without `u`) join the universe only when some set uses them;
        // otherwise every string over the alphabet is a string of scalar values.
        let top = if uniq.iter().any(|s| {
            s.ranges()
                .last()
                .is_some_and(|r| r.1 > crate::charset::MAX_CHAR)
        }) {
            MAX_PSEUDO
        } else {
            crate::charset::MAX_CHAR
        };
        let mut cuts: Vec<u32> = vec![0, 0xD800, 0xE000];
        for s in &uniq {
            for &(lo, hi) in s.ranges() {
                cuts.push(lo);
                if hi < top {
                    cuts.push(hi + 1);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();

        let mut by_signature: HashMap<Vec<bool>, usize> = HashMap::new();
        let mut members: Vec<Vec<(u32, u32)>> = Vec::new();
        let mut intervals = Vec::new();
        for (k, &lo) in cuts.iter().enumerate() {
            let hi = cuts.get(k + 1).map_or(top, |&next| next - 1);
            if lo > top {
                continue;
            }
            if (0xD800..=0xDFFF).contains(&lo) {
                continue; // surrogates are not scalar values
            }
            let sig: Vec<bool> = uniq.iter().map(|s| s.contains_u32(lo)).collect();
            let next_id = members.len();
            let id = *by_signature.entry(sig).or_insert(next_id);
            if id == next_id {
                members.push(Vec::new());
            }
            members[id].push((lo, hi));
            intervals.push((lo, hi, id));
        }
        let classes: Vec<CharSet> = members.into_iter().map(CharSet::from_ranges).collect();
        let reps: Vec<(Readability, u32)> = classes
            .iter()
            .map(|c| match c.min_char() {
                Some(_) => {
                    let (r, ch) = best_char(c);
                    (r, ch as u32)
                }
                // Only pseudo code units (ADR-0007): no scalar to spell it with.
                None => (Readability::Other, c.ranges().first().map_or(0, |r| r.0)),
            })
            .collect();
        let mut order: Vec<usize> = (0..classes.len()).collect();
        order.sort_by_key(|&i| reps[i]);
        Alphabet {
            classes,
            reps,
            order,
            intervals,
        }
    }

    /// Number of minterms.
    pub fn len(&self) -> usize {
        self.classes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }

    pub fn class(&self, i: usize) -> &CharSet {
        &self.classes[i]
    }

    /// The minterm containing `c`.
    pub fn class_of(&self, c: char) -> usize {
        self.class_of_u32(c as u32)
    }

    /// The minterm of any code point, including pseudo code units.
    pub fn class_of_u32(&self, c: u32) -> usize {
        let i = self.intervals.partition_point(|&(_, hi, _)| hi < c);
        self.intervals.get(i).map_or(0, |&(_, _, id)| id)
    }

    /// The minterms making up `set`. `set` must be one of the sets the alphabet was built from
    /// (or a union of minterms); otherwise the result over-approximates.
    pub fn mask(&self, set: &CharSet) -> BitSet {
        let mut m = BitSet::new(self.len());
        for (i, (_, c)) in self.reps.iter().enumerate() {
            if set.contains_u32(*c) {
                m.insert(i);
            }
        }
        m
    }

    /// The most readable member of a minterm, used to spell witnesses.
    pub fn representative(&self, i: usize) -> char {
        char::from_u32(self.reps[i].1).unwrap_or('\u{FFFD}')
    }

    /// The representative as a code point (may be a pseudo code unit).
    pub fn representative_u32(&self, i: usize) -> u32 {
        self.reps[i].1
    }

    /// Minterm indices from most to least readable. Breadth-first searches visit edges in this
    /// order, so the first shortest witness found is also the most readable one.
    pub fn readable_order(&self) -> &[usize] {
        &self.order
    }
}

/// The most readable scalar member of `set` (`None` when it has no scalar value, e.g. only
/// surrogate code units).
pub fn readable_char(set: &CharSet) -> Option<char> {
    let scalars = set.intersect(&CharSet::full());
    if scalars.is_empty() {
        return None;
    }
    Some(best_char(&scalars).1)
}

fn best_char(class: &CharSet) -> (Readability, char) {
    use Readability::*;
    let groups: [(Readability, CharSet); 7] = [
        (LowerAscii, CharSet::range('a', 'z')),
        (UpperAscii, CharSet::range('A', 'Z')),
        (Digit, CharSet::range('0', '9')),
        (
            Punctuation,
            unicode::posix_class("punct").unwrap_or_default(),
        ),
        (Space, CharSet::single(' ')),
        (Newline, CharSet::single('\n')),
        (Control, CharSet::range('\0', '\x7F')),
    ];
    for (rank, g) in &groups[..5] {
        if let Some(c) = class.intersect(g).min_char() {
            return (*rank, c);
        }
    }
    if let Some(c) = class.intersect(unicode::readable()).min_char() {
        return (ReadableUnicode, c);
    }
    for (rank, g) in &groups[5..] {
        if let Some(c) = class.intersect(g).min_char() {
            return (*rank, c);
        }
    }
    (Other, class.min_char().unwrap_or('\0'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn partition_basics() {
        let a = CharSet::range('a', 'm');
        let b = CharSet::range('h', 'z');
        let alpha = Alphabet::new(&[a.clone(), b.clone()]);
        // [a-g], [h-m], [n-z], everything else.
        assert_eq!(alpha.len(), 4);
        assert_eq!(alpha.mask(&a).iter().count(), 2);
        assert_eq!(alpha.class_of('a'), alpha.class_of('g'));
        assert_ne!(alpha.class_of('g'), alpha.class_of('h'));
        // "everything else" is spelled with a readable char, not NUL.
        let other = alpha.class_of('!');
        assert_eq!(alpha.representative(other), 'A');
    }

    #[test]
    fn no_sets() {
        let alpha = Alphabet::new(&[]);
        assert_eq!(alpha.len(), 1);
        assert_eq!(alpha.representative(0), 'a');
    }

    proptest! {
        #[test]
        fn minterms_partition_everything(
            sets in prop::collection::vec(
                prop::collection::vec((0u32..300, 0u32..300), 0..4).prop_map(|v| {
                    CharSet::from_ranges(v.into_iter().map(|(a, b)| (a.min(b), a.max(b))))
                }),
                0..5,
            ),
            probe in prop::collection::vec(any::<char>(), 0..20),
        ) {
            let alpha = Alphabet::new(&sets);
            // Disjoint and covering.
            let mut total = CharSet::empty();
            for i in 0..alpha.len() {
                prop_assert!(!total.intersects(alpha.class(i)));
                total = total.union(alpha.class(i));
                prop_assert!(alpha.class(i).contains(alpha.representative(i)));
            }
            // Every scalar value (and the internal pseudo code units) is covered.
            prop_assert!(total.intersect(&CharSet::full()).is_full());
            prop_assert!(!total.contains_u32(crate::charset::PSEUDO_BASE), "no pseudo points unless used");
            // Every input set is exactly the union of its mask.
            for s in &sets {
                let m = alpha.mask(s);
                let rebuilt = m.iter().fold(CharSet::empty(), |acc, i| acc.union(alpha.class(i)));
                prop_assert_eq!(&rebuilt, s);
            }
            for c in probe {
                prop_assert!(alpha.class(alpha.class_of(c)).contains(c));
            }
        }
    }
}
