# Refract

**Same pattern, different meaning: see exactly where.**

Refract is a regex compiler and verifier. It parses a regex in the flavour you wrote it in, builds
a formal model of what it matches, and tells you — with the **shortest concrete string** — where
two patterns (or one pattern in two engines) disagree. Every "different" verdict is replayed on
the real engine before it is called confirmed.

> **Status: pre-alpha (phases P0–P2 of the plan).** One flavour (Rust `regex`) is implemented end
> to end. JavaScript, Python `re`, Go/RE2 and Java arrive next (P3, the v0.5 milestone), followed
> by the ReDoS lab, match spans, and the web app. Nothing is published yet; the crate will be
> `refract-regex` (the name `refract` is taken on crates.io/PyPI/npm).

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

## Try it

```console
$ cargo run -p refract-cli -- parse 'a(b|c)*'                    # print the IR
$ cargo run -p refract-cli -- equiv --a 'a*' --b 'a+'             # witness: ""
$ cargo run -p refract-cli -- equiv --op search --a '^abc' --b 'abc'   # witness: "aabc"
$ cargo run -p refract-cli -- dfa '(a|b)*abb' --dot | dot -Tsvg > dfa.svg
$ cargo run -p refract-cli -- flavors                             # semantics profiles
```

Every verdict names its **operation**: `full` (the whole haystack must match — Python
`re.fullmatch`, Java `matches()`) or `search` (a match anywhere — JS `test()`, Python
`re.search`, Rust `is_match`). Two patterns can be equivalent under one and differ under the
other.

Exit codes: `0` equivalent, `1` different (or pattern rejected), `2` usage/analysis error,
`3` a real engine contradicted the model — a Refract bug, please report it.

## How verification works

1. **Hand-written flavour front-ends** resolve everything flavour-specific — Unicode tables,
   case folding, what `.` and `$` mean — into one IR over explicit character sets
   ([spec/ir.md](spec/ir.md), [spec/flavours/rust.md](spec/flavours/rust.md)).
2. **Two independent automaton engines** — symbolic Brzozowski derivatives (with intersection and
   complement) and Thompson NFA + subset construction — must produce identical minimal DFAs, as
   must two minimisers (Hopcroft, Brzozowski), all over a shared minterm alphabet.
3. **Equivalence** is decided by breadth-first search of the product automaton; the witness is the
   unique shortest, most readable distinguishing string ([spec/witness.md](spec/witness.md)).
4. **Real-engine replay**: each witness is run on the real engine; verdicts are labelled
   `confirmed`, `predicted` (no oracle yet) or `contradicted`.
5. **Honest limits**: anything outside what is analysed (e.g. `\b` mid-pattern before P4) is
   reported as unsupported with its location, never guessed; analyses have state and time budgets.

## Evidence (from the test suite)

| Check | Result | Where |
|---|---|---|
| Parser accepts exactly what `regex-syntax` accepts | 1,000,000 random patterns, 0 disagreements, plus ~110 edge cases | `tests/rust_differential.rs` |
| Same language as `regex-syntax`'s own parse, decided by equivalence | 300,000 random patterns, 0 differences (1 parser bug found and fixed: `[^]--]`) | `tests/rust_oracle.rs` |
| Predictions vs the real `regex` crate (full match and search) | 300,000 patterns × up to 16 haystacks, 0 disagreements | `tests/rust_oracle.rs` |
| Derivative vs Thompson engine, Hopcroft vs Brzozowski | 1,000,000 random terms with ∩ and ¬, identical minimal DFAs; 0 of 5,000 sampled hit a budget | `tests/double_construction.rs`, `tests/coverage_stats.rs` |
| Witness valid, shortest (brute force ≤ 6), identical across engines | 200,000 random pairs | `tests/equiv_props.rs` |
| Algebraic laws proven (`r\|r=r`, `(r*)*=r*`, `r(s\|t)=rs\|rt`, `¬¬r=r`, `r∩¬r=∅`, De Morgan …) | 1,000 random instances each | `tests/laws.rs`, `tests/equiv_props.rs` |
| Parser fuzzing (libFuzzer) | 599,467 runs / 10 min, no crashes or hangs | `fuzz/` |

Reproduce: `cargo test --workspace` (default case counts), or set `PROPTEST_CASES` for long runs.

## Flavours

| Flavour | Front-end | Oracle | Notes |
|---|---|---|---|
| Rust `regex` | ✅ | in-process | bootstrap flavour, fully regular |
| JavaScript | P3 | Node (Docker) | v0.5 |
| Python `re` | P3 | CPython (Docker) | v0.5 |
| Go / RE2 | P3 | Go (Docker) | v0.5 |
| Java | P3 | JDK (Docker) | v0.5 |
| PCRE2, .NET, Ruby | after v0.5 | Docker | |

## Layout

```
crates/refract-core   parsers, IR, char-set algebra, derivative + Thompson engines,
                      minimisation, equivalence (library, no I/O, forbid(unsafe))
crates/refract-cli    the `refract` binary and the in-process Rust oracle
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
