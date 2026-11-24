//! Primary engine: Brzozowski derivatives over minterm classes (doc §5.3).
//!
//! Terms are hash-consed in a [`Store`]; smart constructors keep them in a normal form
//! (alternation and intersection are flattened, sorted and deduplicated; concatenation is
//! right-associated; units and zeros are simplified). Brzozowski's theorem says a term has
//! finitely many derivatives modulo associativity, commutativity and idempotence of `|`, which
//! this normal form implements, so the lazily explored DFA is finite. Intersection and
//! complement come for free, which the Thompson engine cannot do directly.

use crate::alphabet::{Alphabet, BitSet};
use crate::budget::Budget;
use crate::dfa::Dfa;
use crate::error::Result;
use crate::regular::Re;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Index of an interned term.
pub type Id = u32;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum T {
    Nothing,
    Eps,
    Set(BitSet),
    Concat(Id, Id),
    Alt(Vec<Id>),
    And(Vec<Id>),
    Not(Id),
    Loop(Id, u32, Option<u32>),
}

/// An interning store for derivative terms over one alphabet.
pub struct Store {
    alphabet: Arc<Alphabet>,
    terms: Vec<T>,
    ids: HashMap<T, Id>,
    nullable: Vec<bool>,
    cache: HashMap<(Id, u32), Id>,
    max_terms: usize,
}

const NOTHING: Id = 0;
const EPS: Id = 1;

impl Store {
    pub fn new(alphabet: Arc<Alphabet>, budget: &Budget) -> Self {
        let mut s = Store {
            alphabet,
            terms: Vec::new(),
            ids: HashMap::new(),
            nullable: Vec::new(),
            cache: HashMap::new(),
            max_terms: budget.max_terms,
        };
        let n = s.intern(T::Nothing);
        let e = s.intern(T::Eps);
        debug_assert_eq!((n, e), (NOTHING, EPS));
        s
    }

