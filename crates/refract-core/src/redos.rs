//! Static ReDoS prediction (plan §5.7, ADR-0012).
//!
//! A backtracking matcher explores every way its NFA can read a haystack before it reports a
//! failure, so its worst case follows the *ambiguity* of that NFA (Weber & Seidl 1991; applied
//! to regexes by Rexploiter, Wüstholz et al. 2017, and Weideman et al. 2016):
//!
//! - **EDA** (exponential degree of ambiguity): some state `p` has two different paths
//!   `p →w→ p` on one word `w`. Pumping `w` doubles the paths each time: exponential.
//! - **IDA** (infinite degree, polynomial): states `p ≠ q` with `p →w→ p`, `p →w→ q`,
//!   `q →w→ q`. A chain of `k` such pairs gives `n^k` paths; the matcher's time is `n^(k+1)`.
//!
//! The NFA is built from the IR the way backtracking engines run it: one consuming state per
//! character position (Glushkov-style, after removing ε-moves), with *multiplicities*: two
//! distinct ε-paths between the same positions are two paths for the matcher (`(a*)*`). An ε-path
//! that iterates a loop without consuming is skipped, as engines do. Unanchored search adds the
//! engine's retry loop (a `Σ` state before the pattern). Look-arounds, back-references and
//! anchors are ε (an over-approximation); states inside atomic groups and possessive quantifiers
//! never pivot an ambiguity, because the engine does not backtrack into them.
//!
//! Every prediction comes with an attack `prefix · pump₁ⁿ · … · suffix` whose suffix makes the
//! whole haystack fail (checked on the automaton), and is only called *confirmed* once a real
//! engine's measured times show the same growth (refract-cli `redos`).

use crate::alphabet::{Alphabet, BitSet};
use crate::budget::Budget;
use crate::charset::CharSet;
use crate::error::{Error, Result, Span};
use crate::ir::{Assertion, Node, NodeKind, Regex, Repeat};
use crate::regular::Op;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Predicted worst-case growth of a backtracking match in the haystack length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Growth {
    Linear,
    /// `O(n^d)`, `d ≥ 2`.
    Polynomial(u32),
    Exponential,
}

impl Growth {
    pub fn name(self) -> String {
        match self {
            Growth::Linear => "linear".into(),
            Growth::Polynomial(d) => format!("polynomial O(n^{d})"),
            Growth::Exponential => "exponential".into(),
        }
    }
}

/// `prefix · leads[0] · pumps[0]ⁿ · leads[1] · pumps[1]ⁿ · … · suffix` (`leads` may be empty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attack {
    pub prefix: String,
    pub pumps: Vec<String>,
    pub suffix: String,
    /// Text before each pump (other tools' attacks have them; Refract's are empty).
    pub leads: Vec<String>,
}

impl Attack {
    pub fn build(&self, n: usize) -> String {
        let mut s = self.prefix.clone();
        for (i, p) in self.pumps.iter().enumerate() {
            if let Some(l) = self.leads.get(i) {
                s.push_str(l);
            }
            s.push_str(&p.repeat(n));
        }
        s.push_str(&self.suffix);
        s
    }

    /// Characters per pump step.
    pub fn step_len(&self) -> usize {
        self.pumps.iter().map(|p| p.chars().count()).sum()
    }
}

#[derive(Clone, Debug)]
pub struct Prediction {
    pub growth: Growth,
    pub attack: Option<Attack>,
    /// The quantifier responsible, when known.
    pub span: Option<Span>,
    pub reason: String,
    /// Positions in the analysed NFA.
    pub states: usize,
}

// ------------------------------------------------------------------ backtracking NFA

enum S {
    Eps(Vec<usize>),
    /// A greedy or lazy loop: try `body`, or leave to `exit`.
    Loop {
        body: usize,
        exit: usize,
    },
    Char {
        set: CharSet,
        next: usize,
        atomic: bool,
        span: Span,
    },
    Accept,
}

struct Builder {
    s: Vec<S>,
    atomic: u32,
    loops: Vec<Span>,
    limit: usize,
    max_repeat: u32,
    /// JDK ≥ 9: the outermost greedy unbounded quantified group remembers start positions that
    /// failed (`Pattern.Loop.posIndex`), unless the pattern has a back-reference.
    java_memo: bool,
    /// Quantified groups we are inside (the JDK only memoises the outermost).
    group_closures: u32,
}

impl Builder {
    fn push(&mut self, st: S) -> Result<usize> {
        if self.s.len() >= self.limit {
            return Err(Error::budget(format!(
                "ReDoS NFA exceeded {} states (counted repetitions are expanded)",
                self.limit
            )));
        }
        self.s.push(st);
        Ok(self.s.len() - 1)
    }

    fn char(&mut self, set: CharSet, next: usize, span: Span) -> Result<usize> {
        let span = self.loops.last().copied().unwrap_or(span);
        let atomic = self.atomic > 0;
        self.push(S::Char {
            set,
            next,
            atomic,
            span,
        })
    }

    fn build(&mut self, n: &Node, next: usize) -> Result<usize> {
        match &n.kind {
            NodeKind::Empty | NodeKind::Assert(_) | NodeKind::Look(_) | NodeKind::Backref(..) => {
                Ok(next)
            }
            NodeKind::Literal(c) => self.char(CharSet::single(*c), next, n.span),
            NodeKind::Class(set) => self.char(set.clone(), next, n.span),
            NodeKind::Concat(xs) => {
                let mut k = next;
                for x in xs.iter().rev() {
                    k = self.build(x, k)?;
                }
                Ok(k)
            }
            NodeKind::Alt(xs) => {
                let mut v = Vec::with_capacity(xs.len());
                for x in xs {
                    v.push(self.build(x, next)?);
                }
                self.push(S::Eps(v))
            }
            NodeKind::Group(g) => {
                self.atomic += u32::from(g.atomic);
                let r = self.build(&g.sub, next);
                self.atomic -= u32::from(g.atomic);
                r
            }
            NodeKind::Conditional(c) => {
                let yes = self.build(&c.yes, next)?;
                let no = match &c.no {
                    Some(x) => self.build(x, next)?,
                    None => next,
                };
                self.push(S::Eps(vec![yes, no]))
            }
            NodeKind::Repeat(r) => {
                let on_group = matches!(r.sub.kind, NodeKind::Group(_));
                if self.java_memo
                    && on_group
                    && self.group_closures == 0
                    && r.max.is_none()
                    && r.greedy
                    && !r.possessive
                {
                    // Iterating again from a failed position is cut: only the loop's own
                    // re-splitting is pruned, the body's ambiguity remains. Analyse as x{m}
                    // (x? when m = 0).
                    let memo = Repeat {
                        sub: r.sub.clone(),
                        min: r.min,
                        max: Some(r.min.max(1)),
                        greedy: true,
                        possessive: false,
                    };
                    self.group_closures += 1;
                    let out = self.repeat(&memo, next);
                    self.group_closures -= 1;
                    return out;
                }
                self.group_closures += u32::from(on_group);
                self.atomic += u32::from(r.possessive);
                let unbounded = r.max.is_none();
                if unbounded {
                    self.loops.push(n.span);
                }
                let out = self.repeat(r, next);
                if unbounded {
                    self.loops.pop();
                }
                self.atomic -= u32::from(r.possessive);
                self.group_closures -= u32::from(on_group);
                out
            }
        }
    }

