//! Reference engine: Thompson NFA + subset construction (doc §5.3).
//!
//! Deliberately simple and independent of the derivative engine: it shares only the alphabet
//! and the DFA type. Intersection and complement have no Thompson construction, so for those
//! sub-terms we determinise, combine with a product or complement, and embed the resulting DFA
//! back into the NFA.

use crate::alphabet::{Alphabet, BitSet};
use crate::budget::Budget;
use crate::dfa::Dfa;
use crate::error::Result;
use crate::regular::Re;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
struct State {
    eps: Vec<usize>,
    /// (minterm mask, target)
    edges: Vec<(BitSet, usize)>,
}

/// An NFA with a single start and a single accepting state.
#[derive(Clone, Debug)]
pub struct Nfa {
    states: Vec<State>,
    start: usize,
    accept: usize,
}

struct Builder<'a> {
    alphabet: &'a Arc<Alphabet>,
    budget: &'a Budget,
    states: Vec<State>,
}

impl Builder<'_> {
    fn add(&mut self) -> Result<usize> {
        self.states.push(State::default());
        self.budget
            .check("NFA states", self.states.len(), self.budget.max_nfa_states)?;
        Ok(self.states.len() - 1)
    }

    fn eps(&mut self, from: usize, to: usize) {
        self.states[from].eps.push(to);
    }

    /// Returns a fragment `(start, end)` for `re`.
    fn build(&mut self, re: &Re) -> Result<(usize, usize)> {
        self.budget.check_time()?;
        let s = self.add()?;
        let e = self.add()?;
        match re {
            Re::Nothing => {}
            Re::Eps => self.eps(s, e),
            Re::Set(set) => {
                let mask = self.alphabet.mask(set);
                if !mask.is_empty() {
                    self.states[s].edges.push((mask, e));
                }
            }
            Re::Concat(xs) => {
                let mut cur = s;
                for x in xs {
                    let (xs_, xe) = self.build(x)?;
                    self.eps(cur, xs_);
                    cur = xe;
                }
                self.eps(cur, e);
            }
            Re::Alt(xs) => {
                for x in xs {
                    let (xs_, xe) = self.build(x)?;
                    self.eps(s, xs_);
                    self.eps(xe, e);
                }
            }
            Re::Repeat(r, min, max) => {
                let mut cur = s;
                for _ in 0..*min {
                    let (rs, re_) = self.build(r)?;
                    self.eps(cur, rs);
                    cur = re_;
                }
                match max {
                    None => {
                        // Kleene star on the tail.
                        let (rs, re_) = self.build(r)?;
                        self.eps(cur, rs);
                        self.eps(re_, rs);
                        self.eps(re_, e);
                        self.eps(cur, e);
                    }
                    Some(max) => {
                        // (max - min) optional copies, each may exit early.
                        self.eps(cur, e);
                        for _ in *min..*max {
                            let (rs, re_) = self.build(r)?;
                            self.eps(cur, rs);
                            self.eps(re_, e);
                            cur = re_;
                        }
                    }
                }
            }
            Re::And(xs) => {
                let mut dfas = xs.iter().map(|x| self.sub_dfa(x));
                let first = dfas.next().transpose()?;
                let mut acc = match first {
                    Some(d) => d,
                    None => self.sub_dfa(&Re::Not(Box::new(Re::Nothing)))?,
                };
                for d in dfas {
                    acc = acc.product(&d?, |a, b| a && b, self.budget)?;
                }
                self.embed(&acc, s, e)?;
            }
            Re::Not(x) => {
                let d = self.sub_dfa(x)?.complement();
                self.embed(&d, s, e)?;
            }
        }
        Ok((s, e))
    }

    fn sub_dfa(&self, re: &Re) -> Result<Dfa> {
        let nfa = compile_with(re, self.alphabet, self.budget)?;
        determinize(&nfa, self.alphabet, self.budget)
    }

    /// Copies a DFA into the NFA between `s` and `e`.
    fn embed(&mut self, dfa: &Dfa, s: usize, e: usize) -> Result<()> {
        let dfa = dfa.trim();
        let base = self.states.len();
        for _ in 0..dfa.len() {
            self.add()?;
        }
        let k = dfa.k();
        for q in 0..dfa.len() {
            for c in 0..k {
                let t = dfa.delta[q * k + c];
                let mut m = BitSet::new(k);
                m.insert(c);
                self.states[base + q].edges.push((m, base + t));
            }
            if dfa.accepting[q] {
                self.eps(base + q, e);
            }
        }
        self.eps(s, base + dfa.start);
        Ok(())
    }
}

