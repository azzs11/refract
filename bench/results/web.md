# Web app benchmark (P7)

Generated from `bench/results/web.json` and `bench/results/lighthouse.json`; rerun with the
commands in [bench/web/README.md](../web/README.md).

## Five-engine refraction in WebAssembly

84 of 85 patterns from `bench/web/corpus.json` (common validation regexes + the
ReDoS corpus), written for Python, full match, compared against JavaScript, Go, Java and Rust in
a Web Worker on chromium 158.0.8127.37. Time is the worker's own measurement of the WASM call.

| median | p95 | max | target (plan §10) |
|---|---|---|---|
| 12 ms | 16 ms | 19 ms | median < 50 ms, p95 < 500 ms |

The one pattern not measured is `^\p{L}+$`: CPython `re` has no `\p{..}`, so the page
reports the syntax error (as the engine does).

## Lighthouse (mobile)

| performance | accessibility | best practices | SEO |
|---|---|---|---|
| 99 | 100 | 100 | 100 |

first-contentful-paint 1.1 s, largest-contentful-paint 1.1 s, total-blocking-time 0 ms, cumulative-layout-shift 0.066, speed-index 1.1 s.
