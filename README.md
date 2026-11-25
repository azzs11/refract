# Refract

**Same pattern, different meaning: see exactly where.**

Refract is a regex compiler and verifier. It parses a regex in the flavour you wrote it in, builds
a formal model of what it matches, and tells you — with the **shortest concrete string** — where
two patterns (or one pattern in two engines) disagree. Every "different" verdict is replayed on
the real engine before it is called confirmed.

> **Status: pre-alpha (phases P0–P7).** Five flavours are implemented end to end — **JavaScript
> (V8), Python `re`, Go `regexp`, Java `java.util.regex`** and Rust `regex` — including
> **look-around and assertions anywhere in the pattern**, a **ReDoS lab** that predicts
> worst-case matching time per engine and measures it, **verified translation** between
> flavours and **test generation**, a **web app** that runs the analysis in WebAssembly, and
> `refract check` for CI (GitHub Action), editors (VS Code) and assistants (MCP). Every verdict
> is replayed on the real engines. Next: RefractBench and the release (P8). Nothing is published
> yet; the crate will be `refract-regex`.

One pattern, every engine — the shortest input where each disagrees, confirmed on the engines:

```console
$ refract refract --from python '^\w+@\w+\.com$'
pattern    ^\w+@\w+\.com$
written    python  (python 3.15.2)
operation  full-match

engine   verdict       shortest witness       python / engine        status
js       DIFFERS       "a@ª.com"              match / no match       confirmed
go       DIFFERS       "a@ª.com"              match / no match       confirmed
java     DIFFERS       "a@ª.com"              match / no match       confirmed
rust     DIFFERS       "a@².com"              match / no match       confirmed
```

Python's `\w` is `str.isalnum()`-based, so it matches U+00AA (ª) and U+00B2 (²); JS, Go and Java
use ASCII `\w`, and Rust's UTS#18 `\w` excludes `²`. Under `--op search`, `abc$` shows the other
classic: Python, Java and friends let `$` match before a final newline (`"abc\n"`), and Java also
before `"\r"`.

Two patterns, one verdict:

```console
$ refract equiv --a 'rust:\d+' --b 'rust:[0-9]+'
operation  full-match
a          rust   \d+
b          rust   [0-9]+
verdict    DIFFERENT
witness    "٠"  (length 1, shortest possible)
           a matches it, b does not
status     confirmed on the real engines
           a: match on rust regex 1.13 (in-process)
           b: no match on rust regex 1.13 (in-process)
```

`\d` is Unicode-aware in Rust, so it matches ARABIC-INDIC DIGIT ZERO; `[0-9]` does not.

Translate, with a proof — the printed pattern is re-parsed by the target's front-end, proven
equivalent under full match and search, and replayed on both real engines:

```console
$ refract translate --from js --to python,java,go '/(?<=\$)\d+(?:\.\d\d)?\b/u'
python VERIFIED (full match and search)  (?<=\$)[0-9]+(?:\.[0-9][0-9])?(?a:\b)
         full-match proven                                       confirmed on 2 test strings
         search     proven                                       confirmed on 8 test strings
java   VERIFIED (full match and search)  (?<=\$)\d+(?:\.\d\d)?(?:(?<=\w)(?!\w)|(?<!\w)(?=\w))
         ...
go     UNTRANSLATABLE  go has no look-around  (at `(?<=\$)`)
```

JS `\d` and `\b` are ASCII; Python's are Unicode, so they become `[0-9]` and `(?a:\b)`. Java's
`\b` also counts marks after letters as word characters, so it is spelled out with look-around.
A Python `$` has no form in Go or Rust, so `^\w+$` translates there for full match only
(`\A…\z`) — and the report says so.

Tests that cover every state and transition of the minimal DFA, confirmed on the engine:
`refract tests -f python --format pytest '^\d{3}-\d{4}$'` (also `jest`, `junit`, `go`, `json`).

## Try it

```console
$ cargo run -p refract-cli -- parse 'a(b|c)*'                    # print the IR
$ cargo run -p refract-cli -- equiv --a 'a*' --b 'a+'             # witness: ""
$ cargo run -p refract-cli -- equiv --op search --a '^abc' --b 'abc'   # witness: "aabc"
$ cargo run -p refract-cli -- dfa '(a|b)*abb' --dot | dot -Tsvg > dfa.svg
$ cargo run -p refract-cli -- refract --from js --op search '(?<=\$)\d+(?!\.)'   # look-around
$ cargo run -p refract-cli -- redos '^(\w+\s?)+$'                  # ReDoS per engine, measured
$ cargo run -p refract-cli -- flavors                             # semantics profiles
```

Real engines: Rust runs in-process; Python and JavaScript use `python3`/`node` when installed;
Go and Java (and the others, with `REFRACT_ORACLE=docker`) run in pinned images:

