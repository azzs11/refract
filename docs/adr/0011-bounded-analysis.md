# ADR-0011: Bounded analysis with a backtracking reference interpreter

**Context.** Back-references (`(a+)\1`) are not regular: no automaton decides them. The plan
requires such results to be labelled with their bound and never presented as proven.

**Decision.** `interp` is a backtracking matcher over the IR implementing every construct with
each engine's observable rules (unset-group back-references match empty in JS and fail in
Python/Java; JS clears quantified captures per iteration; per-engine case-insensitive
back-reference comparison; atomic commit; Java look-behind windows; mid-pair search quirks), with
a step budget. `equiv::compare` decides exactly when both sides lower to automata, otherwise it
enumerates all haystacks up to length N (N chosen so the search stays under ~150k strings) over
one representative per minterm plus a second distinct character (so "same" vs "different
character" is visible to back-references), shortest and most readable first. A difference found
is exact and is replayed on the engines; otherwise the verdict is `NoDifferenceUpTo{N}` and
reports say "bounded, not proven".

**Consequences.** Every oracle suite also checks the interpreter against the real engine on every
pattern. Bounded witnesses are canonical within the enumerated alphabet, not globally.
