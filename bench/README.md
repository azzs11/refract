# RefractBench (v0.5)

Every number in the README comes from a file in `bench/results/`. Ground truth is always the real
engines (CPython 3.15.2, V8 14.6 / Node 26, Go 1.27.4, JDK 25.0.5.1, Rust regex 1.13).

| task | data | ground truth | result |
|---|---|---|---|
| Portability (RQ1) | 2,293 regexes from library code in five ecosystems ([census](census/README.md)) | witnesses and rejections replayed on the engines | [results/census.md](results/census.md) |
| Model vs engines (RQ2) | the census, plus the property tests (random patterns × haystacks per engine) | engine outputs | census: 0 contradictions; tests: see the README evidence table |
| ReDoS (RQ3) | 65 patterns; Refract vs recheck 4.5.0 vs regexploit 1.0.0 ([redos](redos/README.md)) | measured time curves per engine | [results/redos.md](results/redos.md) |
| Translation | random patterns × 5 × 5 flavours; verified ones replayed on CPython and V8 | proof + engine replay | `crates/refract-core/tests/translation*.rs` |
| Web | 85 patterns, five-engine refraction in WASM; Lighthouse | browser timing | [results/web.md](results/web.md) |

Not yet included (planned): LLM-written regexes (RQ4: needs model access and prompts), syntax
converters as translation baselines, PCRE2/.NET/Ruby engines, and the 2019 Lingua Franca corpus
(licence to be checked; the census uses fresh 2026 library code instead).
