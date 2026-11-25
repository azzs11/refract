//! Test generation (plan §4): strings that cover every state and every distinct transition of
//! the minimal DFA, each labelled with whether the pattern matches it under the operation.
//!
//! For an edge `s --c--> t` the case is `access(s) · rep(c) · finish(t)`: the shortest
//! readable string reaching `s`, the most readable character of the minterm `c`, then the
//! shortest readable way from `t` to acceptance (empty when `t` is dead). Edges that lead to the
//! same target from the same state are one edge (symbolic automata). Cases are deduplicated
//! and sorted (length, then text), so output is stable.

use crate::automata::regex_dfa;
use crate::budget::Budget;
use crate::dfa::Dfa;
use crate::error::{Error, Result};
use crate::ir::Regex;
use crate::minimize::hopcroft;
use crate::regular::Op;
use std::collections::{BTreeSet, VecDeque};

/// One generated test case.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Case {
    pub text: String,
    pub matches: bool,
}

/// Coverage cases for `re` under `op`. Patterns outside the regular fragment (back-references)
/// are `Unsupported`.
pub fn cover(re: &Regex, op: Op, budget: &Budget) -> Result<Vec<Case>> {
    let dfa = hopcroft(&regex_dfa(re, op, budget)?);
    let n = dfa.len();
    let k = dfa.k();
    let order = dfa.alphabet.readable_order().to_vec();
    let rep = |c: usize| dfa.alphabet.representative(c);

    // Shortest readable access strings (BFS in readable minterm order).
    let mut access: Vec<Option<String>> = vec![None; n];
    access[dfa.start] = Some(String::new());
    let mut queue = VecDeque::from([dfa.start]);
    while let Some(s) = queue.pop_front() {
        budget.check_time()?;
        let base = access[s].clone().unwrap_or_default();
        for &c in &order {
            let t = dfa.next(s, c);
            if access[t].is_none() {
                access[t] = Some(format!("{base}{}", rep(c)));
                queue.push_back(t);
            }
        }
    }

    // Distance to acceptance (reverse BFS), then a greedy readable walk that decreases it.
    let dist = distance_to_accept(&dfa);
    let finish = |mut s: usize| -> String {
        let mut out = String::new();
        while let Some(d) = dist[s]
            && d > 0
        {
            let c = order
                .iter()
                .copied()
                .find(|&c| dist[dfa.next(s, c)] == Some(d - 1))
                .unwrap_or(order[0]);
            out.push(rep(c));
            s = dfa.next(s, c);
        }
        out
    };

    let mut cases = BTreeSet::new();
    let push = |cases: &mut BTreeSet<(usize, Case)>, text: String| {
        let matches = dfa.accepts(&text);
        cases.insert((text.chars().count(), Case { text, matches }));
    };
    for (s, a) in access.iter().enumerate() {
        let Some(a) = a.clone() else { continue };
        push(&mut cases, format!("{a}{}", finish(s)));
        let mut seen = BTreeSet::new();
        for &c in &order {
            let t = dfa.next(s, c);
            if seen.insert(t) {
                push(&mut cases, format!("{a}{}{}", rep(c), finish(t)));
            }
        }
        if cases.len() > budget.max_dfa_states.saturating_mul(k.max(1)) {
            return Err(Error::budget("too many test cases"));
        }
    }
    Ok(cases.into_iter().map(|(_, c)| c).collect())
}

/// Cases for patterns outside the regular fragment (back-references): every string up to
/// length 4 over the bounded-check alphabet (ADR-0011), labelled by the interpreter; the
/// shortest `per_side` matches and non-matches are kept. Not a coverage guarantee.
pub fn sample(re: &Regex, op: Op, per_side: usize, budget: &Budget) -> Result<Vec<Case>> {
    let alphabet = crate::equiv::bounded_alphabet(re, re);
    let (mut yes, mut no) = (Vec::new(), Vec::new());
    let mut level = vec![String::new()];
    for _ in 0..=4 {
        let mut next = Vec::new();
        for s in &level {
            budget.check_time()?;
            let m = crate::interp::is_match(re, op, s, 200_000)?;
            let side = if m { &mut yes } else { &mut no };
            if side.len() < per_side {
                side.push(Case {
                    text: s.clone(),
                    matches: m,
                });
            }
            if next.len() < 4096 {
                next.extend(alphabet.iter().map(|c| format!("{s}{c}")));
            }
        }
        if yes.len() >= per_side && no.len() >= per_side {
            break;
        }
        level = next;
    }
    yes.extend(no);
    Ok(yes)
}

fn distance_to_accept(dfa: &Dfa) -> Vec<Option<usize>> {
    let n = dfa.len();
    let mut rev: Vec<Vec<usize>> = vec![Vec::new(); n];
    for s in 0..n {
        for c in 0..dfa.k() {
            rev[dfa.next(s, c)].push(s);
        }
    }
    let mut dist = vec![None; n];
    let mut queue = VecDeque::new();
    for (s, &acc) in dfa.accepting.iter().enumerate() {
        if acc {
            dist[s] = Some(0);
            queue.push_back(s);
        }
    }
    while let Some(t) = queue.pop_front() {
        let d = dist[t].unwrap_or(0);
        for &s in &rev[t] {
            if dist[s].is_none() {
                dist[s] = Some(d + 1);
                queue.push_back(s);
            }
        }
    }
    dist
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flavor::{Flavor, parse};

    fn gen_cases(p: &str, op: Op) -> Vec<Case> {
        cover(&parse(Flavor::Python, p).unwrap(), op, &Budget::default()).unwrap()
    }

    #[test]
    fn covers_both_sides_of_an_alternation() {
        let cs = gen_cases(r"ab|cd", Op::FullMatch);
        let texts: Vec<_> = cs
            .iter()
            .filter(|c| c.matches)
            .map(|c| c.text.as_str())
            .collect();
        assert!(texts.contains(&"ab") && texts.contains(&"cd"), "{cs:?}");
        assert!(cs.iter().any(|c| !c.matches));
    }

    #[test]
    fn labels_agree_with_the_dfa_and_cover_every_edge() {
        for p in [r"\d{3}-\d{4}", r"[a-z]+@[a-z]+\.com", r"(?:x|y)*z?", r"^a$"] {
            for op in [Op::FullMatch, Op::Search] {
                let re = parse(Flavor::Python, p).unwrap();
                let dfa = hopcroft(&regex_dfa(&re, op, &Budget::default()).unwrap());
                let cs = cover(&re, op, &Budget::default()).unwrap();
                // Every distinct reachable edge is taken by some case.
                let mut taken = BTreeSet::new();
                for c in &cs {
                    assert_eq!(dfa.accepts(&c.text), c.matches);
                    let mut q = dfa.start;
                    for ch in c.text.chars() {
                        let t = dfa.next(q, dfa.alphabet.class_of(ch));
                        taken.insert((q, t));
                        q = t;
                    }
                }
                let canon_edges: BTreeSet<_> = reachable_edges(&dfa);
                assert!(canon_edges.is_subset(&taken), "{p} {op:?}");
            }
        }
    }

    fn reachable_edges(dfa: &Dfa) -> BTreeSet<(usize, usize)> {
        let mut seen = vec![false; dfa.len()];
        let mut stack = vec![dfa.start];
        seen[dfa.start] = true;
        let mut edges = BTreeSet::new();
        while let Some(s) = stack.pop() {
            for c in 0..dfa.k() {
                let t = dfa.next(s, c);
                edges.insert((s, t));
                if !seen[t] {
                    seen[t] = true;
                    stack.push(t);
                }
            }
        }
        edges
    }
}