    fn repeat(&mut self, r: &Repeat, next: usize) -> Result<usize> {
        let optional = r.max.map(|m| m.saturating_sub(r.min));
        if r.min > self.max_repeat || optional.is_some_and(|o| o > self.max_repeat) {
            return Err(Error::budget(format!(
                "counted repetition above {} in the ReDoS analysis",
                self.max_repeat
            )));
        }
        let mut k = match optional {
            None => {
                let l = self.push(S::Eps(Vec::new()))?;
                let body = self.build(&r.sub, l)?;
                self.s[l] = S::Loop { body, exit: next };
                l
            }
            Some(opt) => {
                let mut k = next;
                for _ in 0..opt {
                    let b = self.build(&r.sub, k)?;
                    k = self.push(S::Eps(vec![b, next]))?;
                }
                k
            }
        };
        for _ in 0..r.min {
            k = self.build(&r.sub, k)?;
        }
        Ok(k)
    }
}

/// The ε-free NFA: node 0 is the start; every other node consumes one character of `set`.
struct Nfa {
    set: Vec<CharSet>,
    atomic: Vec<bool>,
    span: Vec<Option<Span>>,
    /// `(target, multiplicity ∈ {1, 2})`.
    succ: Vec<Vec<(usize, u8)>>,
    /// Can the match end right after this state (ε-path to Accept)?
    fin: Vec<bool>,
    /// Minterms of all sets, and each state's set as a minterm mask.
    alpha: Arc<Alphabet>,
    mask: Vec<BitSet>,
}

impl Nfa {
    fn finish(&mut self) {
        let alpha = Arc::new(Alphabet::new(&self.set));
        self.mask = self.set.iter().map(|s| alpha.mask(s)).collect();
        self.alpha = alpha;
    }

    /// Do the sets of all `states` share a character (as a word-level AND)?
    fn meet(&self, states: &[usize]) -> bool {
        let first = self.mask[states[0]].words();
        (0..first.len()).any(|w| {
            states
                .iter()
                .fold(u64::MAX, |acc, &s| acc & self.mask[s].words()[w])
                != 0
        })
    }

    /// The most readable character in all of `states`' sets.
    fn common(&self, states: &[usize]) -> Option<char> {
        self.alpha.readable_order().iter().copied().find_map(|i| {
            let c = self.alpha.representative_u32(i);
            (c <= crate::charset::MAX_CHAR
                && !(0xD800..=0xDFFF).contains(&c)
                && states.iter().all(|&s| self.mask[s].contains(i)))
            .then(|| char::from_u32(c))
            .flatten()
        })
    }
}

const CLOSURE_STEPS: u64 = 2_000_000;

fn eliminate(b: &Builder, start: usize) -> Result<Nfa> {
    let mut id: HashMap<usize, usize> = HashMap::new();
    let mut set = vec![CharSet::empty()];
    let mut atomic = vec![false];
    let mut span = vec![None];
    for (i, st) in b.s.iter().enumerate() {
        if let S::Char {
            set: cs,
            atomic: a,
            span: sp,
            ..
        } = st
        {
            id.insert(i, set.len());
            set.push(cs.clone());
            atomic.push(*a);
            span.push(Some(*sp));
        }
    }
    let mut steps = 0u64;
    let mut memo: HashMap<usize, Vec<(usize, u8)>> = HashMap::new();
    let mut closure = |from: usize, steps: &mut u64| -> Result<Vec<(usize, u8)>> {
        if let Some(v) = memo.get(&from) {
            return Ok(v.clone());
        }
        let mut counts: HashMap<usize, u8> = HashMap::new();
        let mut order = Vec::new();
        let mut iterating = vec![false; b.s.len()];
        // Explicit DFS: (node, next child index).
        fn walk(
            b: &Builder,
            x: usize,
            iterating: &mut [bool],
            counts: &mut HashMap<usize, u8>,
            order: &mut Vec<usize>,
            steps: &mut u64,
        ) -> Result<()> {
            *steps += 1;
            if *steps > CLOSURE_STEPS {
                return Err(Error::budget("ReDoS ε-closure exceeded its step limit"));
            }
            match &b.s[x] {
                S::Char { .. } => {
                    let c = counts.entry(x).or_insert_with(|| {
                        order.push(x);
                        0
                    });
                    *c = (*c + 1).min(2);
                }
                S::Accept => {}
                S::Eps(v) => {
                    for &y in v {
                        walk(b, y, iterating, counts, order, steps)?;
                    }
                }
                S::Loop { body, exit } => {
                    // Iterating again without consuming is an empty iteration: engines stop.
                    if !iterating[x] {
                        iterating[x] = true;
                        walk(b, *body, iterating, counts, order, steps)?;
                        iterating[x] = false;
                    }
                    walk(b, *exit, iterating, counts, order, steps)?;
                }
            }
            Ok(())
        }
        walk(b, from, &mut iterating, &mut counts, &mut order, steps)?;
        let v: Vec<(usize, u8)> = order.iter().map(|x| (id[x], counts[x])).collect();
        memo.insert(from, v.clone());
        Ok(v)
    };
    let reaches_accept = |from: usize| -> bool {
        let mut seen = vec![false; b.s.len()];
        let mut st = vec![from];
        while let Some(x) = st.pop() {
            if std::mem::replace(&mut seen[x], true) {
                continue;
            }
            match &b.s[x] {
                S::Accept => return true,
                S::Eps(v) => st.extend(v.iter().copied()),
                S::Loop { body, exit } => st.extend([*body, *exit]),
                S::Char { .. } => {}
            }
        }
        false
    };
    let mut fin = vec![false; set.len()];
    fin[0] = reaches_accept(start);
    for (i, st) in b.s.iter().enumerate() {
        if let S::Char { next, .. } = st {
            fin[id[&i]] = reaches_accept(*next);
        }
    }
    let mut succ = vec![Vec::new(); set.len()];
    succ[0] = closure(start, &mut steps)?;
    for (i, st) in b.s.iter().enumerate() {
        if let S::Char { next, .. } = st {
            succ[id[&i]] = closure(*next, &mut steps)?;
        }
    }
    Ok(Nfa {
        set,
        atomic,
        span,
        succ,
        fin,
        alpha: Arc::new(Alphabet::new(&[])),
        mask: Vec::new(),
    })
}

