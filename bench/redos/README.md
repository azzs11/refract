# ReDoS benchmark

Scores ReDoS detectors against **measured** behaviour of real engines, per engine.

## Run

```console
$ cargo build --release -p refract-cli
$ for e in python node java; do docker build -t refract-oracle-$e oracle/$e; done   # or native python3/node
$ cd bench/redos && npm ci && uv venv --python 3.12 .venv && uv pip install --python .venv/bin/python -r requirements.txt && cd -
$ python3 bench/redos/run.py        # → bench/results/redos.{json,md}
```

regexploit 1.0.0 imports `sre_constants`, which Python 3.13+ removed, so it runs on Python 3.12.

## Method

- **Corpus** (`corpus.json`): 65 patterns valid in Python, JavaScript and Java. *textbook* (20):
  classic examples from the ReDoS literature; *real-world* (25): shapes that recur in published
  advisories and popular validators (trim, e-mail, URL, duration parsers, …); *everyday* (20):
  validators expected to be safe. Operation: **search** (`re.search`, `RegExp.prototype.test`,
  `Matcher.find`), what most code calls.
- **Tools**: Refract (`refract redos --no-measure` gives the prediction per engine), recheck 4.5.0
  (verdict `vulnerable`/`safe`; models JavaScript), regexploit 1.0.0 (reports starriness > 2, its
  default; models Python).
- **Ground truth** per (pattern, engine): *super-linear* when any tool's attack measures
  super-linear on that engine. All attacks are timed by the same harness, `refract redos
  --attack`: min of 3 runs per point inside the engine's runner, pump count +2 then doubling
  lengths, 2 s timeout, curve classified by log-log slope (ADR-0012). A pattern no tool attacks
  successfully counts as linear, so recall is relative to the three tools' attacks combined.
- **Score**: a tool's verdict is compared with the truth on Python `re`, V8 and `java.util.regex`.
  The same recheck/regexploit verdict is scored on every engine, which shows the cost of
  single-engine models (the JDK memoises loops, see docs/findings.md). "No verdict" = the tool
  errored or answered unknown.

Every number in docs comes from `bench/results/redos.md`.