```console
$ for e in python node go java; do docker build -t refract-oracle-$e oracle/$e; done
```

Every verdict names its **operation**: `full` (the whole haystack must match — Python
`re.fullmatch`, Java `matches()`) or `search` (a match anywhere — JS `test()`, Python
`re.search`, Rust `is_match`). Two patterns can be equivalent under one and differ under the
other.

Exit codes: `0` equivalent, `1` different (or pattern rejected), `2` usage/analysis error,
`3` a real engine contradicted the model — a Refract bug, please report it.

## Web app, CI, editor, assistant

- **Web** (`web/`, the "optics bench"): one row per engine with the witness, the minimal DFA
  with the witness path lit, the ReDoS lab with timing in your browser, and verified
  translation. The analysis runs in WebAssembly in a Web Worker; nothing is sent anywhere;
  JavaScript verdicts are replayed live on the browser's own engine. Five-engine refraction:
  median 12 ms, p95 16 ms; Lighthouse 99 / 100 / 100 / 100; axe clean on every screen
  ([bench/results/web.md](bench/results/web.md)).
  `cd web && npm ci && npm run wasm && npm run dev`.
- **`refract check src/`** finds regex literals in Python, JS/TS, Go, Java and Rust files and
  reports patterns the engine rejects, predicted ReDoS, and (`--targets js,go`) meaning changes,
  each with the command that confirms it on the real engine. `--format github` annotates pull
  requests; the [GitHub Action](integrations/github-action/README.md) wraps it.
- **VS Code**: [integrations/vscode](integrations/vscode/README.md) shows the same findings as
  diagnostics.
- **MCP**: `refract mcp` serves the tools `refract`, `equiv`, `translate`, `redos` and `tests`
  over stdio, so an assistant can check the regexes it writes
  (`claude mcp add refract -- refract mcp`).

## How verification works

1. **Front-ends that mirror each engine's own parser** (CPython's `_parser.py`, V8's Annex B and
   Unicode grammars, Go's `regexp/syntax`, OpenJDK's `Pattern.java`, `regex-syntax`) resolve every
   flavour-specific meaning — Unicode tables **measured from each engine**, case folding, `.`, `$` —
   into one IR over explicit character sets ([spec/](spec/)).
2. **Exact look-around**: look-ahead by continuation passing (`(?=s)` before `k` is `(s·Σ*) ∩ k`),
   look-behind by a context carried in the automaton state; `\b`, `^`, `$` anywhere are built
   from those with each engine's own sets ([ADR-0008](docs/adr/0008-lookaround-construction.md)).
3. **Two independent automaton engines** — symbolic Brzozowski derivatives and Thompson NFA +
   subset construction — and two minimisers must agree, over a shared minterm alphabet.
4. **Equivalence** by breadth-first search of the product automaton; the witness is the unique
   shortest, most readable distinguishing string ([spec/witness.md](spec/witness.md)).
5. **Real-engine replay**: every witness and every rejection is run on the real engines; verdicts
   are `proven`, `confirmed`, `predicted` (engine unavailable) or `contradicted` (a bug, exit 3).
6. **Beyond regular**: back-references are not regular, so comparisons that involve them are
   **bounded** — every haystack up to length N, checked by a reference interpreter with each
   engine's rules — and reported as "no difference up to length N", never as proven. Atomic groups
   and possessive quantifiers are exact when their body is prefix-free.
