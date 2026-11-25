# Web app ("optics bench") — P7 design

Status: **superseded** by the plan tab "Web app: design and build spec" (summarised in the
project notes); this file describes the P7 app (Vite + Svelte), which UI0–UI6 replace. Source:
plan §11 and §5.10.

## Goals

- A first-time user finds a portability bug in under 60 s (plan §10): paste a regex, pick where it
  came from, see per engine whether it means the same, with the shortest witness.
- Analysis runs **in the browser** (refract-core compiled to WASM); nothing is sent to a server.
- Every verdict links to its evidence. In the browser only JavaScript can be replayed live (the
  page's own V8/SpiderMonkey/JSC); other engines show the CLI's oracle status as "predicted"
  unless a confirmed result is loaded from a permalink or a report file.

## Architecture

- `crates/refract-wasm`: `wasm-bindgen` façade over refract-core, JSON in / JSON out:
  `parse(flavor, pattern)`, `refract(from, pattern, targets, op)`, `equiv(a, b, op)`,
  `dfa(flavor, pattern, op)`, `redos(flavor, pattern, op)` (static prediction only; timing needs
  the engines). Budgets lowered for the browser; runs in a Web Worker so the UI never blocks.
- `web/`: Vite + TypeScript (strict) + Svelte. A layered breadth-first layout renders the
  minimal DFA as SVG (ELK.js was not needed for DFAs of this size, ADR-0014).
- Live JS check: the worker's verdicts for the `js` row are replayed with `new RegExp` in the
  page (labelled with the browser's engine), mirroring oracle/node/runner.mjs.

## Screens

1. **Bench** (home): pattern input with flavour picker and operation toggle (full / search).
   One row ("beam") per engine: colour + icon shape + name (colour-blind safe), verdict
   (SAME proven / SAME ≤N bounded / DIFFERS / REJECTS), the witness with visible invisibles
   (`(newline)`, `(tab)`, `(nbsp)`), which side matches, evidence status.
2. **Automaton lens**: minimal DFA of the pattern (or the product of two), witness path lit.
3. **Stress lab**: ReDoS prediction per engine with attack string; "measure in this browser" for
   JS; CLI command to confirm on all engines.
4. **Translate**: verified translation into every other engine (ADR-0013) with its scope, the
   checks, and the coverage strings replayed live for the JavaScript side.
5. **Spectrum strip** (after spans are reported): your text × engines, match spans. Not built.

## Interaction

- Keyboard first: `/` focuses the pattern, `1`–`5` jump to engine rows, `w` copies the witness.
- Permalinks: state in the URL fragment (`#p=…&f=python&op=search`), never sent to a server.
- Accessibility: axe clean, every colour has a text label and an icon shape, reduced motion.

## Done when (plan §13 P7)

Onboarding < 60 s with the demo `^\w+@\w+\.com$` (the page opens on it: the first verdict is on
screen without typing); Lighthouse ≥ 90; axe clean on every screen in light and dark; WASM
refraction of 5 engines median < 50 ms, p95 < 500 ms. Results: `bench/results/web.md`.