fn has_backref(n: &Node) -> bool {
    match &n.kind {
        NodeKind::Backref(..) => true,
        NodeKind::Concat(xs) | NodeKind::Alt(xs) => xs.iter().any(has_backref),
        NodeKind::Repeat(r) => has_backref(&r.sub),
        NodeKind::Group(g) => has_backref(&g.sub),
        NodeKind::Look(l) => has_backref(&l.sub),
        NodeKind::Conditional(c) => {
            has_backref(&c.yes) || c.no.as_ref().is_some_and(|x| has_backref(x))
        }
        _ => false,
    }
}

fn anchored_at_start(n: &Node) -> bool {
    match &n.kind {
        NodeKind::Assert(Assertion::TextStart | Assertion::LineStart { .. }) => true,
        NodeKind::Assert(Assertion::JavaLineStart { .. }) => true,
        NodeKind::Concat(xs) => xs.first().is_some_and(anchored_at_start),
        NodeKind::Group(g) => anchored_at_start(&g.sub),
        NodeKind::Alt(xs) => !xs.is_empty() && xs.iter().all(anchored_at_start),
        _ => false,
    }
}

// ------------------------------------------------------------------ graph helpers

/// Iterative Tarjan: component id per node.
fn sccs(n: usize, succ: &dyn Fn(usize) -> Vec<usize>) -> Vec<usize> {
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0; n];
    let mut on = vec![false; n];
    let mut comp = vec![usize::MAX; n];
    let mut stack = Vec::new();
    let mut next_index = 0;
    let mut ncomp = 0;
    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        let mut call: Vec<(usize, Vec<usize>, usize)> = vec![(root, succ(root), 0)];
        index[root] = next_index;
        low[root] = next_index;
        next_index += 1;
        stack.push(root);
        on[root] = true;
        while let Some((v, kids, i)) = call.last_mut() {
            let v = *v;
            if *i < kids.len() {
                let w = kids[*i];
                *i += 1;
                if index[w] == usize::MAX {
                    index[w] = next_index;
                    low[w] = next_index;
                    next_index += 1;
                    stack.push(w);
                    on[w] = true;
                    call.push((w, succ(w), 0));
                } else if on[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                call.pop();
                if let Some((u, _, _)) = call.last() {
                    low[*u] = low[*u].min(low[v]);
                }
                if low[v] == index[v] {
                    while let Some(w) = stack.pop() {
                        on[w] = false;
                        comp[w] = ncomp;
                        if w == v {
                            break;
                        }
                    }
                    ncomp += 1;
                }
            }
        }
    }
    comp
}

/// Shortest readable word from `from` to `to` (`None` if unreachable). `allowed` restricts
/// the visited nodes.
fn word_between(
    nfa: &Nfa,
    from: usize,
    to: usize,
    allowed: &dyn Fn(usize) -> bool,
) -> Option<String> {
    let mut prev: HashMap<usize, (usize, char)> = HashMap::new();
    let mut q = VecDeque::from([from]);
    let mut seen = vec![false; nfa.set.len()];
    seen[from] = true;
    while let Some(x) = q.pop_front() {
        if x == to && (x != from || !prev.is_empty()) {
            break;
        }
        for &(y, _) in &nfa.succ[x] {
            if !allowed(y) {
                continue;
            }
            let Some(c) = nfa.common(&[y]) else {
                continue;
            };
            if y == to && to == from && !prev.contains_key(&y) {
                prev.insert(y, (x, c));
                q.clear();
                break;
            }
            if !seen[y] {
                seen[y] = true;
                prev.insert(y, (x, c));
                q.push_back(y);
            }
        }
    }
    if from == to && !prev.contains_key(&to) {
        return None;
    }
    if from != to && !seen[to] {
        return None;
    }
    let mut out = Vec::new();
    let mut cur = to;
    loop {
        let (p, c) = prev[&cur];
        out.push(c);
        cur = p;
        if cur == from {
            break;
        }
    }
    out.reverse();
    Some(out.into_iter().collect())
}

// ------------------------------------------------------------------ analysis

const MAX_PAIRS: usize = 1_000_000;
const MAX_TRIPLES: usize = 3_000_000;

/// Predicts the worst-case backtracking growth of `re` under `op`.
pub fn analyse(re: &Regex, op: Op, budget: &Budget) -> Result<Prediction> {
    let memo = re.flavor == crate::Flavor::Java && !has_backref(&re.root);
    let base = analyse_with(re, op, budget, memo)?;
    if !memo {
        return Ok(base);
    }
    // JDK: a memoised loop retries each start position at most once per match attempt, so it
    // costs n × (the body's cost from one start). Measured on JDK 25 (ADR-0012).
    let mut loops = Vec::new();
    memo_loops(&re.root, false, &mut loops);
    if loops.is_empty() {
        return Ok(base);
    }
    // The attack for the unpruned loops; the memo's cost depends on what it pumps.
    let raw = analyse_with(re, op, budget, false)?;
    let Some(attack) = raw.attack.clone() else {
        return Ok(base);
    };
    let mut worst: Option<(Growth, Span)> = None;
    for (body, span) in loops {
        let g = match body_cost(body, &attack.pumps, budget)? {
            None => continue,
            Some(BodyCost::Exponential) => Growth::Exponential,
            Some(BodyCost::Degree(d)) => Growth::Polynomial(d + 1),
        };
        if g != Growth::Polynomial(1) && worst.is_none_or(|(w, _)| rank(g) > rank(w)) {
            worst = Some((g, span));
        }
    }
    let Some((g, span)) = worst else {
        return Ok(base);
    };
    if rank(g) <= rank(base.growth) {
        return Ok(base);
    }
    Ok(Prediction {
        growth: g,
        attack: raw.attack,
        span: Some(span),
        reason: "the JDK remembers failed start positions of this loop, so it costs n × (its body's cost from one start)".into(),
        states: raw.states,
    })
}

fn rank(g: Growth) -> u32 {
    match g {
        Growth::Linear => 1,
        Growth::Polynomial(d) => d,
        Growth::Exponential => u32::MAX,
    }
}

