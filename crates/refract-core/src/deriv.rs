//! Primary engine: Brzozowski derivatives over minterm classes (doc §5.3).
//!
//! Terms are hash-consed in a [`Store`]; smart constructors keep them in a normal form
//! (alternation and intersection are flattened, sorted and deduplicated; concatenation is
//! right-associated; units and zeros are simplified). Brzozowski's theorem says a term has
//! finitely many derivatives modulo associativity, commutativity and idempotence of `|`, which
//! this normal form implements, so the lazily explored DFA is finite. Intersection and
//! complement come for free, which the Thompson engine cannot do directly.
//!
//! Look-behind (ADR-0008): a `Behind` atom is zero-width and true when the input read so far
//! is in `Σ* · sub`. The store keeps one small DFA per distinct `sub` and a *context*: the
//! current state of each of them. Automaton states are (term, context) pairs; nullability and
//! derivatives of terms that contain look-behinds are evaluated in a context. Terms without
//! look-behinds ignore it ([`NO_CTX`]), so ordinary patterns build exactly the DFAs they did.

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
    /// Look-behind number `.0` (an index into `Store::behinds`), negated when `.1`.
    Behind(u32, bool),
    /// Recursion variable number `.0`; its definition is `Store::defs[.0]` (a `Fix`).
    Var(u32),
}

/// Context id meaning "irrelevant": used for terms without look-behinds.
pub const NO_CTX: u32 = u32::MAX;

/// An interning store for derivative terms over one alphabet.
pub struct Store {
    alphabet: Arc<Alphabet>,
    terms: Vec<T>,
    ids: HashMap<T, Id>,
    nullable: Vec<bool>,
    /// Per term: does it contain a look-behind (so its meaning depends on the context)?
    has_behind: Vec<bool>,
    cache: HashMap<(Id, u32, u32), Id>,
    max_terms: usize,
    /// One DFA for `Σ* · sub` per distinct look-behind.
    behinds: Vec<Dfa>,
    behind_ids: HashMap<Re, u32>,
    ctxs: Vec<Vec<usize>>,
    ctx_ids: HashMap<Vec<usize>, u32>,
    ctx_steps: HashMap<(u32, u32), u32>,
    null_memo: HashMap<(Id, u32), bool>,
    /// Per term: does it reach a recursion variable?
    has_var: Vec<bool>,
    /// Definitions of recursion variables.
    defs: Vec<Id>,
    /// `Re` variable → store variable while interning a `Fix` body.
    env: Vec<(u32, Id)>,
    /// Variables being unfolded right now (least-fixed-point guards).
    in_progress: Vec<u32>,
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
            has_behind: Vec::new(),
            cache: HashMap::new(),
            max_terms: budget.max_terms,
            behinds: Vec::new(),
            behind_ids: HashMap::new(),
            ctxs: Vec::new(),
            ctx_ids: HashMap::new(),
            ctx_steps: HashMap::new(),
            null_memo: HashMap::new(),
            has_var: Vec::new(),
            defs: Vec::new(),
            env: Vec::new(),
            in_progress: Vec::new(),
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
        let hb = |x: &Id| self.has_behind[*x as usize];
        let has_behind = match &t {
            T::Behind(..) => true,
            // A variable's definition may contain look-behinds: be conservative.
            T::Var(_) => true,
            T::Nothing | T::Eps | T::Set(_) => false,
            T::Concat(a, b) => hb(a) || hb(b),
            T::Alt(xs) | T::And(xs) => xs.iter().any(hb),
            T::Not(x) | T::Loop(x, _, _) => hb(x),
        };
        let hv = |x: &Id| self.has_var[*x as usize];
        let has_var = match &t {
            T::Var(_) => true,
            T::Nothing | T::Eps | T::Set(_) | T::Behind(..) => false,
            T::Concat(a, b) => hv(a) || hv(b),
            T::Alt(xs) | T::And(xs) => xs.iter().any(hv),
            T::Not(x) | T::Loop(x, _, _) => hv(x),
        };
        // Static nullability; meaningless (and never consulted) for terms with look-behinds.
        let nullable = match &t {
            T::Nothing | T::Set(_) | T::Behind(..) | T::Var(_) => false,
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
        self.has_behind.push(has_behind);
        self.has_var.push(has_var);
        self.ids.insert(t, id);
        id
    }

    fn check(&self, budget: &Budget) -> Result<()> {
        budget.check("derivative terms", self.terms.len(), self.max_terms)?;
        budget.check_time()
    }

    /// Does the term's meaning depend on the look-behind context?
    pub fn has_behind(&self, id: Id) -> bool {
        self.has_behind[id as usize]
    }

