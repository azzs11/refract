# ADR-0008: Exact look-around by continuations and look-behind context

**Context.** Mandatory scope: one regex *with look-around* turned into automata. Zero-width
assertions (`(?=) (?!) (?<=) (?<!)`, `\b`, `\B`, `^`/`$` anywhere, multi-line anchors) are not
character transitions; ADR-0004 only resolved anchors at the pattern's edges.

**Decision.** Two constructions, both exact for the regular fragment:

1. **Look-ahead by continuation passing** (`regular::lower`). `node(n, k)` builds the language of
   "n, then whatever k describes, to the end of the haystack". A look-ahead `(?=s)` before `k` is
   `(s·Σ*) ∩ k`; `(?!s)` is `¬(s·Σ*) ∩ k`. Only sub-patterns that contain a look-ahead thread `k`.
2. **Look-behind by context** (`deriv`). `Re::Behind{sub}` is a zero-width atom that holds when the
   input read so far is in `Σ*·sub`. The derivative store keeps one DFA per distinct `sub`; an
   automaton state is a pair (term, context = states of those DFAs), finite by construction.

Every other assertion is built from these, with each engine's own character sets: `\b` =
`(?<=w)(?!w) | (?<!w)(?=w)`, `^` = `(?<![\s\S])`, Java's `$`, `(?m)^`, Mn-aware `\b`, etc.
Engine quirks that change results are modelled explicitly (Java look-behind windows from its
32-bit `TreeInfo`; V8 `u`-mode search at mid-surrogate positions, docs/findings.md).

**Consequences.** Look-around is exact and confirmed on the real engines (oracle tests).
Refused, with spans: look-ahead inside an *unbounded* repetition, any look-ahead-type assertion
inside a look-behind, look-behind inside look-behind. The Thompson engine has no look-behind;
for those terms the independent check is the direct set-based matcher (`tests/lookbehind_engine.rs`).
