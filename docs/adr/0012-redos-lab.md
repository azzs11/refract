# ADR-0012: ReDoS lab — ambiguity prediction, engine models, measured confirmation

**Status:** accepted (P5).

**Context.** Catastrophic backtracking depends on the pattern *and* the engine: CPython `re` and
V8 backtrack naively, the JDK memoises some loops, V8 has an opt-in linear fallback, Go and Rust
never backtrack. Static ReDoS detectors (recheck, regexploit, rxxr2, Rexploiter) model one engine
and report a verdict; the plan (§5.7, review item 4) requires per-engine, version- and
option-aware verdicts that are confirmed by measurement.

**Decision.**

1. *Prediction* (`refract_core::redos`): build the backtracking NFA from the IR (one consuming
   state per position, ε-paths counted with multiplicity, empty loop iterations skipped, search
   adds the retry loop). EDA (two paths around one state on one word, via ε-multiplicity or a
   cycle in the self-product through a diagonal and an off-diagonal pair) ⇒ exponential; else the
   longest chain of IDA pairs (`p→p, p→q, q→q` on one word, triple product, single-character fast
   path) gives `O(n^(k+1))`. States in atomic groups/possessive loops never pivot. Each prediction
   carries an attack whose suffix makes the haystack fail on the automaton (or the interpreter).
2. *Engine models*:
   - Go, Rust: linear (automaton engines).
   - V8 with `--enable-experimental-regexp-engine-on-excessive-backtracks`: linear unless the
     pattern has back-references or look-around, or the `i`/`u`/`v` flag (measured on V8 14.6).
   - JDK: the outermost greedy unbounded quantified group (no back-reference in the pattern)
     remembers failed start positions (`Pattern.Loop.posIndex`, JDK ≥ 9). Analysed as
     `max(n × cost(body from one start), rest with the loop replaced by x{m})`; matches the
     measured n, n², n³ cases (docs/findings.md).
3. *Confirmation* (`refract redos`): every attack is timed inside each engine's runner (min of 3
   runs, a timeout kills the runner). Exponential predictions grow the pump count by 2, others
   double the length; the curve is classified from its log-log slope and per-pump ratio above a
   200 µs floor. "confirmed" only when the measured class equals the predicted one; otherwise the
   row says "not confirmed" (engine faster) or "CONTRADICTED" (slower, exit 3).

**Consequences.** Verdicts name engine, version and options. Engine optimisations we have not
modelled show up as "not confirmed", never as a silent false alarm. The benchmark
(`bench/redos`, results in `bench/results/redos.md`) scores Refract, recheck and regexploit
against measured ground truth per engine.
