# Flavour: JavaScript (ECMAScript 2025 as implemented by V8 14.6 / Node 26)

Status: front-end implemented (P3) for no flags and `u`, with `i m s` (and the match-irrelevant
`d g`). `v` and `y` are reported as unsupported. Oracle: `oracle/node/runner.mjs`. Tables:
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
| `.` | any char except LineTerminator `\n \r    `; with `s`: any |
| `^` / `$` | haystack start / end (**`$` does not match before a final `\n`**); with `m`: line edges at any LineTerminator |
| `\d \w` | ASCII only; under `iu`, `\w` also matches U+017F and U+212A |
| `\s` | WhiteSpace ∪ LineTerminator (measured, includes U+FEFF) |
| `i` | without `u`: `toUpperCase`-based Canonicalize (no ASCII↔non-ASCII); with `u`: simple case folding (Kelvin sign ~ `k`) |
| Code units | without `u`: UTF-16 code units — an astral char is two units |

## Analysis domain (ADR-0007)

With `u`: all strings of scalar values. **Without `u`: strings of BMP characters only** — a
verdict about `/^.$/` is about BMP haystacks, where `.` is one character. Differences that only
appear with astral characters (e.g. `/^.$/` vs Python on "😀") are outside the analysed domain
and reports say so; exact UTF-16 modelling is planned.
