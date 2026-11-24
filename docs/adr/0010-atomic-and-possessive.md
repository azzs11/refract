# ADR-0010: Exact atomic groups and possessive quantifiers for prefix-free bodies

**Context.** `(?>x)` and `x*+` commit to the first match of `x`, which changes the language and
is not expressible by plain concatenation.

**Decision.** If `L(x)` is *prefix-free* (decided on the minimal DFA: no accepting state reaches
an accepting state by a non-empty path), the first match at a position is the only one, so with
continuation `k`: `(?>x)·k = x·k`; `(?>x₁|…|xₙ)·k = ⋃ᵢ xᵢ·k ∩ ¬(x₁Σ*) ∩ … ∩ ¬(xᵢ₋₁Σ*)`;
greedy/possessive `x{n,m}` takes the maximum count: `x{n,m−1}·(¬(xΣ*) ∩ k) ∪ x{m}·k`; lazy inside
an atomic group takes `x{n}`; `(?>y·z) = (?>y)(?>z)` when `y` is prefix-free.

**Consequences.** `\d++`, `[^"]*+`, `a*+a` (never matches), `(?>a|ab)c ≡ ac` are exact and agree
with CPython and the JDK. Other bodies (e.g. `(?>a*b*c?)` combinations that are not prefix-free
and not reducible) fall back to the bounded check (ADR-0011).
