# Refract IR

The IR is the single representation every flavour front-end produces. Its design goal: **after
parsing, nothing about the flavour or its flags is needed to know what a node means.** All
flavour-dependent meaning is resolved by the front-end and recorded explicitly.

Source: `crates/refract-core/src/ir.rs`.

## Nodes

| Node | Meaning |
|---|---|
| `Empty` | matches the empty string |
| `Literal(c)` | exactly the scalar value `c` (case-sensitive) |
| `Class(set)` | any one scalar value in `set`; an empty set never matches |
| `Concat([n…])` | sequence |
| `Alt([n…])` | alternation, in source order (order matters for leftmost-first spans, P4) |
| `Repeat{sub, min, max, greedy}` | `max = None` is unbounded; greediness only affects spans |
| `Group{sub, capture}` | capture index (1-based) and optional name, or non-capturing |
| `Assert(a)` | zero-width assertion, see below |

Every node carries the byte `Span` of the pattern text it came from, so any later finding (an
unsupported construct, a ReDoS pump, a non-portable class) can point at the exact source.

## What the front-end resolves

- **Character classes** become explicit `CharSet`s: Unicode tables of that engine's Unicode
  version, POSIX classes, set operations, case folding (simple vs full, Unicode vs ASCII), and
  `.` (what it excludes, with and without dot-all).
- **Case-insensitivity** is gone after parsing: a folded literal becomes a `Class`.
- **Anchors** become precise `Assertion`s. `$` is never stored as "dollar"; it is stored as
  `TextEnd` (Rust, Go, JS) or `TextEndOrFinalNewline` (Python, PCRE2, Java, .NET, Ruby), or as
  `LineEnd{crlf}` under multi-line.
- **Greediness** is resolved (Rust's `U` flag swaps it at parse time).

## Character sets

A `CharSet` is a canonical list of inclusive intervals over Unicode scalar values (surrogates
excluded): sorted, non-overlapping, non-adjacent. Canonical form makes set equality value
equality. Algebra: union, intersection, complement, difference, symmetric difference; all
property-tested against membership definitions and Boolean laws.

## Assertions

| Assertion | True at position i when |
|---|---|
| `TextStart` | i = 0 |
| `TextEnd` | i = n |
| `TextEndOrFinalNewline` | i = n, or i = n−1 and the last char is `\n` |
| `LineStart{crlf}` | i = 0 or char i−1 is `\n` (crlf: `\r` or `\n`, but not between `\r\n`) |
| `LineEnd{crlf}` | i = n or char i is `\n` (crlf: `\r` or `\n`, but not between `\r\n`) |
| `WordBoundary{unicode}` | word-ness of chars i−1 and i differ |
| `NotWordBoundary{unicode}` | they are the same |
| `WordStart` / `WordEnd` | non-word→word / word→non-word |
| `WordStartHalf` / `WordEndHalf` | left / right side is non-word or absent |

## Operations and lowering

A pattern does not denote one language; a *(pattern, operation)* pair does (plan review item
5). `regular::lower(regex, op)` produces a pure regular term `Re` (∅, ε, set, concat, alt,
repeat, and, not):

- **Full match**: the whole haystack matches. Assertions whose truth is statically known are
  replaced: `TextStart` where everything before it is zero-width at the haystack start → ε;
  where something before it must consume → ∅. Symmetrically for the end.
- **Search**: Σ* r Σ*, with each top-level alternative's leading `TextStart` / `LineStart`
  and trailing `TextEnd` / `TextEndOrFinalNewline` / `LineEnd` absorbed into the prefix/suffix
  (e.g. trailing `TextEndOrFinalNewline` becomes the suffix `(ε | \n)`).

Any assertion whose truth depends on text the analysis cannot see statically is rejected with
`Unsupported` and its span (ADR-0004). It is never guessed. Derivatives with look-around context
(P4) remove this restriction.

## Non-regular features (P4)

Backreferences, look-around and recursion will be added as IR nodes with a `bounded` analysis
path; results from that path are always labelled with their bound ("checked up to length 12").