/// The JDK's memoised loops: outermost greedy unbounded quantified groups.
fn memo_loops<'a>(n: &'a Node, inside: bool, out: &mut Vec<(&'a Node, Span)>) {
    match &n.kind {
        NodeKind::Repeat(r) => {
            let on_group = matches!(r.sub.kind, NodeKind::Group(_));
            if on_group && !inside && r.max.is_none() && r.greedy && !r.possessive {
                out.push((&r.sub, n.span));
            }
            memo_loops(&r.sub, inside || on_group, out);
        }
        NodeKind::Concat(xs) | NodeKind::Alt(xs) => {
            for x in xs {
                memo_loops(x, inside, out);
            }
        }
        NodeKind::Group(g) => memo_loops(&g.sub, inside, out),
        NodeKind::Conditional(c) => {
            memo_loops(&c.yes, inside, out);
            if let Some(x) = &c.no {
                memo_loops(x, inside, out);
            }
        }
        _ => {}
    }
}

enum BodyCost {
    Exponential,
    /// Work from one start is O(n^d) (d = 0: bounded).
    Degree(u32),
}

/// How much work a backtracking matcher does running `body` from one start inside the pumped
/// text (characters `pumped`), over all ends. `None` when the body cannot complete there: then
/// iterations cannot start at arbitrary positions of the pumped part and the memo costs O(1).
fn body_cost(body: &Node, pumps: &[String], budget: &Budget) -> Result<Option<BodyCost>> {
    let pumped = CharSet::from_iter(pumps.iter().flat_map(|p| p.chars()));
    let pumped = &pumped;
    let mut b = Builder {
        s: vec![S::Accept],
        atomic: 0,
        loops: Vec::new(),
        limit: budget.max_nfa_states.min(20_000),
        max_repeat: budget.max_repeat,
        java_memo: false,
        group_closures: 1, // inner loops are not memoised
    };
    let start = b.build(body, 0)?;
    let mut nfa = eliminate(&b, start)?;
    // Only what the pumped text can feed the body.
    for set in nfa.set.iter_mut().skip(1) {
        *set = set.intersect(pumped);
    }
    let alive: Vec<bool> = nfa
        .set
        .iter()
        .enumerate()
        .map(|(i, s)| i == 0 || !s.is_empty())
        .collect();
    for succ in nfa.succ.iter_mut() {
        succ.retain(|e| alive[e.0]);
    }
    nfa.finish();
    let n = nfa.set.len();
    // Can the body consume a non-empty part of the pumped text and finish?
    let mut seen = vec![false; n];
    let mut st: Vec<usize> = nfa.succ[0].iter().map(|e| e.0).collect();
    let mut completes = false;
    while let Some(x) = st.pop() {
        if std::mem::replace(&mut seen[x], true) {
            continue;
        }
        completes |= nfa.fin[x];
        st.extend(nfa.succ[x].iter().map(|e| e.0));
    }
    if !completes {
        return Ok(None);
    }
    let comp = sccs(n, &|x| nfa.succ[x].iter().map(|e| e.0).collect());
    let cyclic: Vec<bool> = (0..n)
        .map(|x| {
            nfa.succ[x].iter().any(|&(y, _)| y == x) || (0..n).any(|y| y != x && comp[y] == comp[x])
        })
        .collect();
    if eda(&nfa, &comp, budget)?.is_some() {
        return Ok(Some(BodyCost::Exponential));
    }
    if let Some((chain, _)) = ida(&nfa, &comp, &cyclic, budget)?.into_iter().next() {
        return Ok(Some(BodyCost::Degree(chain.len() as u32)));
    }
    // A loop of the body costs O(n) from one start only if it can run over the pumped text:
    // some state returns to itself reading a rotation of a pump word.
    let cycles_on = |x: usize, w: &[char]| -> bool {
        let mut cur = vec![x];
        for &c in w {
            let class = nfa.alpha.class_of(c);
            let mut next: Vec<usize> = cur
                .iter()
                .flat_map(|&y| nfa.succ[y].iter().map(|e| e.0))
                .filter(|&z| nfa.mask[z].contains(class))
                .collect();
            next.sort_unstable();
            next.dedup();
            cur = next;
        }
        cur.contains(&x)
    };
    let looping = (1..n).any(|x| {
        cyclic[x]
            && !nfa.atomic[x]
            && pumps.iter().any(|p| {
                let w: Vec<char> = p.chars().collect();
                (0..w.len()).any(|r| {
                    let rot: Vec<char> = w[r..].iter().chain(&w[..r]).copied().collect();
                    cycles_on(x, &rot)
                })
            })
    });
    Ok(Some(BodyCost::Degree(u32::from(looping))))
}

