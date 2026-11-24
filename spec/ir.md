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
| `Look{sub, behind, negated, window}` | look-around; `window` is Java's look-behind length window |
| `Backref(n)`, `Conditional{..}` | non-regular; parsed exactly, analysed from P4 |

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
| `LineStart{terms}` | i = 0 or char i−1 is a terminator (`Lf`: `\n`; `Js`: `\n \r \u2028 \u2029`; `CrLf`: not between `\r\n`) |
| `LineEnd{terms}` | i = n or char i is a terminator (same `terms`) |
| `WordBoundary{word}` | word-ness of chars i−1 and i differ (`word` = that engine's `\w`; Java adds marks after letters) |
| `NotWordBoundary{word}` | they are the same |
| `JavaDollar{unix}` / `JavaLineEnd{unix}` / `JavaLineStart{unix}` | Java's `$`, `(?m)$`, `(?m)^` (spec/flavours/java.md) |
| `WordStart` / `WordEnd` | non-word→word / word→non-word |
| `WordStartHalf` / `WordEndHalf` | left / right side is non-word or absent |

## Operations and lowering

A pattern does not denote one language; a *(pattern, operation)* pair does (plan review item
5). `regular::lower(regex, op)` produces a regular term `Re` (∅, ε, set, concat, alt, repeat,
and, not, and the zero-width `Behind` atom), exact for every zero-width construct (ADR-0008):

- **Full match** = `node(root, ε)`; **search** = `Σ* · node(root, Σ*)`, where `node(n, k)` is
  "n followed by the continuation k up to the end of the haystack".
- Look-ahead `(?=s)` before `k` is `(s·Σ*) ∩ k`; `(?!s)` is `¬(s·Σ*) ∩ k`.
- Look-behind is `Behind{sub}`: true when the input read so far is in `Σ*·sub`; the derivative
  engine evaluates it in a (term, context) state.
- Anchors and boundaries are built from those with each engine's own sets, e.g.
  `\b` = `(?<=w)(?!w) | (?<!w)(?=w)`, `^` = `(?<![\s\S])`, multi-line `^` = `(?<![^\n])` (per
  `LineTerms`), Python `$` = `(?=\n?(?![\s\S]))`, Java's `$`/`(?m)^`/Mn-aware `\b` per their source.
- The analysis **domain** (ADR-0007) restricts JS-without-`u` to BMP haystacks; comparisons use
  the common domain of both sides.

Refused with a span (never guessed): look-ahead inside an unbounded repetition, look-ahead-type
assertions inside a look-behind, nested look-behind, and the non-regular constructs below.

## Non-regular features (P4)

Backreferences, conditionals, atomic groups and possessive quantifiers are already parsed exactly
into IR nodes; P4 adds their analysis (atomic/possessive where they stay regular, a `bounded`
path for back-references). Bounded results are always labelled with their bound ("checked up to
length 12").