    /// The starting context (every look-behind DFA at its start). Call after interning all
    /// terms: look-behinds are registered while interning.
    pub fn initial_ctx(&mut self) -> u32 {
        let start: Vec<usize> = self.behinds.iter().map(|d| d.start).collect();
        self.ctx_id(start)
    }

    fn ctx_id(&mut self, states: Vec<usize>) -> u32 {
        if let Some(&id) = self.ctx_ids.get(&states) {
            return id;
        }
        let id = self.ctxs.len() as u32;
        self.ctxs.push(states.clone());
        self.ctx_ids.insert(states, id);
        id
    }

    /// The context after reading one character of minterm `c`.
    pub fn step_ctx(&mut self, ctx: u32, c: u32) -> u32 {
        if ctx == NO_CTX {
            return NO_CTX;
        }
        if let Some(&n) = self.ctx_steps.get(&(ctx, c)) {
            return n;
        }
        let next: Vec<usize> = self.ctxs[ctx as usize]
            .iter()
            .zip(&self.behinds)
            .map(|(&q, d)| d.next(q, c as usize))
            .collect();
        let id = self.ctx_id(next);
        self.ctx_steps.insert((ctx, c), id);
        id
    }

    /// The context to pair with `id`: [`NO_CTX`] when the term cannot consult it.
    pub fn normalize_ctx(&self, id: Id, ctx: u32) -> u32 {
        if self.has_behind(id) { ctx } else { NO_CTX }
    }

