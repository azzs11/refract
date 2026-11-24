# ADR-0003: Automata over minterms of Unicode scalar values

**Context.** Unicode has ~1.1M scalar values; labelling transitions with single characters is
infeasible. Plan review item 7: two engines' DFAs are only comparable when built over the same
partition of the alphabet.

**Decision.** The alphabet is all Unicode scalar values (surrogates excluded; flavours that work
on UTF-16 code units, e.g. JS without `u`, will map lone surrogates explicitly in P3). For a set
of terms, compute the minterms: the coarsest partition such that every character set in the terms
is a union of parts. Automata (NFA edges, DFA rows, derivative classes) are indexed by minterm.
Both engines and both minimisers always receive the *same* `Arc<Alphabet>`.

**Consequences.** DFAs are small (minterms ≈ 2× number of distinct sets), equality of minimal
DFAs is a cheap canonical-form comparison, and witnesses are spelled with one representative per
minterm (ADR-0006). The alphabet must be rebuilt when terms are combined.
