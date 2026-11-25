# Method

How Refract decides what it reports, what each claim means, and where it can be wrong. Design
decisions are in [docs/adr/](adr/README.md); per-flavour semantics in [spec/flavours/](../spec/flavours).

## 1. From pattern text to a formal model

1. **Front-ends that mirror each engine's parser.** One hand-written recursive-descent parser per
   flavour (CPython `_parser.py`, V8's Annex B and Unicode grammars, Go `regexp/syntax`, OpenJDK
   `Pattern.java`, `regex-syntax`). Each accepts exactly what its engine accepts (checked on
   100,000 random patterns per engine) and rejects the rest with a span.
2. **One IR over character sets.** Every flavour-specific meaning is resolved during parsing:
   `\w \d \s .`, case folding, `$`, `\b`, line terminators, inline flags. Unicode tables are
   **measured from each engine** (generators in `tools/`), because engines pin different Unicode
   versions and definitions (CPython's `\w` is `str.isalnum()`, not UTS #18).
3. **Operations.** A pattern is a set of strings only once the operation is fixed: *full match*
   (`re.fullmatch`, Java `matches()`) or *search* (JS `test()`, `re.search`, Rust `is_match`).
   Lowering builds the language for the chosen operation; every verdict names it.
4. **Haystack domain.** Strings of Unicode scalar values; JavaScript without `u`/`v` matches
   UTF-16 code units, which is lifted exactly to scalar haystacks (ADR-0007).

## 2. Deciding equivalence

- Lowered terms are compiled to DFAs over **minterms** (the coarsest partition of code points that
  every set in either pattern respects), so automata stay small over all of Unicode.
- **Two independent constructions** — symbolic Brzozowski derivatives and Thompson NFA + subset
  construction — and two minimisers (Hopcroft, Brzozowski) must produce isomorphic minimal DFAs
  (1,000,000 random terms in CI).
- **Look-around** is exact: look-ahead by continuation (`(?=s)` before `k` is `(s·Σ*) ∩ k`),
  look-behind by a context carried in the state (ADR-0008); `\b`, `^`, `$` anywhere are built
  from those with each engine's own sets.
- **Equivalence** is breadth-first search of the product automaton. If a state accepting on one
  side only is reachable, the BFS path is the **shortest witness**; ties are broken towards
  readable characters (printable ASCII first), giving one canonical witness (ADR-0006).

## 3. What each claim means

A **disagreement** between two engines is either a *difference* (they match different strings;
Refract reports the shortest witness) or a *rejection* (one engine refuses the pattern, so the two
disagree on every input — the shortest is the empty string; confirmed by the engine's compile
error). Both count as disagreements in every report, exit code and summary.

| claim | meaning |
|---|---|
| **Proved** (SAME, verified) | the whole product automaton was explored: no string of any length distinguishes the patterns under that operation |
| **Confirmed** | the witness (or rejection) was replayed on the real engines and they behaved as predicted |
| **Predicted** | computed but not replayed (engine unavailable, or a static check like `refract check`) |
| **Bounded** | back-references are not regular: every string up to length *n* over a representative alphabet was checked with a reference interpreter that follows each engine's rules (ADR-0011); never called proved |
| **Contradicted** | an engine disagreed with the model — a Refract bug; exit code 3, never hidden |

## 4. Real-engine oracle

Pinned engines run behind one JSON-lines protocol ([oracle/protocol.md](../oracle/protocol.md)):
CPython 3.15.2 and Node 26 (V8 14.6) natively or in Docker, Go 1.27.4 and JDK 25.0.5.1 in Docker,
Rust `regex` 1.13 in process. Full match is implemented the way each engine's users write it
(`fullmatch`, `matches()`, a sticky look-ahead wrapper in JS, `\A(?:…)\z` in Go). Timeouts kill
and restart a runner.

## 5. ReDoS

Ambiguity of the backtracking NFA (EDA → exponential, IDA chains → polynomial degree) gives a
prediction and an attack whose suffix forces failure; per-engine models cover what each engine
actually does (Go and Rust linear; V8's opt-in linear fallback and where it does not apply; the
JDK's loop memo). A prediction is *confirmed* only when the attack's measured time curve on that
engine has the predicted class (ADR-0012).

## 6. Translation and test generation

The IR is printed in the target, re-parsed by the target's front-end and proven equivalent under
full match and search; only then is it *verified* (ADR-0013). Test strings cover every state and
transition of the minimal DFA and are labelled by the model, then confirmed on the engine.

## 7. The census

Regexes from 2026 library code in five ecosystems, each compared with what someone porting it
would write in the other engines; every difference and rejection replayed
([bench/census/README.md](../bench/census/README.md), results in
[bench/results/census.md](../bench/results/census.md)).

## 8. Threats to validity

- **Front-end fidelity.** A parser that accepts what the engine accepts can still resolve a
  construct differently; mitigated by semantic property tests against each engine (random
  patterns × haystacks) and by replaying every reported difference. Untested corners remain.
- **Corpus.** The census corpus is library code from five ecosystems, skewed to JavaScript (npm);
  it is not a random sample of all code, and test code is excluded.
- **Porting assumption.** "Same text in another engine" is one way to port; flags passed
  separately are modelled as inline flags (or JS literal flags). Real ports may adapt patterns.
- **Bounded checks** cover short strings only; a difference that needs a longer string is missed.
- **Engine versions.** Results are for the pinned versions; engines change (V8's linear engine,
  Ruby's memoisation), and the weekly drift job is planned, not yet running.
- **Timing.** ReDoS confirmation depends on the machine; classes come from growth, not absolute
  times, but noise near the timeout can misclassify.