    pub fn len(&self) -> usize {
        self.terms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    fn intern(&mut self, t: T) -> Id {
        if let Some(&id) = self.ids.get(&t) {
            return id;
        }
        let nullable = match &t {
            T::Nothing | T::Set(_) => false,
            T::Eps => true,
            T::Concat(a, b) => self.nullable[*a as usize] && self.nullable[*b as usize],
            T::Alt(xs) => xs.iter().any(|&x| self.nullable[x as usize]),
            T::And(xs) => xs.iter().all(|&x| self.nullable[x as usize]),
            T::Not(x) => !self.nullable[*x as usize],
            T::Loop(r, min, _) => *min == 0 || self.nullable[*r as usize],
        };
        let id = self.terms.len() as Id;
        self.terms.push(t.clone());
        self.nullable.push(nullable);
        self.ids.insert(t, id);
        id
    }

    fn check(&self, budget: &Budget) -> Result<()> {
        budget.check("derivative terms", self.terms.len(), self.max_terms)?;
        budget.check_time()
    }

    pub fn nullable(&self, id: Id) -> bool {
        self.nullable[id as usize]
    }

    /// Σ*, i.e. ¬∅.
    fn top(&mut self) -> Id {
        self.intern(T::Not(NOTHING))
    }

    // ------------------------------------------------------------------ smart constructors

    fn set(&mut self, mask: BitSet) -> Id {
        if mask.is_empty() {
            NOTHING
        } else {
            self.intern(T::Set(mask))
        }
    }

    fn concat(&mut self, a: Id, b: Id) -> Id {
        if a == NOTHING || b == NOTHING {
            return NOTHING;
        }
        if a == EPS {
            return b;
        }
        if b == EPS {
            return a;
        }
        if let T::Concat(x, y) = self.terms[a as usize] {
            let tail = self.concat(y, b);
            return self.concat(x, tail);
        }
        self.intern(T::Concat(a, b))
    }

    fn alt(&mut self, xs: Vec<Id>) -> Id {
        let top = self.top();
        let mut flat: Vec<Id> = Vec::with_capacity(xs.len());
        let mut set: Option<BitSet> = None;
        for x in xs {
            match &self.terms[x as usize] {
                T::Nothing => {}
                T::Alt(ys) => flat.extend(ys.iter().copied()),
                _ => flat.push(x),
            }
        }
        // Merge character sets into one (keeps derivatives small) and absorb Σ*.
        let mut out = Vec::with_capacity(flat.len());
        for x in flat {
            if x == top {
                return top;
            }
            match &self.terms[x as usize] {
                T::Set(m) => match &mut set {
                    Some(s) => s.union_with(m),
                    None => set = Some(m.clone()),
                },
                _ => out.push(x),
            }
        }
        if let Some(m) = set {
            out.push(self.set(m));
        }
        out.sort_unstable();
        out.dedup();
        match out.len() {
            0 => NOTHING,
            1 => out[0],
            _ => self.intern(T::Alt(out)),
        }
    }

    fn and(&mut self, xs: Vec<Id>) -> Id {
        let top = self.top();
        let mut flat: Vec<Id> = Vec::with_capacity(xs.len());
        for x in xs {
            match &self.terms[x as usize] {
                T::Nothing => return NOTHING,
                T::And(ys) => flat.extend(ys.iter().copied()),
                _ if x == top => {}
                _ => flat.push(x),
            }
        }
        flat.sort_unstable();
        flat.dedup();
        match flat.len() {
            0 => top,
            1 => flat[0],
            _ => self.intern(T::And(flat)),
        }
    }

    fn not(&mut self, x: Id) -> Id {
        if let T::Not(y) = self.terms[x as usize] {
            return y;
        }
        self.intern(T::Not(x))
    }

    fn repeat(&mut self, r: Id, min: u32, max: Option<u32>) -> Id {
        if max == Some(0) {
            return EPS;
        }
        if r == NOTHING {
            return if min == 0 { EPS } else { NOTHING };
        }
        if r == EPS {
            return EPS;
        }
        if min == 1 && max == Some(1) {
            return r;
        }
        // A nullable body makes the lower bound irrelevant: r{n,m} = r{0,m}.
        let min = if self.nullable(r) { 0 } else { min };
        // (r*)* = r*
        if min == 0
            && max.is_none()
            && let T::Loop(_, 0, None) = self.terms[r as usize]
        {
            return r;
        }
        self.intern(T::Loop(r, min, max))
    }

    /// Interns a regular term.
    pub fn from_re(&mut self, re: &Re, budget: &Budget) -> Result<Id> {
        self.check(budget)?;
        Ok(match re {
            Re::Nothing => NOTHING,
            Re::Eps => EPS,
            Re::Set(s) => {
                let m = self.alphabet.mask(s);
                self.set(m)
            }
            Re::Concat(xs) => {
                let mut acc = EPS;
                for x in xs.iter().rev() {
                    let id = self.from_re(x, budget)?;
                    acc = self.concat(id, acc);
                }
                acc
            }
            Re::Alt(xs) => {
                let ids = xs
                    .iter()
                    .map(|x| self.from_re(x, budget))
                    .collect::<Result<Vec<_>>>()?;
                self.alt(ids)
            }
            Re::And(xs) => {
                let ids = xs
                    .iter()
                    .map(|x| self.from_re(x, budget))
                    .collect::<Result<Vec<_>>>()?;
                self.and(ids)
            }
            Re::Not(x) => {
                let id = self.from_re(x, budget)?;
                self.not(id)
            }
            Re::Repeat(r, min, max) => {
                let id = self.from_re(r, budget)?;
                self.repeat(id, *min, *max)
            }
        })
    }

    /// The derivative of term `id` with respect to minterm `c`.
    pub fn derive(&mut self, id: Id, c: u32, budget: &Budget) -> Result<Id> {
        if let Some(&d) = self.cache.get(&(id, c)) {
            return Ok(d);
        }
        self.check(budget)?;
        let d = match self.terms[id as usize].clone() {
            T::Nothing | T::Eps => NOTHING,
            T::Set(m) => {
                if m.contains(c as usize) {
                    EPS
                } else {
                    NOTHING
                }
            }
            T::Concat(a, b) => {
                let da = self.derive(a, c, budget)?;
                let left = self.concat(da, b);
                if self.nullable(a) {
                    let db = self.derive(b, c, budget)?;
                    self.alt(vec![left, db])
                } else {
                    left
                }
            }
            T::Alt(xs) => {
                let ds = xs
                    .iter()
                    .map(|&x| self.derive(x, c, budget))
                    .collect::<Result<Vec<_>>>()?;
                self.alt(ds)
            }
            T::And(xs) => {
                let ds = xs
                    .iter()
                    .map(|&x| self.derive(x, c, budget))
                    .collect::<Result<Vec<_>>>()?;
                self.and(ds)
            }
            T::Not(x) => {
                let dx = self.derive(x, c, budget)?;
                self.not(dx)
            }
            // With the normalisation in `repeat` (min = 0 whenever r is nullable):
            // d(r{n,m}) = d(r) · r{n-1, m-1}.
            T::Loop(r, min, max) => {
                let dr = self.derive(r, c, budget)?;
                let rest = self.repeat(r, min.saturating_sub(1), max.map(|m| m - 1));
                self.concat(dr, rest)
            }
        };
        self.cache.insert((id, c), d);
        Ok(d)
    }

    /// Debug rendering of a term.
    pub fn show(&self, id: Id) -> String {
        match &self.terms[id as usize] {
            T::Nothing => "∅".into(),
            T::Eps => "ε".into(),
            T::Set(m) => {
                let set = m.iter().fold(crate::CharSet::empty(), |a, i| {
                    a.union(self.alphabet.class(i))
                });
                set.to_string()
            }
            T::Concat(a, b) => format!("{}{}", self.show(*a), self.show(*b)),
            T::Alt(xs) => format!(
                "({})",
                xs.iter()
                    .map(|&x| self.show(x))
                    .collect::<Vec<_>>()
                    .join("|")
            ),
            T::And(xs) => format!(
                "({})",
                xs.iter()
                    .map(|&x| self.show(x))
                    .collect::<Vec<_>>()
                    .join("&")
            ),
            T::Not(x) => format!("~({})", self.show(*x)),
            T::Loop(r, min, max) => format!(
                "({}){{{min},{}}}",
                self.show(*r),
                max.map_or(String::new(), |m| m.to_string())
            ),
        }
    }
}

/// Builds the derivative DFA for `re`: states are distinct derivatives reachable from `re`,
/// accepting when nullable.
pub fn to_dfa(re: &Re, alphabet: &Arc<Alphabet>, budget: &Budget) -> Result<Dfa> {
    let mut store = Store::new(alphabet.clone(), budget);
    let root = store.from_re(re, budget)?;
    explore(&mut store, root, budget)
}

/// Explores the derivatives of `root` breadth-first.
pub fn explore(store: &mut Store, root: Id, budget: &Budget) -> Result<Dfa> {
    let k = store.alphabet.len();
    let mut state_of: HashMap<Id, usize> = HashMap::new();
    let mut terms = vec![root];
    state_of.insert(root, 0);
    let mut queue = VecDeque::from([0usize]);
    let mut delta = Vec::new();
    while let Some(q) = queue.pop_front() {
        debug_assert_eq!(delta.len(), q * k);
        for c in 0..k {
            let d = store.derive(terms[q], c as u32, budget)?;
            let next = terms.len();
            let t = *state_of.entry(d).or_insert_with(|| {
                terms.push(d);
                queue.push_back(next);
                next
            });
            delta.push(t);
        }
        budget.check(
            "DFA states (derivatives)",
            terms.len(),
            budget.max_dfa_states,
        )?;
    }
    let accepting = terms.iter().map(|&t| store.nullable(t)).collect();
    Ok(Dfa {
        alphabet: store.alphabet.clone(),
        start: 0,
        accepting,
        delta,
    })
}
