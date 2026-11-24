# ADR-0004: Assertions — resolve statically or refuse

**Status: superseded by [ADR-0008](0008-lookaround-construction.md)** (P3: assertions anywhere are exact).


**Context.** Anchors and word boundaries are zero-width and context-dependent; a plain DFA over
characters cannot express them directly. Full support (derivatives with look-around context, as
in RE# / .NET NonBacktracking) is planned for P4, but P1–P2 need anchored patterns, which are
very common (`^...$`).

**Decision.** Assertions stay in the IR with exact meanings (spec/ir.md). Lowering to a regular
term, which fixes the operation (full match or search), uses a three-valued analysis of each
assertion's position: certainly at the haystack edge → ε, certainly not → ∅, unknown →
`Unsupported` error with the assertion's span. Search absorbs leading/trailing anchors of each
top-level alternative into the Σ* prefix/suffix.

**Consequences.** No verdict ever rests on a guessed assertion. About 20% of random generated
patterns are currently refused (measured in `tests/coverage_stats.rs`), mostly `\b` and anchors
inside repetitions. P4 removes the restriction without changing the IR.
