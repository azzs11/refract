//! Character-set algebra over Unicode scalar values.
//!
//! A [`CharSet`] is a canonical, sorted list of inclusive code-point intervals. Canonical means:
//! intervals are sorted, non-empty, non-overlapping and non-adjacent, and never contain a
//! surrogate code point (U+D800..=U+DFFF). Two sets are therefore equal as sets exactly when they
//! are equal as values, which the automata layer relies on.
//!
//! The universe is every Unicode scalar value: `0..=0xD7FF` and `0xE000..=0x10FFFF`.

use std::fmt;

/// Largest Unicode scalar value.
pub const MAX_CHAR: u32 = 0x10_FFFF;
/// Internal pseudo code points `0x110000 + (u - 0xD800)` stand for lone UTF-16 surrogate code
/// units, so JavaScript without `u` can be modelled on code units (ADR-0007). They never occur
/// in a haystack of scalar values.
pub const PSEUDO_BASE: u32 = 0x11_0000;
pub const MAX_PSEUDO: u32 = PSEUDO_BASE + 0x7FF;
const SURROGATE_LO: u32 = 0xD800;
const SURROGATE_HI: u32 = 0xDFFF;

/// A set of Unicode scalar values stored as canonical inclusive intervals.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct CharSet {
    ranges: Vec<(u32, u32)>,
}

impl CharSet {
    /// The empty set.
    pub const fn empty() -> Self {
        CharSet { ranges: Vec::new() }
    }

    /// Every Unicode scalar value.
    pub fn full() -> Self {
        CharSet {
            ranges: vec![(0, SURROGATE_LO - 1), (SURROGATE_HI + 1, MAX_CHAR)],
        }
    }

    /// A set holding one character.
    pub fn single(c: char) -> Self {
        let c = c as u32;
        CharSet {
            ranges: vec![(c, c)],
        }
    }

    /// The inclusive range `lo..=hi`. Returns the empty set when `lo > hi`.
    pub fn range(lo: char, hi: char) -> Self {
        Self::from_ranges([(lo as u32, hi as u32)])
    }

    /// Builds a canonical set from arbitrary code-point intervals. Intervals may overlap, be
    /// unsorted, be reversed (dropped), exceed `MAX_CHAR` (clamped) or touch surrogates (removed).
    pub fn from_ranges<I: IntoIterator<Item = (u32, u32)>>(iter: I) -> Self {
        let mut raw: Vec<(u32, u32)> = Vec::new();
        for (lo, hi) in iter {
            let hi = hi.min(MAX_PSEUDO);
            if lo > hi {
                continue;
            }
            // Split around the surrogate block.
            if lo < SURROGATE_LO {
                raw.push((lo, hi.min(SURROGATE_LO - 1)));
            }
            if hi > SURROGATE_HI {
                raw.push((lo.max(SURROGATE_HI + 1), hi));
            }
        }
        raw.sort_unstable();
        let mut ranges: Vec<(u32, u32)> = Vec::with_capacity(raw.len());
        for (lo, hi) in raw {
            match ranges.last_mut() {
                // Merge overlapping or adjacent intervals. Adjacent across the surrogate gap is
                // also merged conceptually, but because we never store surrogates the stored
                // intervals stay split there; `is_adjacent` handles that.
                Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
                _ => ranges.push((lo, hi)),
            }
        }
        CharSet { ranges }
    }

    /// The canonical intervals.
    pub fn ranges(&self) -> &[(u32, u32)] {
        &self.ranges
    }

    /// True when the set has no members.
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// True when the set holds every scalar value.
    pub fn is_full(&self) -> bool {
        *self == Self::full()
    }

    /// Number of scalar values in the set.
    pub fn len(&self) -> u64 {
        self.ranges
            .iter()
            .map(|&(lo, hi)| u64::from(hi - lo) + 1)
            .sum()
    }

