# Engine findings

Disagreements found while checking Refract against real engines that turned out to be engine
behaviour rather than Refract bugs (plan §9, Gauntlet triage). Each entry has a reproduction.

## V8: `u`-mode search tries a position inside a surrogate pair

- Engine: V8 14.6.202.34 (Node 26.13.1).
- Repro: `/(?<![\s\S])(?![\s\S])/u.exec("😀").index` → `1`.
- Expected by ECMA-262 (RegExpBuiltinExec advances with `AdvanceStringIndex(S, i, fullUnicode)`,
  which steps over a whole surrogate pair): no match, since every code-point boundary of "😀" has
  a character on one side.
- Characterised on V8 (probes in the P3 session): at a mid-pair position `^`, `$` and `\b` are
  false, `\B` is true, and look-arounds see no character on either side (`(?<=\uD83D)` and
  `(?=\uDE00)` fail), so only empty matches can occur there. More repros:
  `/\B/u.exec("a😀").index` → `2`; `/(?![\s\S])/u.exec("😀x").index` → `1`; `/^$/u.test("😀")` → false.
- How Refract handles it: modelled, not hidden. For `u`-mode *search*, if the pattern can match
  empty at such an isolated position, every haystack containing an astral character is
  accepted (`Quirks::v8_mid_surrogate_search`, `regular::matches_empty_mid_pair`). Full match is
  unaffected (the match must start at 0). Verified by `js_oracle.rs` against V8.
- Found by: `crates/refract-core/tests/js_oracle.rs`, 2026-11-24.
