# ADR-0013: Verified translation by printing and proving, and DFA-coverage test generation

**Status:** accepted (P6).

**Context.** Syntax converters (regex101's code generator, online "regex converters", LLMs)
rewrite a pattern's text for another engine and trust the rewrite. The rewrite is wrong exactly
where flavours differ: `\w`, `\d`, `\b`, `$` before a final newline, case folding, Unicode
versions. The plan (§5.8) requires "verified" only after equivalence is proven, and every claim
replayed on the real engines (§5.10).

**Decision.**

1. *Printer* (`refract_core::print`): IR → target text. Character sets are already resolved
   to the source engine's meaning, so the printer prints sets, not escapes: it uses a target
   shorthand (`\w`, `\d`, `\s`, `\p{L}`, …) only when the *target's own front-end* resolves it
   to a subset of the set, chosen greedily while it shortens the text; the rest is ranges.
   No global flags are printed (JS gets `u`). Assertions are mapped to the target's form with
   the same meaning (Python `$` → `(?=\n?\z)`, ASCII `\b` in Python → `(?a:\b)`, a `\b` over a
   word set the target lacks → look-around over the explicit set). A construct with no form is
   `Unsupported` with its span and a message naming it. The printer is never trusted.
2. *Validation* (`refract_core::translate`): parse the printed text with the target front-end
   (a failure is reported as a printer bug, never hidden) and compare it with the source under
   **both** full match and search. "Verified" = proven equivalent under both. Non-regular
   patterns get the bounded verdict (ADR-0011) and are "not verified".
3. *Full-match scope*: when no form preserves search, `refract translate` retries for full match
   only, rewriting end assertions in tail position to the text end (`^\w+$` in Python → Go
   `\A…\z`) and proving that; the report names the scope.
4. *Back-references across JS*: an unset group matches the empty string in JS and fails
   elsewhere. A back-reference is translated across that boundary only when a static check shows
   its group has always matched when it is reached; Python refuses references from inside the
   group itself.
5. *Engine replay*: each check is replayed on both real engines with the source's coverage
   strings (below; sampled strings when not regular) and the witness when they differ. Status:
   confirmed / predicted (engine unavailable) / contradicted.
6. *Test generation* (`refract_core::testgen`): the cases cover every state and every distinct
   transition (by target state) of the minimal DFA: shortest readable access string, the most
   readable character of the minterm, then the shortest readable completion to acceptance.
   Labels come from the DFA and are confirmed on the real engine before emission (pytest, Jest,
   JUnit 5, Go `testing`, JSON).

**Consequences.**
- Translations can be long where engines' Unicode tables differ (Java 25 vs CPython 3.15
  residue ranges); that length is the honest difference, not noise.
- V8's mid-surrogate-pair search positions (docs/findings.md) have no form elsewhere, so a JS
  pattern whose search can match there is "not verified" for search, with the witness.
- Tests: `print` unit tests, `tests/translation.rs` (50 000 random patterns × 5 sources × 5
  targets: every printed text parses, no check is ever *different* except the V8 quirk),
  `tests/translation_engines.rs` (verified Python↔JS translations replayed on CPython and V8).
- LexLint (plan P6) is deferred: it is outside the mandatory scope and first to drop (plan §3).
