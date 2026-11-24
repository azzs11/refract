# Flavour: Java `java.util.regex` (JDK 25)

Status: front-end implemented (P3). Oracle: `oracle/java` (pinned Docker `eclipse-temurin:25`,
`Pattern.compile(p)` with no flags; full = `matcher.matches()`, search = `matcher.find()`).
Tables: `crates/refract-core/src/tables/java.rs` from `java.lang.Character` of Java 25.0.5.1
(`tools/GenJavaTables.java`).

Source: OpenJDK `src/java.base/share/classes/java/util/regex/Pattern.java` at tag `jdk-25-ga`;
the parser is a port of its `sequence`, `atom`, `closure`, `escape`, `clazz`, `range`, `group0`,
`addFlag` and `RemoveQEQuoting`.

## Semantics profile

| Aspect | Java |
|---|---|
| Matching | backtracking, leftmost-first |
| `.` | any char except `\n \r \u0085    `; `(?d)`: except `\n`; `(?s)`: any |
| `$`, `\Z` | end, or before a final `\r\n` or single final `\n \r \u0085    ` (never between `\r\n`) |
| `(?m)^` | start or after a terminator (not between `\r\n`), **never at the very end** (so not on `""`) |
| `(?m)$` | before any terminator (not between `\r\n`) or at the end |
| `\d \w \s` | ASCII; `\s` = `[ \t\n\x0B\f\r]`; `\h`, `\v` (vertical whitespace, *not* VT), `\R` = `\r\n` or one of `[\n\x0B\f\r\x85  ]` |
| `\b` | ASCII word char, **or a non-spacing mark following a Unicode letter/digit** |
| `(?i)` | ASCII-only; `(?iu)`: per-char `toLowerCase(toUpperCase(c))` comparison for literals, `c ∨ toUpperCase(c) ∨ toLowerCase(toUpperCase(c)) ∈ range` for ranges; predicates (`\w`, `\p`) are never case-folded |

## Syntax notes (each probed on JDK 25)

- `*`, `+`, `?` with nothing to repeat → "Dangling meta character". `a**`, `a{2}*`, `a*??`,
  `a(?i)*` are errors. A `{` where an atom should start is an *empty* atom, so `{2}` and `a{2}{3}`
  are valid. After an atom, `{` must start a valid `{n}`, `{n,}`, `{n,m}` (`a{`, `x{a}`, `a{,2}` → "Illegal
  repetition"); counts up to 2³¹−1.
- `]` and `}` are literals. `[]` is unterminated but `[]a]` is a class with `]`.
- Escapes: any non-letter (incl. non-ASCII, `\_`) is itself; unknown ASCII letters are errors;
  `\0` needs an octal digit; `\1`–`\9` are backreferences (forward references allowed; multi-digit
  only while that many groups exist); `\cX` is `X ^ 64` for any `X`; `\xHH`, `\x{H…}`, `\uHHHH`
  (surrogate pairs join); `\Q…\E` quoting is a pre-pass (unterminated `\Q` quotes to the end).
- Classes: nested `[a[b]]` union, `&&` intersection with JDK's left-to-right quirks, `^` only right
  after `[`, ranges need literal ends (`[a-\d]` error; `[\d-z]` is `\d`, `-`, `z`).
- Groups: `(?<name>...)` with `[A-Za-z][A-Za-z0-9]*` (no `_`, unique), `\k<name>` (must already be
  defined), look-around, atomic `(?>...)`, flags `i m s d u c x U` with `-`; `(?)` and `(?-)` are
  valid; `(?#...)`, `(?P<..>)` are errors.
- `(?x)` skips whitespace and `#` comments everywhere, including inside classes.

## `\p{..}`

Resolved like `Pattern.family()` + `CharPredicates`: `name=value` (`sc`/`script`, `blk`/`block`,
`gc`/`general_category`), `In…` blocks, `Is…` (Unicode property, then `forProperty`, then script),
else `forProperty` (exact case). Sets, `(?i)` variants and every accepted block/script spelling are
generated inside the JDK (`tools/GenJavaProps.java`, reading the JDK's own alias maps).

## Search over code units

When no predicate is outside the JDK's `BmpCharPredicate`s and the pattern text has no astral
characters, `find()` advances one UTF-16 unit at a time and so also tries positions inside
surrogate pairs (`\B` matches there). Refract ports `hasSupplementary` and models this. `\b`'s
base-character scan walks UTF-16 units and stops at an astral base (docs/findings.md).

## Not analysed yet (reported as unsupported)

`\X`, `\b{g}`, `\N{..}`, `(?c)` canonical equivalence and `(?U)` Unicode character classes.
Back-references and atomic/possessive bodies that are not prefix-free are handled by the bounded
check (ADR-0011).
