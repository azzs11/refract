# ADR-0007: Exact UTF-16 semantics for JavaScript without `u`

**Status:** accepted (revised in P4: replaces the earlier "BMP haystacks only" restriction).

**Context.** JavaScript without `u`/`v` matches UTF-16 code units: an astral character is two
units, `.` matches one half, `[😀-😂]` is "range out of order". Refract's haystacks are strings of
Unicode scalar values, and `CharSet` excluded surrogates.

**Decision.** Such a pattern carries `Domain::Utf16`. Surrogate code units are internal *pseudo
code points* (`0x110000 + (u - 0xD800)`), which `CharSet`, the alphabet (only when some set uses
them) and the interpreter understand. Lowering yields a language over code units (Σ = all code
units). `utf16::UnitDfa::lift` turns its DFA into one over scalar values by the inverse UTF-16
homomorphism — `δ'(q, c) = δ(δ(q, hi(c)), lo(c))` for astral `c` — on a scalar alphabet that
partitions astral characters by the unit classes of their halves. `automata::shared_dfas` puts
any mix of domains on one alphabet for comparison.

**Consequences.** `/^.$/` (no `u`) versus Python on "𐀀" is found and confirmed. Every JS flag set
is checked against V8 on astral haystacks. Engines that also search mid-pair (V8 with `u`, Java
`Start`) are modelled by `Quirks::mid_pair_search` (docs/findings.md).