fn analyse_with(re: &Regex, op: Op, budget: &Budget, java_memo: bool) -> Result<Prediction> {
    let mut b = Builder {
        s: vec![S::Accept],
        atomic: 0,
        loops: Vec::new(),
        limit: budget.max_nfa_states.min(20_000),
        max_repeat: budget.max_repeat,
        java_memo,
        group_closures: 0,
    };
    let start = b.build(&re.root, 0)?;
    let mut nfa = eliminate(&b, start)?;
    // The search loop: retrying at each position is a `Σ*` the pattern can overlap with.
    let searching = op == Op::Search && !re.quirks.sticky && !anchored_at_start(&re.root);
    if searching {
        let p = nfa.set.len();
        nfa.set.push(CharSet::full());
        nfa.atomic.push(false);
        nfa.span.push(None);
        let mut s = vec![(p, 1)];
        s.extend(nfa.succ[0].iter().copied());
        nfa.succ.push(s);
        nfa.fin.push(nfa.fin[0]);
        nfa.succ[0].push((p, 1));
    }
    nfa.finish();
    let states = nfa.set.len();
    let comp = sccs(states, &|x| nfa.succ[x].iter().map(|e| e.0).collect());
    let cyclic: Vec<bool> = (0..states)
        .map(|x| {
            nfa.succ[x].iter().any(|&(y, _)| y == x)
                || (0..states).any(|y| y != x && comp[y] == comp[x])
        })
        .collect();
    budget.check_time()?;

    // Candidates, most severe first: the EDA witness, then IDA chains from longest to single
    // links. The first whose attack can be made to fail wins (a long chain may only pump
    // characters that always end in a match: `(.*a){12}`).
    let mut cands: Vec<Candidate> = Vec::new();
    if let Some((p, pumps, why)) = eda(&nfa, &comp, budget)? {
        cands.push((Growth::Exponential, p, pumps, why));
    }
    for (chain, pumps) in ida(&nfa, &comp, &cyclic, budget)? {
        let why = format!(
            "{} overlapping loops in sequence: each split of the pumped part is tried",
            chain.len()
        );
        cands.push((Growth::Polynomial(chain.len() as u32), chain[0], pumps, why));
    }
    let Some(top) = cands.first().map(|c| c.0) else {
        return Ok(Prediction {
            growth: Growth::Linear,
            attack: None,
            span: None,
            reason: "no ambiguous loop: each character is read in at most one way per position"
                .into(),
            states,
        });
    };
    // Cheap pre-filter: a candidate whose pumped text alone already contains a whole match can
    // hardly fail; try those last (one search automaton, built once).
    if searching
        && cands.len() > 1
        && let Ok(any) = crate::automata::regex_dfa(re, Op::Search, budget)
    {
        let hopeless = |c: &Candidate| -> bool {
            let mut text = String::new();
            for p in &c.2 {
                text.push_str(&p.repeat(13));
            }
            any.accepts(&text)
        };
        let mut scored: Vec<(bool, Candidate)> =
            cands.into_iter().map(|c| (hopeless(&c), c)).collect();
        scored.sort_by_key(|(h, _)| *h); // stable: severity order kept within each group
        cands = scored.into_iter().map(|(_, c)| c).collect();
    }
    let cache: DfaCache = std::cell::RefCell::new(HashMap::new());
    let searching_since = std::time::Instant::now();
    let mut tried: std::collections::HashSet<(String, Vec<String>)> = Default::default();
    for (growth, pivot, mut pumps, why) in cands.into_iter().take(MAX_CANDIDATES) {
        budget.check_time()?;
        if searching_since.elapsed() > ATTACK_SEARCH {
            break;
        }
        // Identical pumps in sequence are one pump (pumping it k·n times reaches the same paths).
        if pumps.windows(2).all(|w| w[0] == w[1]) {
            pumps.truncate(1);
        }
        let span = nfa.span[pivot];
        let prefix = if pivot == 0 || (searching && pivot == states - 1) {
            Some(String::new())
        } else {
            word_between(&nfa, 0, pivot, &|_| true)
        };
        let Some(prefix) = prefix else {
            continue;
        };
        if !tried.insert((prefix.clone(), pumps.clone())) {
            continue; // same attack as a more severe candidate
        }
        // In search mode the retry loop reads any character, so a one-character lead can make an
        // early alternative fail (`^\s+|\s+$` needs "a" + "  …" + "a").
        let mut found = failing_suffix(re, op, searching, &prefix, &pumps, budget, &cache)
            .map(|s| (prefix.clone(), s));
        if found.is_none() && searching {
            for c in ['a', '!', '0', 'A', ' ', '-', '_', '.'] {
                let lead = format!("{c}{prefix}");
                if let Some(s) = failing_suffix(re, op, searching, &lead, &pumps, budget, &cache) {
                    found = Some((lead, s));
                    break;
                }
            }
        }
        if let Some((prefix, suffix)) = found {
            return Ok(Prediction {
                growth,
                attack: Some(Attack {
                    prefix,
                    pumps,
                    suffix,
                    leads: Vec::new(),
                }),
                span,
                reason: why,
                states,
            });
        }
    }
    Ok(Prediction {
        growth: Growth::Linear,
        attack: None,
        span: None,
        reason: if searching_since.elapsed() > ATTACK_SEARCH {
            format!(
                "ambiguous ({}), but no failing attack was found within {} ms (not proven safe)",
                top.name(),
                ATTACK_SEARCH.as_millis()
            )
        } else {
            format!(
                "ambiguous ({}), but no suffix makes the pumped haystack fail, so the first try succeeds",
                top.name()
            )
        },
        states,
    })
}

const MAX_CANDIDATES: usize = 400;

/// (growth, pivot state, pump words, reason).
type Candidate = (Growth, usize, Vec<String>, String);
type Triple = (usize, usize, usize);
/// Wall-clock cap on looking for a failing attack among the candidates.
const ATTACK_SEARCH: std::time::Duration = std::time::Duration::from_millis(1500);

/// "A match starts within the first k positions" automata, by k (they are costly to build).
type DfaCache = std::cell::RefCell<HashMap<u32, Option<crate::Dfa>>>;

type Found = Option<(usize, Vec<String>, String)>;

