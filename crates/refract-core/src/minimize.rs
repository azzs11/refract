//! DFA minimisation, implemented twice so the results can be checked against each other
//! (engineering rule: two implementations for anything that decides).
//!
//! - [`hopcroft`]: partition refinement, O(k·n log n).
//! - [`brzozowski`]: determinise the reverse, twice. Exponential in the worst case but tiny and
//!   obviously correct, which is what a cross-check needs.

use crate::budget::Budget;
use crate::dfa::Dfa;
use crate::error::Result;
use std::collections::{HashMap, VecDeque};

/// Hopcroft's algorithm on the reachable part of `dfa`. The result is complete and numbered
/// canonically.
pub fn hopcroft(dfa: &Dfa) -> Dfa {
    let d = dfa.trim();
    let n = d.len();
    let k = d.k();
    if n == 0 {
        return d;
    }

    // inverse[c][q] = states p with δ(p, c) = q
    let mut inverse: Vec<Vec<Vec<usize>>> = vec![vec![Vec::new(); n]; k];
    for p in 0..n {
        for (c, inv) in inverse.iter_mut().enumerate() {
            inv[d.delta[p * k + c]].push(p);
        }
    }

    // Initial partition: accepting vs non-accepting (drop an empty side).
    let mut block_of = vec![0usize; n];
    let mut blocks: Vec<Vec<usize>> = Vec::new();
    let acc: Vec<usize> = (0..n).filter(|&q| d.accepting[q]).collect();
    let rej: Vec<usize> = (0..n).filter(|&q| !d.accepting[q]).collect();
    for b in [acc, rej] {
        if !b.is_empty() {
            let id = blocks.len();
            for &q in &b {
                block_of[q] = id;
            }
            blocks.push(b);
        }
    }

    let mut worklist: VecDeque<(usize, usize)> = VecDeque::new();
    let smallest = (0..blocks.len())
        .min_by_key(|&b| blocks[b].len())
        .unwrap_or(0);
    for b in 0..blocks.len() {
        if blocks.len() == 1 || b == smallest {
            for c in 0..k {
                worklist.push_back((b, c));
            }
        }
    }

    while let Some((a, c)) = worklist.pop_front() {
        // X = states with a c-transition into block a.
        let mut hit: HashMap<usize, Vec<usize>> = HashMap::new();
        for &q in &blocks[a] {
            for &p in &inverse[c][q] {
                hit.entry(block_of[p]).or_default().push(p);
            }
        }
        let mut touched: Vec<usize> = hit.keys().copied().collect();
        touched.sort_unstable(); // determinism
        for y in touched {
            let mut inside = hit.remove(&y).unwrap_or_default();
            inside.sort_unstable();
            inside.dedup();
            if inside.len() == blocks[y].len() {
                continue; // no split
            }
            let outside: Vec<usize> = blocks[y]
                .iter()
                .copied()
                .filter(|q| inside.binary_search(q).is_err())
                .collect();
            // Keep the larger part as `y`, the smaller becomes the new block.
            let (keep, split) = if inside.len() > outside.len() {
                (inside, outside)
            } else {
                (outside, inside)
            };
            let new_id = blocks.len();
            for &q in &split {
                block_of[q] = new_id;
            }
            blocks[y] = keep;
            blocks.push(split);
            // If (y, c) is pending, both halves must be: add the new one. If not, adding only
            // the smaller half suffices, which is the new one by construction. Either way:
            for cc in 0..k {
                worklist.push_back((new_id, cc));
            }
        }
    }

    let m = blocks.len();
    let mut delta = vec![0usize; m * k];
    let mut accepting = vec![false; m];
    for (b, members) in blocks.iter().enumerate() {
        let q = members[0];
        accepting[b] = d.accepting[q];
        for c in 0..k {
            delta[b * k + c] = block_of[d.delta[q * k + c]];
        }
    }
    Dfa {
        alphabet: d.alphabet.clone(),
        start: block_of[d.start],
        accepting,
        delta,
    }
    .trim()
}

/// Determinises the reverse of `dfa`: states are subsets S of `dfa`'s states, the start is the
/// set of accepting states, δ'(S, c) = { p | δ(p, c) ∈ S }, and S accepts when it contains the
/// original start.
fn reverse_determinize(dfa: &Dfa, budget: &Budget) -> Result<Dfa> {
    let n = dfa.len();
    let k = dfa.k();
    let mut inverse: Vec<Vec<Vec<usize>>> = vec![vec![Vec::new(); n]; k];
    for p in 0..n {
        for (c, inv) in inverse.iter_mut().enumerate() {
            inv[dfa.delta[p * k + c]].push(p);
        }
    }
    let start: Vec<usize> = (0..n).filter(|&q| dfa.accepting[q]).collect();
    let mut id: HashMap<Vec<usize>, usize> = HashMap::new();
    let mut subsets = vec![start.clone()];
    id.insert(start, 0);
    let mut delta = Vec::new();
    let mut i = 0;
    while i < subsets.len() {
        budget.check_time()?;
        for inv in &inverse {
            let mut t: Vec<usize> = subsets[i]
                .iter()
                .flat_map(|&q| inv[q].iter().copied())
                .collect();
            t.sort_unstable();
            t.dedup();
            let next_id = subsets.len();
            let tid = *id.entry(t.clone()).or_insert_with(|| {
                subsets.push(t);
                next_id
            });
            delta.push(tid);
        }
        budget.check(
            "DFA states (Brzozowski)",
            subsets.len(),
            budget.max_dfa_states,
        )?;
        i += 1;
    }
    let accepting = subsets
        .iter()
        .map(|s| s.binary_search(&dfa.start).is_ok())
        .collect();
    Ok(Dfa {
        alphabet: dfa.alphabet.clone(),
        start: 0,
        accepting,
        delta,
    })
}

/// Brzozowski's algorithm: `det(rev(det(rev(A))))` is minimal.
pub fn brzozowski(dfa: &Dfa, budget: &Budget) -> Result<Dfa> {
    let once = reverse_determinize(dfa, budget)?;
    Ok(reverse_determinize(&once, budget)?.trim())
}