    /// Nullability of `id` at a position whose look-behind context is `ctx`.
    pub fn nullable(&mut self, id: Id, ctx: u32) -> bool {
        if !self.has_behind[id as usize] && !self.has_var[id as usize] {
            return self.nullable[id as usize];
        }
        if let Some(&v) = self.null_memo.get(&(id, ctx)) {
            return v;
        }
        let v = match self.terms[id as usize].clone() {
            // Least fixed point: a re-entrant variable contributes "not nullable".
            T::Var(n) if self.in_progress.contains(&n) => false,
            T::Var(n) => {
                self.in_progress.push(n);
                let d = self.defs[n as usize];
                let v = self.nullable(d, ctx);
                self.in_progress.pop();
                v
            }
            T::Behind(i, negated) => {
                let q = self.ctxs[ctx as usize][i as usize];
                self.behinds[i as usize].accepting[q] != negated
            }
            T::Concat(a, b) => self.nullable(a, ctx) && self.nullable(b, ctx),
            T::Alt(xs) => xs.iter().any(|&x| self.nullable(x, ctx)),
            T::And(xs) => xs.iter().all(|&x| self.nullable(x, ctx)),
            T::Not(x) => !self.nullable(x, ctx),
            T::Loop(r, min, _) => min == 0 || self.nullable(r, ctx),
            T::Nothing | T::Set(_) => false,
            T::Eps => true,
        };
        // Only cache results that do not depend on an unfinished fixed point.
        if self.in_progress.is_empty() || !self.has_var[id as usize] {
            self.null_memo.insert((id, ctx), v);
        }
        v
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
        // A nullable body makes the lower bound irrelevant: r{n,m} = r{0,m}. Not for bodies
        // with look-behinds, whose nullability depends on the position.
        let min = if !self.has_behind(r) && !self.has_var[r as usize] && self.nullable[r as usize] {
            0
        } else {
            min
        };
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
            Re::Fix { var, body } => {
                let n = self.defs.len() as u32;
                self.defs.push(NOTHING);
                let v = self.intern(T::Var(n));
                self.env.push((*var, v));
                let b = self.from_re(body, budget);
                self.env.pop();
                self.defs[n as usize] = b?;
                v
            }
            Re::Var(var) => match self.env.iter().rev().find(|(w, _)| w == var) {
                Some(&(_, v)) => v,
                None => {
                    return Err(crate::error::Error::unsupported(
                        crate::error::Span::default(),
                        "internal: unbound recursion variable",
                    ));
                }
            },
            Re::Behind { sub, negated } => {
                if sub.has_behind() {
                    return Err(crate::error::Error::unsupported(
                        crate::error::Span::default(),
                        "look-behind inside a look-behind is not analysed yet",
                    ));
                }
                let idx = match self.behind_ids.get(sub.as_ref()) {
                    Some(&i) => i,
                    None => {
                        // Σ* over this alphabet's whole universe (¬∅), which includes pseudo
                        // code units when the pattern matches UTF-16 (ADR-0007).
                        let lang =
                            Re::Concat(vec![Re::Not(Box::new(Re::Nothing)), sub.as_ref().clone()]);
                        let dfa = to_dfa(&lang, &self.alphabet, budget)?;
                        let i = self.behinds.len() as u32;
                        self.behinds.push(dfa);
                        self.behind_ids.insert(sub.as_ref().clone(), i);
                        i
                    }
                };
                self.intern(T::Behind(idx, *negated))
            }
        })
    }

    /// The derivative of term `id` with respect to minterm `c`, at a position whose
    /// look-behind context is `ctx`.
    pub fn derive(&mut self, id: Id, ctx: u32, c: u32, budget: &Budget) -> Result<Id> {
        let ctx = self.normalize_ctx(id, ctx);
        if let Some(&d) = self.cache.get(&(id, ctx, c)) {
            return Ok(d);
        }
        self.check(budget)?;
        let d = match self.terms[id as usize].clone() {
            T::Nothing | T::Eps | T::Behind(..) => NOTHING,
            // Least fixed point: re-entry is only reachable through iterations that consumed
            // nothing, which add nothing.
            T::Var(n) if self.in_progress.contains(&n) => return Ok(NOTHING),
            T::Var(n) => {
                self.in_progress.push(n);
                let def = self.defs[n as usize];
                let r = self.derive(def, ctx, c, budget);
                self.in_progress.pop();
                r?
            }
            T::Set(m) => {
                if m.contains(c as usize) {
                    EPS
                } else {
                    NOTHING
                }
            }
            T::Concat(a, b) => {
                let da = self.derive(a, ctx, c, budget)?;
                let left = self.concat(da, b);
                if self.nullable(a, ctx) {
                    let db = self.derive(b, ctx, c, budget)?;
                    self.alt(vec![left, db])
                } else {
                    left
                }
            }
            T::Alt(xs) => {
                let ds = xs
                    .iter()
                    .map(|&x| self.derive(x, ctx, c, budget))
                    .collect::<Result<Vec<_>>>()?;
                self.alt(ds)
            }
            T::And(xs) => {
                let ds = xs
                    .iter()
                    .map(|&x| self.derive(x, ctx, c, budget))
                    .collect::<Result<Vec<_>>>()?;
                self.and(ds)
            }
            T::Not(x) => {
                let dx = self.derive(x, ctx, c, budget)?;
                self.not(dx)
            }
            // With the normalisation in `repeat` (min = 0 whenever r is nullable):
            // d(r{n,m}) = d(r) · r{n-1, m-1}. For a body with look-behinds the normalisation is
            // not applied, so use the general rule: if r is nullable here and min > 0, the first
            // copy may be empty, adding d(r{n-1,m-1}).
            T::Loop(r, min, max) => {
                let dr = self.derive(r, ctx, c, budget)?;
                let rest = self.repeat(r, min.saturating_sub(1), max.map(|m| m - 1));
                let main = self.concat(dr, rest);
                if min > 0 && self.has_behind(r) && self.nullable(r, ctx) {
                    let skip = self.derive(rest, ctx, c, budget)?;
                    self.alt(vec![main, skip])
                } else {
                    main
                }
            }
        };
        if self.in_progress.is_empty() || !self.has_var[id as usize] {
            self.cache.insert((id, ctx, c), d);
        }
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
            T::Behind(i, negated) => format!("(?<{}#{i})", if *negated { "!" } else { "=" }),
            T::Var(n) => format!("x{n}"),
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

/// Explores the (term, context) states reachable from `root` breadth-first.
pub fn explore(store: &mut Store, root: Id, budget: &Budget) -> Result<Dfa> {
    let k = store.alphabet.len();
    let ctx0 = store.initial_ctx();
    let start = (root, store.normalize_ctx(root, ctx0));
    let mut state_of: HashMap<(Id, u32), usize> = HashMap::from([(start, 0)]);
    let mut states = vec![start];
    let mut queue = VecDeque::from([0usize]);
    let mut delta = Vec::new();
    while let Some(q) = queue.pop_front() {
        debug_assert_eq!(delta.len(), q * k);
        let (term, ctx) = states[q];
        for c in 0..k {
            let d = store.derive(term, ctx, c as u32, budget)?;
            let next_ctx = store.step_ctx(ctx, c as u32);
            let key = (d, store.normalize_ctx(d, next_ctx));
            let next = states.len();
            let t = *state_of.entry(key).or_insert_with(|| {
                states.push(key);
                queue.push_back(next);
                next
            });
            delta.push(t);
        }
        budget.check(
            "DFA states (derivatives)",
            states.len(),
            budget.max_dfa_states,
        )?;
    }
    let accepting = states
        .iter()
        .map(|&(t, ctx)| store.nullable(t, ctx))
        .collect();
    Ok(Dfa {
        alphabet: store.alphabet.clone(),
        start: 0,
        accepting,
        delta,
    })
}