fn eda(nfa: &Nfa, comp: &[usize], budget: &Budget) -> Result<Found> {
    // (a) Two ε-paths between the same positions inside a loop.
    let mut first: Option<((usize, usize), usize, String)> = None;
    for x in 0..nfa.set.len() {
        for &(y, m) in &nfa.succ[x] {
            if m < 2 || nfa.atomic[y] || comp[x] != comp[y] || x == 0 {
                continue;
            }
            let Some(c) = nfa.common(&[y]) else {
                continue;
            };
            let back = if x == y {
                Some(String::new())
            } else {
                word_between(nfa, y, x, &|z| comp[z] == comp[x])
            };
            if let Some(back) = back {
                let pump = format!("{c}{back}");
                let prefix = word_between(nfa, 0, x, &|_| true).map_or(usize::MAX, |w| w.len());
                let key = (pump.chars().count(), prefix);
                if first.as_ref().is_none_or(|(k, _, _)| key < *k) {
                    first = Some((key, x, pump));
                }
            }
        }
    }
    // (b) A cycle in the self-product through a diagonal and an off-diagonal pair.
    let n = nfa.set.len();
    let mut ids: HashMap<(usize, usize), usize> = HashMap::new();
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let mut edges: Vec<Vec<(usize, char)>> = Vec::new();
    let mut q = VecDeque::new();
    let intern = |p: (usize, usize),
                  ids: &mut HashMap<(usize, usize), usize>,
                  pairs: &mut Vec<(usize, usize)>,
                  edges: &mut Vec<Vec<(usize, char)>>,
                  q: &mut VecDeque<usize>| {
        *ids.entry(p).or_insert_with(|| {
            pairs.push(p);
            edges.push(Vec::new());
            q.push_back(pairs.len() - 1);
            pairs.len() - 1
        })
    };
    intern((0, 0), &mut ids, &mut pairs, &mut edges, &mut q);
    while let Some(i) = q.pop_front() {
        let (x, y) = pairs[i];
        budget.check("ReDoS product pairs", pairs.len(), MAX_PAIRS)?;
        for &(x2, _) in &nfa.succ[x] {
            for &(y2, _) in &nfa.succ[y] {
                // Unordered pairs suffice: (p,q) and (q,p) are symmetric.
                let key = (x2.min(y2), x2.max(y2));
                if !nfa.meet(&[x2, y2]) {
                    continue;
                }
                let Some(c) = nfa.common(&[x2, y2]) else {
                    continue;
                };
                let j = intern(key, &mut ids, &mut pairs, &mut edges, &mut q);
                edges[i].push((j, c));
            }
        }
    }
    let _ = n;
    let pc = sccs(pairs.len(), &|i| edges[i].iter().map(|e| e.0).collect());
    let mut has_off: HashMap<usize, usize> = HashMap::new();
    for (i, &(x, y)) in pairs.iter().enumerate() {
        if x != y {
            has_off.entry(pc[i]).or_insert(i);
        }
    }
    let mut best: Option<((usize, usize), usize, String)> = None;
    let nested = first.clone();
    for (i, &(x, y)) in pairs.iter().enumerate() {
        if x != y || x == 0 || nfa.atomic[x] {
            continue;
        }
        let Some(&off) = has_off.get(&pc[i]) else {
            continue;
        };
        // Word (x,x) → off → (x,x), inside the component.
        let path = |from: usize, to: usize| -> Option<String> {
            let mut prev: HashMap<usize, (usize, char)> = HashMap::new();
            let mut dq = VecDeque::from([from]);
            let mut seen: HashMap<usize, ()> = HashMap::from([(from, ())]);
            while let Some(u) = dq.pop_front() {
                if u == to {
                    break;
                }
                for &(v, c) in &edges[u] {
                    if pc[v] == pc[from] && !seen.contains_key(&v) {
                        seen.insert(v, ());
                        prev.insert(v, (u, c));
                        dq.push_back(v);
                    }
                }
            }
            if !seen.contains_key(&to) {
                return None;
            }
            let mut out = Vec::new();
            let mut cur = to;
            while cur != from {
                let (p, c) = prev[&cur];
                out.push(c);
                cur = p;
            }
            out.reverse();
            Some(out.into_iter().collect())
        };
        if let (Some(a), Some(b)) = (path(i, off), path(off, i)) {
            let pump = format!("{a}{b}");
            let prefix = word_between(nfa, 0, x, &|_| true).map_or(usize::MAX, |w| w.len());
            let key = (pump.chars().count(), prefix);
            if best.as_ref().is_none_or(|(k, _, _)| key < *k) {
                best = Some((key, x, pump));
            }
        }
    }
    if let Some((k, x, pump)) = nested.clone()
        && best.as_ref().is_none_or(|(b, _, _)| k <= *b)
    {
        return Ok(Some((
            x,
            vec![pump],
            "a loop can be entered in two ways on the same input (nested quantifiers)".into(),
        )));
    }
    if let Some((_, x, pump)) = best {
        return Ok(Some((
            x,
            vec![pump],
            "two different paths read the same pumped input and meet again (overlapping alternatives in a loop)"
                .into(),
        )));
    }
    Ok(None)
}

