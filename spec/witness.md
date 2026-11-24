# Witnesses

A **witness** for two patterns under an operation is a haystack on which exactly one of them
matches. Refract always reports a *canonical* witness, so the answer does not depend on which
engine or which run produced it.

## Definition

Let L₁, L₂ be the languages of the two patterns under the chosen operation, and let the minterm
alphabet be the finest partition of code points such that every character set in either pattern
is a union of minterms. Each minterm has a **representative**: its most readable member, chosen
by rank, then lowest code point:

1. lower-case ASCII `a–z`
2. upper-case ASCII `A–Z`
3. digits `0–9`
4. ASCII punctuation
5. space
6. readable Unicode (letters, numbers, punctuation, symbols: `\p{L}`, `\p{N}`, `\p{P}`, `\p{S}`)
7. newline
8. other ASCII control characters
9. anything else

Minterms are ordered by their representative's (rank, code point). The witness is the
**shortest** string in L₁ △ L₂ spelled with representatives, and among those of that length the
**lexicographically least** in minterm order.

## Why this definition

- *Shortest* makes the difference easy to see.
- *Readability order* prefers `"ba"` over `"\u{0}a"` and `"٠"` (a real Unicode digit) over an
  unassigned code point.
- *Uniqueness* means both automaton engines must return exactly the same witness, which the test
  suite checks (`equiv_props.rs::engines_return_the_same_verdict`).

## Algorithm

Breadth-first search over the product automaton from the pair of start states, expanding
minterms in readability order. BFS with ordered edges discovers each product state first along
its lexicographically least shortest path, so the first state found where exactly one side
accepts yields the canonical witness. The derivative engine builds the product lazily and stops
at that first state; "equivalent" means the whole reachable product was explored.

## Checks (doc §9)

- **Validity**: every witness is accepted by exactly the side reported, checked by an
  independent set-based matcher (`Re::matches_naive`).
- **Shortness**: brute force over all representative strings shorter than the witness (up to
  length 6) finds no distinguishing string.
- **Equivalence claims**: brute force up to length 5 finds no distinguishing string.
- **Reality**: for the Rust flavour, the CLI replays every witness on the real `regex` crate and
  labels the verdict `confirmed`, `predicted` (no oracle yet) or `contradicted` (a Refract bug,
  exit code 3).

## Display

Witnesses are printed as escaped, quoted strings (`"abc\n"`, `"\u{A0}"`) so invisible
characters are never hidden. The web view (P7) will render them as visible glyphs.