    /// Membership test, O(log n).
    pub fn contains(&self, c: char) -> bool {
        let c = c as u32;
        self.ranges
            .binary_search_by(|&(lo, hi)| {
                if hi < c {
                    std::cmp::Ordering::Less
                } else if lo > c {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }

    /// Smallest member, if any.
    pub fn min_char(&self) -> Option<char> {
        self.ranges.first().and_then(|&(lo, _)| char::from_u32(lo))
    }

    /// Iterates over every member in ascending order. Can be huge; callers should bound it.
    pub fn chars(&self) -> impl Iterator<Item = char> + '_ {
        self.ranges
            .iter()
            .flat_map(|&(lo, hi)| (lo..=hi).filter_map(char::from_u32))
    }

    /// Set union.
    pub fn union(&self, other: &CharSet) -> CharSet {
        CharSet::from_ranges(self.ranges.iter().chain(other.ranges.iter()).copied())
    }

    /// Set intersection.
    pub fn intersect(&self, other: &CharSet) -> CharSet {
        let (a, b) = (&self.ranges, &other.ranges);
        let (mut i, mut j) = (0, 0);
        let mut out = Vec::new();
        while i < a.len() && j < b.len() {
            let lo = a[i].0.max(b[j].0);
            let hi = a[i].1.min(b[j].1);
            if lo <= hi {
                out.push((lo, hi));
            }
            if a[i].1 < b[j].1 {
                i += 1;
            } else {
                j += 1;
            }
        }
        CharSet { ranges: out }
    }

    /// Complement relative to all scalar values (pseudo code units are never included).
    pub fn complement(&self) -> CharSet {
        let scalars = self.intersect(&CharSet::full());
        let mut out = Vec::with_capacity(scalars.ranges.len() + 2);
        let mut next = 0u32;
        for &(lo, hi) in &scalars.ranges {
            if lo > next {
                out.push((next, lo - 1));
            }
            next = hi + 1;
        }
        if next <= MAX_CHAR {
            out.push((next, MAX_CHAR));
        }
        // Re-canonicalise to strip the surrogate block from the gaps.
        CharSet::from_ranges(out)
    }

    /// Membership of any code point, including pseudo code units.
    pub fn contains_u32(&self, c: u32) -> bool {
        self.ranges
            .binary_search_by(|&(lo, hi)| {
                if hi < c {
                    std::cmp::Ordering::Less
                } else if lo > c {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }

    /// Every UTF-16 code unit: BMP scalars plus the pseudo points for surrogates.
    pub fn units() -> CharSet {
        CharSet::from_ranges([(0, 0xD7FF), (0xE000, 0xFFFF), (PSEUDO_BASE, MAX_PSEUDO)])
    }

    /// Complement relative to all UTF-16 code units.
    pub fn units_complement(&self) -> CharSet {
        CharSet::units().difference_raw(self)
    }

    /// `self \ other` without re-normalising to scalars (keeps pseudo points).
    pub fn difference_raw(&self, other: &CharSet) -> CharSet {
        let mut out = Vec::new();
        for &(lo, hi) in &self.ranges {
            let mut cur = lo;
            for &(olo, ohi) in &other.ranges {
                if ohi < cur || olo > hi {
                    continue;
                }
                if olo > cur {
                    out.push((cur, olo - 1));
                }
                cur = ohi.saturating_add(1);
                if cur > hi {
                    break;
                }
            }
            if cur <= hi {
                out.push((cur, hi));
            }
        }
        CharSet::from_ranges(out)
    }

    /// Set difference `self \ other`.
    pub fn difference(&self, other: &CharSet) -> CharSet {
        self.difference_raw(other)
    }

    /// Symmetric difference.
    pub fn symmetric_difference(&self, other: &CharSet) -> CharSet {
        self.union(other).difference(&self.intersect(other))
    }

    /// True when every member of `self` is in `other`.
    pub fn is_subset(&self, other: &CharSet) -> bool {
        self.difference(other).is_empty()
    }

    /// True when the two sets share a member.
    pub fn intersects(&self, other: &CharSet) -> bool {
        !self.intersect(other).is_empty()
    }
}

impl FromIterator<char> for CharSet {
    fn from_iter<I: IntoIterator<Item = char>>(iter: I) -> Self {
        CharSet::from_ranges(iter.into_iter().map(|c| (c as u32, c as u32)))
    }
}

/// Formats a code point for humans: printable ASCII as itself, everything else as `\u{..}`.
pub fn fmt_code_point(c: u32) -> String {
    if (PSEUDO_BASE..=MAX_PSEUDO).contains(&c) {
        return format!("\\u{{{:X}}}(unit)", c - PSEUDO_BASE + 0xD800);
    }
    match char::from_u32(c) {
        Some(ch) if ch.is_ascii_graphic() && !matches!(ch, '\\' | '-' | '[' | ']' | '^') => {
            ch.to_string()
        }
        Some(' ') => "' '".to_string(),
        Some('\n') => "\\n".to_string(),
        Some('\t') => "\\t".to_string(),
        Some('\r') => "\\r".to_string(),
        Some(ch) if matches!(ch, '\\' | '-' | '[' | ']' | '^') => format!("\\{ch}"),
        _ => format!("\\u{{{c:X}}}"),
    }
}

impl fmt::Debug for CharSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for CharSet {
    /// `[a-c]`, or `[^\n]` when the complement has fewer intervals, or `[ANY]`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_full() {
            return f.write_str("[ANY]");
        }
        let complement = self.complement();
        // The surrogate gap splits the complement of a small set into one extra interval.
        let (negated, ranges) = if complement.ranges.len() + 1 < self.ranges.len()
            || (self.ranges.len() >= 2 && complement.len() < 0x100)
        {
            (true, complement.ranges)
        } else {
            (false, self.ranges.clone())
        };
        f.write_str(if negated { "[^" } else { "[" })?;
        for (lo, hi) in ranges {
            if lo == hi {
                f.write_str(&fmt_code_point(lo))?;
            } else {
                write!(f, "{}-{}", fmt_code_point(lo), fmt_code_point(hi))?;
            }
        }
        f.write_str("]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn arb_set() -> impl Strategy<Value = CharSet> {
        // Bias towards small code points and the surrogate/max boundaries.
        let point = prop_oneof![
            0u32..200,
            0xD700u32..0xE100,
            0x10_FF00u32..=MAX_CHAR,
            0u32..=MAX_CHAR,
        ];
        prop::collection::vec((point.clone(), point), 0..6)
            .prop_map(|v| CharSet::from_ranges(v.into_iter().map(|(a, b)| (a.min(b), a.max(b)))))
    }

    fn arb_char() -> impl Strategy<Value = char> {
        prop_oneof![
            (0u32..200).prop_filter_map("scalar", char::from_u32),
            (0xD700u32..0xE100).prop_filter_map("scalar", char::from_u32),
            any::<char>(),
        ]
    }

    fn is_canonical(s: &CharSet) -> bool {
        s.ranges
            .iter()
            .all(|&(lo, hi)| lo <= hi && hi <= MAX_CHAR && (hi < SURROGATE_LO || lo > SURROGATE_HI))
            && s.ranges.windows(2).all(|w| w[0].1 + 1 < w[1].0)
    }

    #[test]
    fn full_and_empty() {
        assert!(CharSet::empty().is_empty());
        assert!(CharSet::full().is_full());
        assert_eq!(CharSet::full().complement(), CharSet::empty());
        assert_eq!(CharSet::empty().complement(), CharSet::full());
        assert_eq!(CharSet::full().len(), 0x10_FFFF + 1 - 0x800);
    }

    #[test]
    fn surrogates_are_removed() {
        let s = CharSet::from_ranges([(0xD000, 0xE000)]);
        assert_eq!(s.ranges(), &[(0xD000, 0xD7FF), (0xE000, 0xE000)]);
        assert!(CharSet::from_ranges([(0xD800, 0xDFFF)]).is_empty());
    }

    #[test]
    fn display() {
        let s = CharSet::from_iter("abcx-".chars());
        assert_eq!(s.to_string(), "[\\-a-cx]");
        let dot = CharSet::full().difference(&CharSet::single('\n'));
        assert_eq!(dot.to_string(), "[^\\n]");
        let not_ab = CharSet::full().difference(&CharSet::range('a', 'b'));
        assert_eq!(not_ab.to_string(), "[^a-b]");
    }

    proptest! {
        #[test]
        fn ops_are_canonical(a in arb_set(), b in arb_set()) {
            prop_assert!(is_canonical(&a));
            prop_assert!(is_canonical(&a.union(&b)));
            prop_assert!(is_canonical(&a.intersect(&b)));
            prop_assert!(is_canonical(&a.complement()));
            prop_assert!(is_canonical(&a.difference(&b)));
        }

        #[test]
        fn membership_matches_definition(a in arb_set(), b in arb_set(), c in arb_char()) {
            prop_assert_eq!(a.union(&b).contains(c), a.contains(c) || b.contains(c));
            prop_assert_eq!(a.intersect(&b).contains(c), a.contains(c) && b.contains(c));
            prop_assert_eq!(a.complement().contains(c), !a.contains(c));
            prop_assert_eq!(a.difference(&b).contains(c), a.contains(c) && !b.contains(c));
            prop_assert_eq!(a.symmetric_difference(&b).contains(c), a.contains(c) != b.contains(c));
        }

        #[test]
        fn boolean_laws(a in arb_set(), b in arb_set(), c in arb_set()) {
            prop_assert_eq!(a.complement().complement(), a.clone());
            prop_assert_eq!(a.union(&b), b.union(&a));
            prop_assert_eq!(a.intersect(&b), b.intersect(&a));
            prop_assert_eq!(a.union(&b).complement(), a.complement().intersect(&b.complement()));
            prop_assert_eq!(a.intersect(&b.union(&c)), a.intersect(&b).union(&a.intersect(&c)));
            prop_assert!(a.intersect(&a.complement()).is_empty());
            prop_assert!(a.union(&a.complement()).is_full());
        }

        #[test]
        fn len_is_additive(a in arb_set(), b in arb_set()) {
            prop_assert_eq!(a.union(&b).len() + a.intersect(&b).len(), a.len() + b.len());
        }
    }
}