/// Does a word `w` exist with `p →w→ p`, `p →w→ q` and `q →w→ q`? Returns it.
fn ida_word(nfa: &Nfa, p: usize, q: usize, visited: &mut usize) -> Result<Vec<String>> {
    // Fast path: one character c with p →c→ p, p →c→ q, q →c→ q (self-loops; the usual case).
    // One word per shared minterm (up to 3): the most readable one may be the character that
    // always leads to a match.
    let self_p = nfa.succ[p].iter().any(|e| e.0 == p);
    let self_q = nfa.succ[q].iter().any(|e| e.0 == q);
    if self_p && self_q && nfa.meet(&[p, q]) {
        let mut words = Vec::new();
        for &class in nfa.alpha.readable_order() {
            if words.len() == 3 {
                break;
            }
            if !(nfa.mask[p].contains(class) && nfa.mask[q].contains(class)) {
                continue;
            }
            let c = nfa.alpha.representative_u32(class);
            let Some(c) = char::from_u32(c).filter(|_| c <= crate::charset::MAX_CHAR) else {
                continue;
            };
            // p reaches q reading only c: then w = c^k works (k = that path's length).
            let mut dist = vec![usize::MAX; nfa.set.len()];
            dist[p] = 0;
            let mut dq = VecDeque::from([p]);
            while let Some(x) = dq.pop_front() {
                for &(y, _) in &nfa.succ[x] {
                    if dist[y] == usize::MAX && nfa.mask[y].contains(class) {
                        dist[y] = dist[x] + 1;
                        dq.push_back(y);
                    }
                }
            }
            if dist[q] != usize::MAX {
                words.push(c.to_string().repeat(dist[q].max(1)));
            }
        }
        if !words.is_empty() {
            return Ok(words);
        }
    }
    let start = (p, p, q);
    let goal = (p, q, q);
    let mut prev: HashMap<Triple, (Triple, char)> = HashMap::new();
    let mut dq = VecDeque::from([start]);
    let mut found = false;
    while let Some(t @ (x, y, z)) = dq.pop_front() {
        for &(x2, _) in &nfa.succ[x] {
            for &(y2, _) in &nfa.succ[y] {
                if !nfa.meet(&[x2, y2]) {
                    continue;
                }
                for &(z2, _) in &nfa.succ[z] {
                    let u = (x2, y2, z2);
                    if u == start || prev.contains_key(&u) || !nfa.meet(&[x2, y2, z2]) {
                        continue;
                    }
                    let Some(c) = nfa.common(&[x2, y2, z2]) else {
                        continue;
                    };
                    prev.insert(u, (t, c));
                    *visited += 1;
                    if *visited > MAX_TRIPLES {
                        return Err(Error::budget("ReDoS triple product exceeded its limit"));
                    }
                    if u == goal {
                        found = true;
                        dq.clear();
                        break;
                    }
                    dq.push_back(u);
                }
                if found {
                    break;
                }
            }
            if found {
                break;
            }
        }
    }
    if !found {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut cur = goal;
    while cur != start {
        let (p, c) = prev[&cur];
        out.push(c);
        cur = p;
    }
    out.reverse();
    Ok(vec![out.into_iter().collect()])
}

/// The longest chain of IDA pairs and its pump words.
fn ida(
    nfa: &Nfa,
    comp: &[usize],
    cyclic: &[bool],
    budget: &Budget,
) -> Result<Vec<(Vec<usize>, Vec<String>)>> {
    let n = nfa.set.len();
    let cand: Vec<usize> = (1..n).filter(|&x| cyclic[x] && !nfa.atomic[x]).collect();
    if cand.len() < 2 {
        return Ok(Vec::new());
    }
    // Reachability between candidate states.
    let reach = |from: usize| -> Vec<bool> {
        let mut seen = vec![false; n];
        let mut st = vec![from];
        seen[from] = true;
        while let Some(x) = st.pop() {
            for &(y, _) in &nfa.succ[x] {
                if !seen[y] {
                    seen[y] = true;
                    st.push(y);
                }
            }
        }
        seen
    };
    let mut visited = 0usize;
    let mut link: HashMap<usize, Vec<(usize, String)>> = HashMap::new();
    for &p in &cand {
        let r = reach(p);
        for &q in &cand {
            if q == p || comp[q] == comp[p] || !r[q] {
                continue;
            }
            budget.check_time()?;
            for w in ida_word(nfa, p, q, &mut visited)? {
                link.entry(p).or_default().push((q, w));
            }
        }
    }
    if link.is_empty() {
        return Ok(Vec::new());
    }
    // Longest chain (the relation is acyclic: it follows the SCC order).
    fn longest(
        p: usize,
        link: &HashMap<usize, Vec<(usize, String)>>,
        memo: &mut HashMap<usize, (usize, Option<usize>)>,
    ) -> usize {
        if let Some(&(l, _)) = memo.get(&p) {
            return l;
        }
        let mut best = (0, None);
        for (k, (q, _)) in link.get(&p).into_iter().flatten().enumerate() {
            let l = 1 + longest(*q, link, memo);
            if l > best.0 {
                best = (l, Some(k));
            }
        }
        memo.insert(p, best);
        best.0
    }
    let mut memo = HashMap::new();
    let mut starts: Vec<usize> = link.keys().copied().collect();
    for &p in &starts {
        longest(p, &link, &mut memo);
    }
    starts.sort_by_key(|&p| (std::cmp::Reverse(memo[&p].0), p));
    let mut out: Vec<(Vec<usize>, Vec<String>)> = Vec::new();
    for &p in &starts {
        let mut chain = vec![p];
        let mut pumps = Vec::new();
        let mut cur = p;
        while let Some(&(_, Some(k))) = memo.get(&cur) {
            let (q, w) = &link[&cur][k];
            pumps.push(w.clone());
            chain.push(*q);
            cur = *q;
        }
        out.push((chain, pumps));
    }
    // Single links too: a shorter chain may have an attack where the longest has none.
    let mut singles: Vec<(Vec<usize>, Vec<String>)> = link
        .iter()
        .flat_map(|(&p, qs)| qs.iter().map(move |(q, w)| (vec![p, *q], vec![w.clone()])))
        .collect();
    singles.sort();
    out.extend(singles);
    out.dedup();
    // Growth n^(links + 1): a chain holds links + 1 states.
    Ok(out)
}

/// The shortest readable suffix that makes `prefix · pumpsⁿ · suffix` fail for every tested `n`.
///
/// For unanchored search the engine pays for every failed start position before a later one
/// matches, so an attack only needs every start up to the end of the pumped part to fail
/// (checked with the match-at-start language); otherwise the whole haystack must be rejected.
fn failing_suffix(
    re: &Regex,
    op: Op,
    searching: bool,
    prefix: &str,
    pumps: &[String],
    budget: &Budget,
    cache: &DfaCache,
) -> Option<String> {
    let attack = |n: usize, suffix: &str| {
        Attack {
            prefix: prefix.to_string(),
            pumps: pumps.to_vec(),
            suffix: suffix.to_string(),
            leads: Vec::new(),
        }
        .build(n)
    };
    let dfa = if searching {
        None
    } else {
        crate::automata::regex_dfa(re, op, budget).ok()
    };
    let pumped_len = |n: usize| {
        prefix.chars().count() + pumps.iter().map(|p| p.chars().count()).sum::<usize>() * n
    };
    // Search: one "a match starts within the pumped part" automaton per n.
    let starts: HashMap<usize, Option<crate::Dfa>> = if searching {
        [5usize, 8, 13]
            .iter()
            .map(|&n| {
                let k = pumped_len(n) as u32;
                let d = cache
                    .borrow_mut()
                    .entry(k)
                    .or_insert_with(|| {
                        crate::regular::lower_starts_within(re, k, budget)
                            .and_then(|l| crate::automata::shared_dfas(&[(&l, re.domain)], budget))
                            .ok()
                            .map(|(_, mut v)| v.remove(0))
                    })
                    .clone();
                (n, d)
            })
            .collect()
    } else {
        HashMap::new()
    };
    let have_dfa = dfa.is_some() || starts.values().all(Option::is_some) && searching;
    let rejects = |s: &str, n: usize| -> Option<bool> {
        if searching && let Some(Some(d)) = starts.get(&n) {
            return Some(!d.accepts(s));
        }
        match &dfa {
            Some(d) => Some(!d.accepts(s)),
            None => crate::interp::is_match(re, op, s, 200_000).ok().map(|m| !m),
        }
    };
    let ns: &[usize] = if have_dfa && searching {
        &[5, 8, 13]
    } else if have_dfa {
        &[5, 8, 13, 21]
    } else {
        &[2, 3]
    };
    let mut candidates: Vec<String> = vec![String::new()];
    let any_dfa = dfa.as_ref().or_else(|| starts.values().flatten().next());
    let singles: Vec<char> = match any_dfa {
        Some(d) => d
            .alphabet
            .readable_order()
            .iter()
            .map(|&i| d.alphabet.representative(i))
            .filter(|&c| c != '\u{FFFD}')
            .collect(),
        None => ['!', ' ', '\n', '0', 'a', 'A', '_', '-'].to_vec(),
    };
    for &c in &singles {
        candidates.push(c.to_string());
    }
    for &c in &singles {
        for &d in singles.iter().take(16) {
            candidates.push(format!("{c}{d}"));
        }
    }
    candidates
        .into_iter()
        .find(|s| ns.iter().all(|&n| rejects(&attack(n, s), n) == Some(true)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Flavor, parse};

    fn growth(f: Flavor, p: &str, op: Op) -> (Growth, Option<String>) {
        let re = parse(f, p).unwrap();
        let pr = analyse(&re, op, &Budget::default()).unwrap();
        let attack = pr
            .attack
            .as_ref()
            .map(|a| format!("{}|{}|{}", a.prefix, a.pumps.join(","), a.suffix));
        (pr.growth, attack)
    }

    #[test]
    fn classic_exponential() {
        for p in [
            r"^(a+)+$",
            r"^(a|a)*$",
            r"^(a*)*$",
            r"^(a|aa)+$",
            r"^(\w+\s?)+$",
            r"^(a|a?)+$",
            r"(x+x+)+y",
        ] {
            assert_eq!(
                growth(Flavor::Python, p, Op::FullMatch).0,
                Growth::Exponential,
                "{p}"
            );
        }
        let (_, a) = growth(Flavor::Python, r"^(a+)+$", Op::FullMatch);
        let a = a.unwrap();
        let pump = a.split('|').nth(1).unwrap();
        assert!(!pump.is_empty() && pump.chars().all(|c| c == 'a'), "{a}");
    }

    #[test]
    fn polynomial_degrees() {
        assert_eq!(
            growth(Flavor::Python, r"a*a*b", Op::FullMatch).0,
            Growth::Polynomial(2)
        );
        assert_eq!(
            growth(Flavor::Python, r"a*a*a*b", Op::FullMatch).0,
            Growth::Polynomial(3)
        );
        // Search adds the retry loop.
        assert_eq!(
            growth(Flavor::Python, r"a+b", Op::Search).0,
            Growth::Polynomial(2)
        );
        assert_eq!(
            growth(Flavor::Python, r"\d+\.\d+", Op::Search).0,
            Growth::Polynomial(2)
        );
        assert_eq!(
            growth(Flavor::Python, r"^a+b", Op::Search).0,
            Growth::Linear
        );
    }

    #[test]
    fn safe_patterns() {
        for p in [
            r"^a+$",
            r"^(ab)*$",
            r"^[a-z]+@[a-z]+\.com$",
            r"^(a|b)*c$",
            r"^\d{1,3}(\.\d{1,3}){3}$",
            r"^(?:a+)$",
        ] {
            assert_eq!(
                growth(Flavor::Python, p, Op::FullMatch).0,
                Growth::Linear,
                "{p}"
            );
        }
    }

    #[test]
    fn atomic_and_possessive_are_safe() {
        for p in [r"^(?>a+)+$", r"^(a++)+$", r"^(?>(a|a))*$"] {
            assert_eq!(
                growth(Flavor::Python, p, Op::FullMatch).0,
                Growth::Linear,
                "{p}"
            );
        }
    }

    #[test]
    fn jdk_loop_memo() {
        let g = |p: &str| growth(Flavor::Java, p, Op::FullMatch).0;
        // Measured on JDK 25 (docs/findings.md): memoised loops cost n × body.
        assert_eq!(g(r"^(a+)+$"), Growth::Polynomial(2));
        assert_eq!(g(r"^(a|a)*$"), Growth::Linear);
        assert_eq!(g(r"^(a|aa)+$"), Growth::Linear);
        assert_eq!(g(r"^(\w+\s?)+$"), Growth::Polynomial(2));
        assert_eq!(g(r"^(a+b?)+$"), Growth::Polynomial(2));
        assert_eq!(g(r"^(?:a*a*)+b"), Growth::Polynomial(3));
        assert_eq!(g(r"^((a+)+)+$"), Growth::Exponential);
        assert_eq!(g(r"^(a+)+?$"), Growth::Exponential);
        assert_eq!(g(r"^(a+)+\1?$"), Growth::Exponential);
        assert_eq!(g(r"^(?:a+){2,}$"), Growth::Polynomial(2));
        // Python has no such memo.
        assert_eq!(
            growth(Flavor::Python, r"^(a+)+$", Op::FullMatch).0,
            Growth::Exponential
        );
    }

    #[test]
    fn jdk_memo_needs_iterations_inside_the_pump() {
        // Measured linear on JDK 25 (bench/results/redos.md): the body cannot run over the pump.
        let g = |p: &str| growth(Flavor::Java, p, Op::Search).0;
        assert_eq!(g(r"^(\s*,\s*)*$"), Growth::Linear);
        assert_eq!(
            g(r"^([a-zA-Z0-9_.\-])+@(([a-zA-Z0-9\-])+\.)+([a-zA-Z0-9]{2,4})+$"),
            Growth::Linear
        );
        assert_eq!(g(r"^(a+)+$"), Growth::Polynomial(2));
    }

    #[test]
    fn shorter_chains_when_the_longest_always_matches() {
        // Pumping "a" always completes (.*a){12}; pumping another character gives quadratic search.
        let (g, a) = growth(Flavor::Python, r"(.*a){12}", Op::Search);
        assert!(matches!(g, Growth::Polynomial(_)), "{g:?} {a:?}");
    }

    #[test]
    fn long_counted_chains_stay_fast() {
        let t = std::time::Instant::now();
        let g = growth(Flavor::Python, r"^(a+){1,30}$", Op::FullMatch).0;
        assert!(matches!(g, Growth::Polynomial(d) if d >= 10), "{g:?}");
        assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
    }

    #[test]
    fn no_failing_suffix_is_not_vulnerable() {
        // Search for (a+)+ succeeds on the first 'a'.
        assert_eq!(
            growth(Flavor::Python, r"(a+)+", Op::Search).0,
            Growth::Linear
        );
        // With (?s), `.*` accepts any rest: the first try succeeds.
        assert_eq!(
            growth(Flavor::Python, r"(?s)(a|a)*.*", Op::FullMatch).0,
            Growth::Linear
        );
        // Without it, a final "\n" fails every split: really exponential in CPython.
        assert_eq!(
            growth(Flavor::Python, r"^(a|a)*.*$", Op::FullMatch).0,
            Growth::Exponential
        );
    }

    #[test]
    fn late_matches_do_not_save_search() {
        // "   …a": every start in the spaces is tried (O(n^2) each) before `$` matches at the end.
        assert_eq!(
            growth(Flavor::Python, r"\s*#?\s*$", Op::Search).0,
            Growth::Polynomial(3)
        );
        assert_eq!(
            growth(Flavor::Python, r"\s+$", Op::Search).0,
            Growth::Polynomial(2)
        );
        // The JS trim shape: the attack needs a non-space first.
        let (g, a) = growth(Flavor::Python, r"^\s+|\s+$", Op::Search);
        assert_eq!(g, Growth::Polynomial(2), "{a:?}");
    }

    #[test]
    fn attacks_really_fail() {
        for p in [r"^(a+)+$", r"a*a*b", r"^(\w+\s?)+$"] {
            let re = parse(Flavor::Python, p).unwrap();
            let pr = analyse(&re, Op::FullMatch, &Budget::default()).unwrap();
            let s = pr.attack.unwrap().build(10);
            assert!(
                !crate::automata::regex_dfa(&re, Op::FullMatch, &Budget::default())
                    .unwrap()
                    .accepts(&s)
            );
        }
    }
}
