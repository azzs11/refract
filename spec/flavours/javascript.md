# Flavour: JavaScript (ECMAScript 2025 as implemented by V8 14.6 / Node 26)

Status: front-end implemented for no flags, `u` and `v`, with `i m s y` (and the match-irrelevant
`d g`). `\p{..}` uses tables measured from V8 (`tools/gen_js_props.mjs`); only `v`-mode
*properties of strings* (`\p{RGI_Emoji}` …) are reported as unsupported. Oracle: `oracle/node/runner.mjs`. Tables:
`crates/refract-core/src/tables/js.rs`, measured from V8 by `tools/gen_js_tables.mjs`
(Unicode 17.0).

Sources: ECMA-262 §22.2 (RegExp) and Annex B.1.2 (web-compatibility grammar); every rule below
was probed on V8.

## Input

`refract` accepts either the bare source (no flags) or a literal `/source/flags`, e.g.
`js:/^\w+$/iu`. Flags: `d g i m s u v y`; unknown or repeated flags, or `u` with `v`, are errors.

## Two grammars

Without `u`/`v`, V8 uses the lenient **Annex B** grammar; with `u` it uses the strict grammar.

| Construct | no flag (Annex B) | `u` |
|---|---|---|
| lone `]`, `{`, `}` | literal | error "Lone quantifier brackets" |
| `{` not forming a quantifier (`a{`, `a{,2}`) | literal | error |
| `\u{41}` | `u` repeated 41 times | U+0041 |
| unknown escape `\a`, `\-`, `\p` | the char itself (not `\c`; not `\k` if named groups exist) | error (only `^$\.*+?()[]{}|/` may be escaped) |
| `\c` + non-letter | a literal `\` then `c` | error |
| `\8`, `\9` (no such group) | `8`, `9` | error |
| `\1`–`\7` (no such group) | legacy octal | error |
| `[\d-z]` | `\d`, `-`, `z` | error |
| quantified look-ahead `(?=a)*` | allowed | error |
| `\p{L}` | `p{L}` literally | Unicode property (analysis: unsupported yet) |

Both: `[]` matches nothing, `[^]` matches anything; quantified look-behind is an error; `^*`,
`\b+`, `a**`, `{2}` → "Nothing to repeat"; `a{2,1}` → out of order; huge counts are fine;
backreferences may point forward (`\1(a)`); `\k<n>` must name an existing group when the pattern
has named groups. Named groups may repeat a name only in different alternatives
(`(?<n>a)|(?<n>b)`). Modifiers `(?ims-ims:...)`: a flag may not repeat, both sides may not be
empty; `(?i)` without `:` is an error.

## Semantics profile

| Aspect | JavaScript |
|---|---|
| Matching | backtracking, leftmost-first; V8 can fall back to a linear engine (ReDoS lab, P5) |
| `.` | any char except LineTerminator `\n \r \u2028 \u2029`; with `s`: any |
| `^` / `$` | haystack start / end (**`$` does not match before a final `\n`**); with `m`: line edges at any LineTerminator |
| `\d \w` | ASCII only; under `iu`, `\w` also matches U+017F and U+212A |
| `\s` | WhiteSpace ∪ LineTerminator (measured, includes U+FEFF) |
| `i` | without `u`: `toUpperCase`-based Canonicalize (no ASCII↔non-ASCII); with `u`: simple case folding (Kelvin sign ~ `k`) |
| Code units | without `u`: UTF-16 code units — an astral char is two units |

## `v` flag (Unicode sets)

Classes are set expressions: union, or one kind of operator per level — `&&` intersection or `--`
subtraction — whose operands are nested classes, `\q{…}` string alternatives, class escapes or
single characters (never ranges: `[a-z&&b]` is an error). Syntax characters `( ) [ ] { } / - |`
must be escaped; reserved double punctuators (`!!`, `##`, …) are errors; a negated class must not
contain strings (for `--`, only the first operand counts). Strings are tried longest first. Under
`vi`, every operand is simple-case-folded before the operations, which equals closing each result
under case folding.

## Sticky `y`

`test()` from `lastIndex` 0 only tries index 0: search is anchored at the start.

## Code units (ADR-0007)

Without `u`/`v` the engine matches **UTF-16 code units**; Refract lowers to a code-unit language
(surrogate halves as pseudo code points) and lifts its automaton to scalar haystacks exactly, so
`/^.$/` not matching "😀" is found and confirmed. With `u`/`v`, V8's search also tries positions
inside surrogate pairs (docs/findings.md); that is modelled.
