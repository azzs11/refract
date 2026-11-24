# Oracle protocol (JSON lines)

Each engine has a small runner that reads one JSON request per line on stdin and writes one
JSON response per line on stdout, in order. Runners are deliberately tiny and dependency-free so
that what they measure is the engine, not the harness.

## Request

```json
{"id": 7, "pattern": "^\\w+$", "op": "full", "inputs": ["abc", "abc\n"]}
```

| Field | Meaning |
|---|---|
| `id` | echoed back |
| `pattern` | the pattern text exactly as a user would write it in that language |
| `op` | `full` (whole haystack), `search` (match anywhere) or `compile` (only compile) |
| `inputs` | haystacks; strings of Unicode scalar values |

Special request `{"id": 0, "op": "version"}` returns the engine and its version.

## Response

```json
{"id": 7, "ok": true, "results": [true, false]}
{"id": 8, "ok": false, "error": "nothing to repeat at position 0"}
```

`ok: false` means the engine rejected the pattern. Results are booleans in input order.

Timing request (ReDoS lab): `{"id": 9, "op": "time", "mode": "full"|"search", "pattern": …,
"inputs": [haystack]}` → `{"id": 9, "ok": true, "times": [nanoseconds]}`: wall-clock time of the
match call alone, measured inside the runner. The harness kills a runner that does not answer
within its timeout (Docker: `docker kill` on the named container) and starts a fresh one.
JavaScript times full matches as `^(?:p)$` without `m` (a look-ahead wrapper would disable V8's
linear fallback).

## Operations per engine

| Engine | `full` | `search` |
|---|---|---|
| Python `re` | `re.fullmatch(p, s)` | `re.search(p, s)` |
| JavaScript | `new RegExp("(?:" + p + ")(?![\\s\\S])", flags + "y")`¹ | `new RegExp(p, flags).test(s)` |
| Go `regexp` | `regexp.MustCompile("^(?:" + p + ")$")`² | `MatchString` |
| Java | `Pattern.compile(p).matcher(s).matches()` | `.find()` |
| Rust `regex` | `\A(?:p)\z` | `is_match` (in-process, no runner) |

¹ Sticky (`y`) pins the start at index 0 and the look-ahead pins the end, whatever `m` says.
² Go `^`/`$` without `(?m)` are haystack edges.

## Running

Native: `python3 oracle/python/runner.py`, `node oracle/node/runner.mjs`.
Pinned: `docker build -t refract-oracle-python oracle/python && docker run -i --rm refract-oracle-python`.
