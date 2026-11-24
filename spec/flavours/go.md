# Flavour: Go `regexp` (RE2 syntax, Go 1.27)

Status: front-end implemented (P3). Oracle: `oracle/go` (pinned Docker `golang:1.27-alpine`,
runner built from `runner.go`). Tables: `crates/refract-core/src/tables/go.rs`, generated from
Go's `unicode` package by `tools/gen_go_tables.go` (Go 1.27.4, Unicode 17.0.0).

Sources: `regexp/syntax/parse.go` of Go 1.27 (read from the pinned image), `regexp/syntax` docs.
`regexp.Compile` parses with `syntax.Perl` = `ClassNL | OneLine | PerlX | UnicodeGroups`.

## Semantics profile

| Aspect | Go |
|---|---|
| Matching | RE2 automata, linear time, leftmost-first |
| `.` | any rune except `\n`; with `s`: any |
| `^` / `$` | haystack start / end (`OneLine`); with `m`: line edges at `\n` |
| `\d \s \w`, `\b` | ASCII only; `\s` = `[\t\n\f\r ]` (no `\v`) |
| Negated class | `[^a]` matches `\n` (`ClassNL`) |
| `(?i)` | `unicode.SimpleFold` orbits (Kelvin sign ~ `k`); `(?i)\w` gains `ſ` and K-sign; `(?i)\p{..}` adds Go's `Fold*` tables |
| Unsupported by the engine | look-around, backreferences, `\Z`, `\C`, possessive, atomic, `(?x)` |

## Syntax rules (from `parse.go`)

- A repetition directly after another (`a**`, `a*?*`, `a{2}{3}`, `a{2}*`) → "invalid nested
  repetition operator". `(a*)*` is fine.
- A repetition with nothing to repeat (at the start, after `(` or `|`) → "missing argument".
  A flag group pushes nothing: `a(?i)*` repeats `a`; `^*` and `\b*` are allowed.
- `{` that does not parse as `{n}`, `{n,}`, `{n,m}` (no leading zeros) is a literal. Counts above
  1000, or `min > max`, are errors; nested counted repetitions may not multiply past 1000.
- Escapes: `\A \z \b \B`, `\Q...\E` (an unterminated `\Q` runs to the end), `\pN`, `\p{Name}`,
  `\p{^Name}`, `\PN`; `\d \D \s \S \w \W`; `\a \f \t \n \r \v`; octal `\0`, `\0oo`, `\ooo` (a lone
  `\1`–`\7` is an error); `\xHH`, `\x{H…}` (≤ U+10FFFF); any ASCII punctuation (incl. `\_`) is
  itself; anything else (`\e`, `\Z`, `\C`, `\8`, non-ASCII) is an error.
- `\p` names are canonicalised (first letter upper, rest lower, `_ - space` dropped) and looked
  up as `Any`, `Assigned` (= not `Cn`), `Ascii`, `Lc`, a category, a script, a category alias,
  then a script alias — so `\p{greek}` works and `\p{Script=Greek}` does not.
- Classes: `]` first is literal, `[]` is unterminated, `-` is literal anywhere it cannot form a
  range, ranges need character ends (`[a-\d]` is an error, `[\d-z]` is `\d`, `-`, `z`), `[:name:]`
  with an unknown name is an error, no set operations.
- Groups: `(?:)`, `(?P<name>)`, `(?<name>)` with names `[A-Za-z0-9_]+` (**duplicates allowed**),
  flag groups `(?flags)` / `(?flags:...)` with flags `i m s U` and one `-`; `(?-)` is an error.
