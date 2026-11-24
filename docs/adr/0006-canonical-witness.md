# ADR-0006: Canonical shortest readable witness

**Context.** Many strings distinguish two patterns. Users need the one that explains the
difference fastest, and tests need a witness that does not depend on the engine used.

**Decision.** The witness is the shortest distinguishing string over minterm representatives,
lexicographically least in readability order (spec/witness.md). It is computed by BFS over the
product with edges expanded in that order; the derivative engine explores the product lazily and
stops at the first witness.

**Consequences.** Witnesses are unique, so both engines must return identical ones (tested).
Readability ranking is a fixed, documented table; changing it changes outputs and needs a new ADR.
