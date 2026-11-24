# Flavour: Python `re` (CPython 3.15, `str` patterns)

Status: front-end implemented (P3). Oracle: `oracle/python/runner.py` (native or pinned Docker
`python:3.15.2-alpine`). Tables: `crates/refract-core/src/tables/python.rs`, measured from the
engine by `tools/gen_python_tables.py` (CPython 3.15.2, Unicode 17.0.0).

Sources: `Lib/re/_parser.py` and `Lib/re/_compiler.py` (CPython 3.15), the `re` module docs
<https://docs.python.org/3/library/re.html>. Every rule below was probed on the real engine.

## Semantics profile

| Aspect | Python |
|---|---|
| Matching | backtracking, leftmost-first; exponential worst case (no memoisation) |
| `.` | any char except `\n`; with `s`: any |
| `$` | **end of haystack or just before a final `\n`** (`TextEndOrFinalNewline`) |
| `\Z`, `\z` | end of haystack only |
| `^` / `$` with `m` | line start / end; line terminator `\n` only |
| `\d \w \s` | Unicode by default, *Python's* definitions: `\w` = `str.isalnum()` or `_`; `\s` includes `\x1c`–`\x1f`. ASCII with `a` |
| `\b` | boundary between Python-`\w` and non-`\w` |
| `(?i)` | measured relation: a literal c matches d iff `re.fullmatch("(?i)" + re.escape(c), d)`; ASCII-only under `a` |
| Unsupported by the engine | `\p{..}`, possessive/atomic before 3.11 (supported in 3.15), set operations, `\G`, recursion |

## Syntax (differences from Rust that matter)

- `{` that does not form a valid quantifier is a **literal** (`a{`, `a{x}`, `a{ 2}`), and `{}` is
  literal. `{,n}` is valid (`{0,n}`), `{,}` is `*`. Counts up to 4294967294.
- Quantifier on nothing or on an anchor (`^*`, `\b*`) → "nothing to repeat". A quantifier on a
  quantifier (`a**`, `a{2}{3}`, `a*?+`) → "multiple repeat". `(?:a*)*` is fine.
- Lazy `*?`, possessive `*+ ++ ?+ {n,m}+`, atomic `(?>...)`.
- Escapes: `\a \b(class: backspace) \f \n \r \t \v \\`, `\xhh` (exactly 2), `\uhhhh`, `\Uhhhhhhhh`
  (must be a code point), `\N{NAME}`, octal `\0`, `\0oo`, `\ooo` (≤ 0o377), backreferences `\1`–`\99`
  (to a closed group only). An escaped ASCII letter that is not one of these is an error; any
  other escaped character is itself.
- In a class: `]` first is literal; `[]` alone is unterminated; ranges need literal ends; `&& -- ~~ ||`
  are literal (Python only warns). `\b` is backspace; `\A \B \Z \z` are errors.
- Groups: `(?:)`, `(?P<name>)` (name: `str.isidentifier()`), `(?P=name)`, `(?#comment)` (transparent
  to quantifiers), look-around `(?=) (?!) (?<=) (?<!)`, conditionals `(?(id/name)yes|no)` (forward
  numeric references are checked at the end), atomic `(?>)`.
- Flags `a i L m s u x`. `(?flags)` with no `:` is *global* and must come first (only whitespace or
  comments before it in verbose mode, and other global flag groups). `L` is an error for `str`;
  `a` and `u` are incompatible; type flags cannot be turned off; global flags cannot be scoped off.
- Verbose (`x`): whitespace and `#` comments are skipped between tokens, **not** inside classes or
  quantifier braces.
- Look-behind must be fixed width (checked at compile time).

## Analysis domain

Haystacks are strings of Unicode scalar values. Python `str` can also hold lone surrogates; a
pattern literal such as `\ud800` is modelled as a class that matches nothing in that domain.
