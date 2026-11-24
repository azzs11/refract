//! Exact UTF-16 semantics for JavaScript without `u`/`v` (ADR-0007).
//!
//! Such a pattern matches **code units**: lowering gives a regular language `L_u` over code
//! units, where surrogate units are pseudo code points (`charset::PSEUDO_BASE`). A haystack of
//! scalar values `w` matches when its UTF-16 encoding `h(w)` is in `L_u`, i.e. the scalar
//! language is the inverse homomorphism `h⁻¹(L_u)`, which is regular: from the code-unit DFA,
//! a scalar `c` steps by `δ(q, c)` when `c` is in the BMP and by `δ(δ(q, hi(c)), lo(c))` when
//! it is astral. For that to be well defined per minterm, astral characters are partitioned by
//! the code-unit classes of their surrogate halves.

use crate::alphabet::Alphabet;
use crate::budget::Budget;
use crate::charset::{CharSet, PSEUDO_BASE};
use crate::deriv;
use crate::dfa::Dfa;
use crate::error::Result;
use crate::regular::Re;
use std::sync::Arc;

/// Surrogate halves of an astral scalar value, as pseudo code units.
fn halves(c: u32) -> (u32, u32) {
    let v = c - 0x1_0000;
    (PSEUDO_BASE + (v >> 10), PSEUDO_BASE + 0x400 + (v & 0x3FF))
}

/// A code-unit DFA ready to be lifted.
pub struct UnitDfa {
    dfa: Dfa,
}

impl UnitDfa {
    /// The derivative DFA of a code-unit language.
    pub fn new(lang: &Re, budget: &Budget) -> Result<UnitDfa> {
        // The unit alphabet must span every code unit, surrogates included, even when the
        // pattern never mentions one (otherwise surrogate halves would fall into some class).
        let mut sets = vec![CharSet::units()];
        lang.collect_sets(&mut sets);
        let alpha = Arc::new(Alphabet::new(&sets));
        Ok(UnitDfa {
            dfa: deriv::to_dfa(lang, &alpha, budget)?,
        })
    }

    /// Sets of scalar values on which this automaton behaves uniformly: each BMP code-unit
    /// class, and for every (high class, low class) pair the astral characters with those halves.
    pub fn cells(&self) -> Vec<CharSet> {
        let a = &self.dfa.alphabet;
        let bmp = CharSet::from_ranges([(0, 0xFFFF)]);
        let mut cells: Vec<CharSet> = (0..a.len()).map(|i| a.class(i).intersect(&bmp)).collect();
        // High surrogates D800..DBFF -> pseudo 0..0x3FF; group high values by class.
        let mut by_hi: std::collections::BTreeMap<usize, Vec<u32>> = Default::default();
        for h in 0..0x400u32 {
            by_hi
                .entry(a.class_of_u32(PSEUDO_BASE + h))
                .or_default()
                .push(h);
        }
        let mut lo_classes: std::collections::BTreeMap<usize, Vec<(u32, u32)>> = Default::default();
        for l in 0..0x400u32 {
            let k = a.class_of_u32(PSEUDO_BASE + 0x400 + l);
            let v = lo_classes.entry(k).or_default();
            match v.last_mut() {
                Some(last) if last.1 + 1 == l => last.1 = l,
                _ => v.push((l, l)),
            }
        }
        for hs in by_hi.values() {
            for los in lo_classes.values() {
                let mut ranges = Vec::new();
                for &h in hs {
                    let base = 0x1_0000 + (h << 10);
                    for &(l0, l1) in los {
                        ranges.push((base + l0, base + l1));
                    }
                }
                cells.push(CharSet::from_ranges(ranges));
            }
        }
        cells.retain(|c| !c.is_empty());
        cells
    }

    /// Lifts to a complete DFA over `alphabet` (which must refine [`UnitDfa::cells`]).
    pub fn lift(&self, alphabet: &Arc<Alphabet>) -> Dfa {
        let u = &self.dfa;
        let k = alphabet.len();
        let mut delta = Vec::with_capacity(u.len() * k);
        for q in 0..u.len() {
            for c in 0..k {
                let rep = alphabet.class(c).ranges().first().map_or(0, |r| r.0);
                let t = if rep <= 0xFFFF {
                    u.next(q, u.alphabet.class_of_u32(rep))
                } else {
                    let (hi, lo) = halves(rep);
                    let mid = u.next(q, u.alphabet.class_of_u32(hi));
                    u.next(mid, u.alphabet.class_of_u32(lo))
                };
                delta.push(t);
            }
        }
        Dfa {
            alphabet: alphabet.clone(),
            start: u.start,
            accepting: u.accepting.clone(),
            delta,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::automata::regex_dfa;
    use crate::{Budget, Flavor, Op, parse};

    /// Regression (found by the V8 oracle at 100k cases): a surrogate half must never be
    /// mistaken for a character the pattern mentions.
    #[test]
    fn astral_halves_are_their_own_units() {
        let b = Budget::default();
        for (p, input, want) in [
            (r"\0{1,3}", "😀", false),
            (r"^.$", "😀", false),
            (r"^..$", "😀", true),
            (r"^😀$", "😀", true),
            (r"^[\uD800-\uDBFF]", "😀", true),
            (r"^[^a]{2}$", "😀", true),
        ] {
            let re = parse(Flavor::JavaScript, p).unwrap();
            let d = regex_dfa(&re, Op::FullMatch, &b).unwrap();
            let s = regex_dfa(&re, Op::Search, &b).unwrap();
            let got = if p.starts_with('^') {
                s.accepts(input)
            } else {
                d.accepts(input)
            };
            assert_eq!(got, want, "{p} on {input}");
        }
    }
}
