# Flavour: Rust `regex` (1.x, `regex-syntax` 0.8)

Status: front-end implemented (P0). Oracle: in-process `regex` crate (ADR-0005).

Sources:
- `regex` crate syntax docs: <https://docs.rs/regex/latest/regex/#syntax>
- `regex-syntax` AST/HIR docs: <https://docs.rs/regex-syntax/0.8/regex_syntax/>
- UTS #18 (Unicode regular expressions), RL1.2a for `\w`.

The Rust flavour is the bootstrap flavour: it is fully regular (no backreferences, no
lookaround), so every Rust regex can be decided exactly. It is also the closest relative of Go/RE2.

## Semantics profile (default options)

| Aspect | Rust |
|---|---|
| Matching | Leftmost-first (same as backtracking engines), linear time guaranteed |
| `.` | Any scalar value except `\n`. With `s`: any. With `R` (CRLF mode): except `\r` and `\n` |
| `^` / `$` | Haystack start / haystack end (`\A` / `\z`). **`$` does not match before a final `\n`.** |
| `^` / `$` with `m` | Line start / end, line terminator `\n`; with `R`, `\r\n`-aware and never between `\r` and `\n` |
| `\d \s \w` | Unicode by default: `\p{Nd}`, `\p{White_Space}`, UTS#18 word chars. With `-u`: ASCII |
| `\b \B` | Unicode word boundary by default; ASCII with `-u` |
| Case-insensitive `i` | Unicode *simple* case folding (e.g. `k` ~ `K` ~ U+212A KELVIN SIGN); ASCII-only with `-u` |
| Unicode version | Whatever `regex-syntax` 0.8 ships (we use the same tables) |
| Unsupported | Backreferences, lookaround, possessive quantifiers, atomic groups, `\Z`, `\G`, recursion |

## Syntax accepted

Literals: any char except the meta characters `\ . + * ? ( ) | [ ] { } ^ $`. `]` and `}` outside a
class are literal.

Escapes:
- Any ASCII character other than `[0-9A-Za-z<>]` may be escaped and means itself (this includes
  space and `#`, which matters in `x` mode).
- `\a \f \t \n \r \v` (BEL, FF, TAB, LF, CR, VT).
- `\xHH` (exactly two hex digits), `\x{H...}`, `\uHHHH`, `\u{H...}`, `\UHHHHHHHH`, `\U{H...}`. The value
  must be a Unicode scalar value.
- `\d \D \s \S \w \W`, `\pN`, `\PN`, `\p{Name}`, `\P{Name}`, including `\p{sc=Greek}` style. Property
  name matching is loose (case, spaces, `_` and `-` ignored), as in UTS#18.
- Assertions: `\A \z \b \B \< \>`, `\b{start} \b{end} \b{start-half} \b{end-half}`.
- `\0`–`\9`: error (backreferences unsupported; octal off by default).

Classes `[...]`:
- Negation `[^...]`. A `]` first (after optional `^`) is literal. A `-` first or last is literal.
- Ranges `a-z` need literal (or escaped-literal) endpoints; `lo <= hi`.
- Nested classes `[a[bc]]` (union), POSIX `[[:alpha:]]`, `[[:^alpha:]]` (ASCII definitions).
- Set operations, lowest precedence, left-associative: `&&` intersection, `--` difference,
  `~~` symmetric difference.
- Case-insensitive: every item and operand is folded, then the whole class is folded, then negated.

Groups: `(...)` capturing, `(?:...)`, `(?P<name>...)`, `(?<name>...)`, flag directives `(?flags)`
(apply until the end of the enclosing group, across `|`) and scoped `(?flags:...)`. Flags:
`i m s U u x R`, negation with `-`. Empty flag lists, a dangling `-`, duplicates and repeated
negation are errors. Names: first char `_` or a letter, then letters, digits, `_ . [ ]`; must be
unique.

Repetition: `* + ? {n} {n,} {n,m}`, each optionally followed by `?` (lazy). `U` swaps greedy and
lazy. `{n,m}` requires `n <= m`. A repetition with nothing before it is an error.

`x` mode: whitespace is ignored and `#` starts a comment to the end of the line, both outside and
inside classes. An escaped space or `\#` is a literal.

`-u` (Unicode off) in the `&str` API: anything that could match invalid UTF-8 is an error, e.g.
`(?-u:.)`, `(?-u:\W)`, `(?-u:[^a])`, `(?-u:\xFF)`.

Limits: nesting depth 250 (error beyond it, like `regex-syntax`'s default nest limit).

## How this spec is checked

`tests/rust_differential.rs` generates random patterns from an alphabet dense in meta characters
and requires that our parser accepts exactly the patterns `regex-syntax` accepts, and (P2) that the
two parses denote the same language, decided by our equivalence checker and confirmed on the real
`regex` crate. Every disagreement found so far is a test case in that file.