fn compile_with(re: &Re, alphabet: &Arc<Alphabet>, budget: &Budget) -> Result<Nfa> {
    let mut b = Builder {
        alphabet,
        budget,
        states: Vec::new(),
    };
    let (start, accept) = b.build(re)?;
    Ok(Nfa {
        states: b.states,
        start,
        accept,
    })
}

/// Thompson construction for `re` over `alphabet`.
pub fn compile(re: &Re, alphabet: &Arc<Alphabet>, budget: &Budget) -> Result<Nfa> {
    compile_with(re, alphabet, budget)
}

impl Nfa {
    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    fn closure(&self, set: &mut Vec<usize>) {
        let mut seen = BitSet::new(self.states.len());
        let mut stack: Vec<usize> = set.clone();
        for &q in set.iter() {
            seen.insert(q);
        }
        while let Some(q) = stack.pop() {
            for &t in &self.states[q].eps {
                if !seen.contains(t) {
                    seen.insert(t);
                    set.push(t);
                    stack.push(t);
                }
            }
        }
        set.sort_unstable();
        set.dedup();
    }
}

/// Subset construction. The empty subset becomes the dead state.
pub fn determinize(nfa: &Nfa, alphabet: &Arc<Alphabet>, budget: &Budget) -> Result<Dfa> {
    let k = alphabet.len();
    let mut start = vec![nfa.start];
    nfa.closure(&mut start);
    let mut id: HashMap<Vec<usize>, usize> = HashMap::new();
    let mut subsets: Vec<Vec<usize>> = Vec::new();
    let mut queue = VecDeque::new();
    id.insert(start.clone(), 0);
    subsets.push(start);
    queue.push_back(0usize);
    let mut delta: Vec<usize> = Vec::new();
    while let Some(d) = queue.pop_front() {
        budget.check_time()?;
        let mut targets: Vec<Vec<usize>> = vec![Vec::new(); k];
        for &q in &subsets[d] {
            for (mask, t) in &nfa.states[q].edges {
                for c in mask.iter() {
                    targets[c].push(*t);
                }
            }
        }
        if delta.len() < (d + 1) * k {
            delta.resize((d + 1) * k, usize::MAX);
        }
        for (c, mut t) in targets.into_iter().enumerate() {
            nfa.closure(&mut t);
            let next_id = subsets.len();
            let tid = *id.entry(t.clone()).or_insert_with(|| {
                subsets.push(t);
                queue.push_back(next_id);
                next_id
            });
            delta[d * k + c] = tid;
        }
        budget.check(
            "DFA states (subset construction)",
            subsets.len(),
            budget.max_dfa_states,
        )?;
    }
    delta.resize(subsets.len() * k, usize::MAX);
    let accepting = subsets
        .iter()
        .map(|s| s.binary_search(&nfa.accept).is_ok())
        .collect();
    Ok(Dfa {
        alphabet: alphabet.clone(),
        start: 0,
        accepting,
        delta,
    })
}

/// Thompson + subset construction in one call.
pub fn to_dfa(re: &Re, alphabet: &Arc<Alphabet>, budget: &Budget) -> Result<Dfa> {
    let nfa = compile(re, alphabet, budget)?;
    determinize(&nfa, alphabet, budget)
}
