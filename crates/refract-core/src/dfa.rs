//! Complete deterministic automata over a minterm [`Alphabet`].

use crate::alphabet::Alphabet;
use crate::budget::Budget;
use crate::error::{Error, Result};
use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::sync::Arc;

/// A complete DFA: every state has exactly one successor per minterm. State 0 need not be the
/// start state; a dead (non-accepting, self-looping) state is an ordinary state.
#[derive(Clone, Debug)]
pub struct Dfa {
    pub alphabet: Arc<Alphabet>,
    pub start: usize,
    pub accepting: Vec<bool>,
    /// Row-major transition table: `delta[state * k + class]`.
    pub delta: Vec<usize>,
}

/// A canonical, engine-independent form: states numbered in breadth-first discovery order from
/// the start state, visiting minterms in index order. Two *minimal* complete DFAs over the same
/// alphabet accept the same language exactly when their canonical forms are equal.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Canonical {
    pub accepting: Vec<bool>,
    pub delta: Vec<usize>,
}

impl Dfa {
    pub fn k(&self) -> usize {
        self.alphabet.len()
    }

    pub fn len(&self) -> usize {
        self.accepting.len()
    }

    pub fn is_empty(&self) -> bool {
        self.accepting.is_empty()
    }

    pub fn next(&self, state: usize, class: usize) -> usize {
        self.delta[state * self.k() + class]
    }

    /// Runs the DFA on a string.
    pub fn accepts(&self, s: &str) -> bool {
        let mut q = self.start;
        for c in s.chars() {
            q = self.next(q, self.alphabet.class_of(c));
        }
        self.accepting[q]
    }

    /// The same DFA restricted to states reachable from the start, renumbered canonically.
    pub fn canonical(&self) -> Canonical {
        let k = self.k();
        let mut id: HashMap<usize, usize> = HashMap::new();
        let mut order = vec![self.start];
        id.insert(self.start, 0);
        let mut i = 0;
        while i < order.len() {
            let q = order[i];
            for c in 0..k {
                let t = self.next(q, c);
                if let std::collections::hash_map::Entry::Vacant(e) = id.entry(t) {
                    e.insert(order.len());
                    order.push(t);
                }
            }
            i += 1;
        }
        let accepting = order.iter().map(|&q| self.accepting[q]).collect();
        let delta = order
            .iter()
            .flat_map(|&q| (0..k).map(move |c| (q, c)))
            .map(|(q, c)| id[&self.next(q, c)])
            .collect();
        Canonical { accepting, delta }
    }

    /// A copy containing only reachable states, numbered canonically (start = 0).
    pub fn trim(&self) -> Dfa {
        let c = self.canonical();
        Dfa {
            alphabet: self.alphabet.clone(),
            start: 0,
            accepting: c.accepting,
            delta: c.delta,
        }
    }

    /// The complement language (over the same alphabet). Cheap because the DFA is complete.
    pub fn complement(&self) -> Dfa {
        Dfa {
            accepting: self.accepting.iter().map(|a| !a).collect(),
            ..self.clone()
        }
    }

    /// Reachable product with acceptance `combine(a, b)`. Both DFAs must share an alphabet.
    pub fn product(
        &self,
        other: &Dfa,
        combine: impl Fn(bool, bool) -> bool,
        budget: &Budget,
    ) -> Result<Dfa> {
        if self.alphabet != other.alphabet {
            return Err(Error::unsupported(
                crate::Span::default(),
                "internal error: product of DFAs over different alphabets",
            ));
        }
        let k = self.k();
        let mut id: HashMap<(usize, usize), usize> = HashMap::new();
        let mut queue = VecDeque::new();
        let mut accepting = Vec::new();
        let mut delta = Vec::new();
        let start = (self.start, other.start);
        id.insert(start, 0);
        queue.push_back(start);
        accepting.push(combine(self.accepting[start.0], other.accepting[start.1]));
        while let Some((p, q)) = queue.pop_front() {
            let row = id[&(p, q)];
            debug_assert_eq!(delta.len(), row * k);
            for c in 0..k {
                let t = (self.next(p, c), other.next(q, c));
                let next_id = id.len();
                let tid = *id.entry(t).or_insert_with(|| {
                    queue.push_back(t);
                    accepting.push(combine(self.accepting[t.0], other.accepting[t.1]));
                    next_id
                });
                delta.push(tid);
            }
            budget.check("product DFA states", id.len(), budget.max_dfa_states)?;
        }
        Ok(Dfa {
            alphabet: self.alphabet.clone(),
            start: 0,
            accepting,
            delta,
        })
    }

    /// True when no string is accepted.
    pub fn is_empty_language(&self) -> bool {
        !self.canonical().accepting.iter().any(|&a| a)
    }

    /// Graphviz rendering with minterms printed as character sets.
    pub fn to_dot(&self) -> String {
        let t = self.trim();
        let mut out = String::from("digraph dfa {\n  rankdir=LR;\n  node [shape=circle];\n");
        out.push_str("  __start [shape=point];\n  __start -> 0;\n");
        for (q, &acc) in t.accepting.iter().enumerate() {
            if acc {
                let _ = writeln!(out, "  {q} [shape=doublecircle];");
            }
        }
        let k = t.k();
        for q in 0..t.len() {
            // Merge classes with the same target into one edge label.
            let mut by_target: Vec<(usize, crate::CharSet)> = Vec::new();
            for c in 0..k {
                let dst = t.delta[q * k + c];
                let set = t.alphabet.class(c);
                match by_target.iter_mut().find(|(d, _)| *d == dst) {
                    Some((_, s)) => *s = s.union(set),
                    None => by_target.push((dst, set.clone())),
                }
            }
            for (dst, set) in by_target {
                let label = set.to_string().replace('\\', "\\\\").replace('"', "\\\"");
                let _ = writeln!(out, "  {q} -> {dst} [label=\"{label}\"];");
            }
        }
        out.push_str("}\n");
        out
    }
}
