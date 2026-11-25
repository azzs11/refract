# ADR-0014: Web app in WebAssembly, and integrations that reuse the CLI

**Status:** accepted (P7).

**Context.** Plan §11 asks for a browser app where "core analysis runs in WASM, nothing sent to
a server", and §4/§7 for a CI action, an editor extension and an MCP server. Every claim must
stay honest about what was replayed on a real engine (§8).

**Decision.**

1. *Web*: `crates/refract-wasm` exposes `parse`, `refract`, `translate`, `dfa`, `redos` as
   JSON-in/JSON-out functions with a browser budget (5 000 DFA states, 3 s); the page runs them in
   a Web Worker (Vite + Svelte 5 + TypeScript strict). `std::time::Instant` panics on
   `wasm32-unknown-unknown`, so `refract-core` uses `web-time` (re-exports `std` elsewhere).
   Only JavaScript can be replayed in a browser: JS witnesses and the JS side of translations
   are replayed live with the page's own `RegExp`, labelled with the browser's engine; other
   engines point to the CLI command that confirms them. ELK.js was not needed: the minimal DFAs
   shown are small, and a breadth-first layered SVG layout keeps the bundle at 17 kB gzip of JS.
2. *`refract check`*: a per-language lexer (`refract_core::extract`) finds regex literals in call
   shapes that take a pattern, decoding string escapes with the host language's rules, plus
   `refract: <flavor>` directive comments for patterns stored elsewhere. Findings are static and
   labelled *predicted*; each carries the CLI command that confirms it on the real engine.
3. *GitHub Action*: a composite action (`integrations/github-action`) that installs the CLI from
   git (cached) and prints workflow-command annotations (`--format github`).
4. *MCP server*: `refract mcp` speaks JSON-RPC on stdio and serves each tool by running the same
   binary's subcommand with `--json`, so tool results are exactly the CLI's verdicts, including
   real-engine replay, and stdout stays protocol-only.
5. *VS Code*: a dependency-free extension that runs `refract check --format json` on open/save;
   column conversion (scalar values → UTF-16) lives in a pure module tested against the CLI.

**Consequences.** One analysis implementation everywhere (CLI, browser, CI, editor, assistant).
The browser cannot confirm Python/Go/Java/Rust verdicts by itself and says so. The Action's
first run compiles Refract (minutes) until release binaries exist (P8).
