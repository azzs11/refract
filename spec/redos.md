# ReDoS lab (`refract redos`)

Per engine: predicted worst-case growth of one match (linear, polynomial `O(n^k)`, exponential),
an attack string, the measured time curve, and whether the measurement confirms the prediction.
Decision record: docs/adr/0012-redos-lab.md.

## Command

```
refract redos [-f FLAVOR] [--engines python,js,js-linear,java,go,rust] [--op full|search]
              [--timeout SECONDS] [--no-measure] [--attack JSON] [--json] PATTERN
```

- `js-linear` is V8 started with `--enable-experimental-regexp-engine-on-excessive-backtracks`.
- `--attack '{"prefix":"","pumps":["a"],"suffix":"!"}'` times that attack instead (used to replay
  other tools' attacks): pump count grows by 2, then the length doubles.
- Exit codes: 0 nothing confirmed super-linear; 1 some engine confirmed super-linear;
  2 usage/analysis error; 3 an engine was slower than the model predicts (please report).

## Model

| Engine | Model |
|---|---|
| Python `re` | backtracking: growth = ambiguity of the backtracking NFA |
| JavaScript (V8, default) | backtracking: same |
| `js-linear` | linear, unless back-references, look-around, or the `i`/`u`/`v` flag (then as V8) |
| Java | backtracking with the JDK loop memo (n × body cost for the outermost greedy group loop) |
| Go, Rust | linear: automaton engines |

The analysis treats look-arounds, back-references and anchors as ε (over-approximation), never
pivots inside atomic groups or possessive loops, and adds the engine's retry loop for
unanchored search. Every attack's suffix is checked to make the whole haystack fail.

## Measurement

Times are measured inside each runner around the single match call, min of three runs. A point
over the timeout kills that runner (a fresh one is started). Points below 200 µs are ignored for
fitting. Classes: exponential when the per-pump ratio is at least 1.25 and the log-log slope is at
least 3 (additive schedule), or the slope exceeds 6; polynomial when the slope is at least 1.5;
else linear. A predicted polynomial of degree at least 5 is confirmed by exponential-looking
growth too.

## Known limits

- Attacks are built to make the match fail. Work spent before a late *successful* match is not
  predicted (`^[\s\S]*?\s*$` on tabs then NUL: the lazy loop tries every split before succeeding).
- Look-arounds and back-references are ε in the NFA, so their own cost is not modelled.
- The attack search stops after 1.5 s; a pattern without a failing attack found in time is
  reported "ambiguous … not proven safe", never as safe.