7. **ReDoS lab** ([spec/redos.md](spec/redos.md), [ADR-0012](docs/adr/0012-redos-lab.md)):
   ambiguity of the backtracking NFA (exponential / polynomial degree) with an attack string,
   per-engine models (Go and Rust linear; V8's linear fallback and where it stops applying; the
   JDK's loop memo, which makes `(a+)+` quadratic, not exponential), then the attack is timed on
   each engine and the prediction is only called confirmed when the measured curve agrees.
8. **Verified translation** ([ADR-0013](docs/adr/0013-verified-translation.md)): the IR is
   printed in the target using only shorthands the *target* resolves to the same characters,
   re-parsed by the target's front-end, and proven equivalent; constructs with no form are named.
9. **Honest limits**: the few constructs still not analysed (Java `\X`, `\b{g}`, `\N{}`, `(?U)`,
   `(?c)`; Python `\N{}`; JS properties of strings) are refused with their location.

## Evidence (from the test suite)

| Check | Result | Where |
|---|---|---|
| Accepts exactly what each real engine accepts | 100,000 random patterns per engine (Python, JS with 10 flag sets, Go, Java), 0 disagreements, plus ~70–120 hand-probed edge cases each | `tests/{python,js,go,java}_oracle.rs` |
| Predictions vs the real engines, full match and search, look-around included | 100,000 patterns × up to 16 haystacks per engine, 0 disagreements | same files, `tests/rust_oracle.rs` |
| Rust parser vs `regex-syntax` | 1,000,000 random patterns, 0 disagreements | `tests/rust_differential.rs` |
| Look-behind engine vs direct semantics | 100,000 random terms | `tests/lookbehind_engine.rs` |
| Derivative vs Thompson, Hopcroft vs Brzozowski | 1,000,000 random terms, identical minimal DFAs | `tests/double_construction.rs` |
| Witness valid, shortest (brute force ≤ 6), identical across engines | 100,000 random pairs | `tests/equiv_props.rs` |
| Bugs the oracles caught (all fixed, kept as regressions) | 2 in the Rust parser (plus 1 slow path), 2 in the Python parser, 5 in the oracle runners, 4 in JS/Go/Java parsing and lowering, 1 in the reference interpreter (empty loop iterations keep their captures in CPython and the JDK) | test files, [docs/findings.md](docs/findings.md) |
| ReDoS prediction vs measured engines (65 patterns, search) | precision 100% on Python, V8 and the JDK; recall 97.6% / 97.6% / 97.0%; 110 verdicts confirmed super-linear by timing. recheck 4.5.0: 100% / 100% / 80.5% precision (it models one engine); regexploit 1.0.0: recall 48.8% / 48.8% / 60.6% | [bench/results/redos.md](bench/results/redos.md) |
| Translations parse in the target and never change meaning | 50,000 random patterns × 5 source × 5 target flavours, 0 printer bugs, 0 differences (except V8's mid-surrogate search positions, which have no form elsewhere) | `tests/translation.rs` |
| Verified translations on the real engines | 20,000 random Python↔JS patterns; every coverage string gets the model's answer on CPython and V8 | `tests/translation_engines.rs` |
| Regex literals found in source files | 20,000 random files per language, no panics; a planted call is always found | `tests/extract_props.rs` |
| Engine behaviour found | V8 `u`-mode search tries positions inside surrogate pairs; the JDK also searches inside pairs for BMP-only patterns; the JDK memoises loops (ReDoS); V8's linear fallback skips look-around, back-references, `i`/`u` (all modelled) | [docs/findings.md](docs/findings.md) |

Reproduce: `cargo test --workspace`; set `PROPTEST_CASES` for long runs, `REFRACT_ORACLE=docker`
to use the pinned engine images.

## Flavours

| Flavour | Front-end | Oracle | Unicode | Notes |
|---|---|---|---|---|
| JavaScript | ✅ no flags (Annex B, UTF-16 units), `u`, `v`, `i m s y`, `\p{..}` | V8 14.6 (Node 26) | 17.0 | properties of strings not analysed yet |
| Python `re` | ✅ | CPython 3.15.2 | 17.0.0 | `\N{..}` not analysed yet |
| Go `regexp` | ✅ incl. `\p{..}` | Go 1.27.4 | 17.0.0 | |
| Java | ✅ incl. `\p{..}`, look-behind windows, code-unit search | JDK 25.0.5.1 | JDK 25 | `\X`, `\b{g}`, `(?U)`, `(?c)` not analysed yet |
| Rust `regex` | ✅ | in-process | regex-syntax | bonus flavour |
| PCRE2, .NET, Ruby | planned | | | |

## Layout

```
crates/refract-core   parsers, IR, char-set algebra, derivative + Thompson engines,
                      minimisation, equivalence (library, no I/O, forbid(unsafe))
crates/refract-cli    the `refract` binary (parse, dfa, equiv, refract, redos, translate, tests, check, mcp, flavors)
crates/refract-oracle real-engine harness (JSON lines, native or pinned Docker)
crates/refract-wasm   WebAssembly bindings for the web app
web/                  the optics bench (Vite + Svelte, Playwright + axe tests)
integrations/         GitHub Action, VS Code extension
oracle/               one runner + Dockerfile per engine
tools/                table generators that run inside each engine
spec/                 IR, witness definition, per-flavour semantics (spec before code)
docs/adr/             architecture decision records
fuzz/                 cargo-fuzz targets (nightly)
```

## Related work

Refract builds on and is compared against existing tools rather than claiming to beat them:
RegexBuddy (flavour comparison by emulation), regex101/RegExr (one engine at a time),
gruhn/regex-utils, regexbench, php-regex/regex-automata and RegexSolver (single-flavour
equivalence), recheck/regexploit/Regulator (ReDoS detection), RegexScalpel/REMEDY (ReDoS repair),
and the study by Davis et al., *Why Aren't Regular Expressions a Lingua Franca?* (ESEC/FSE 2019).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
