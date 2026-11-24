# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow SemVer.

## [Unreleased]

### Added
- **P0 — foundation.** Cargo workspace (`refract-core`, `refract-cli`), canonical Unicode
  character-set algebra, span-carrying errors, analysis budgets, Unicode data provider,
  flavour-independent IR, semantics profiles, and a hand-written Rust-flavour (`regex` 1.x)
  parser, differentially tested against `regex-syntax`. `refract parse`, `refract flavors`.
  cargo-fuzz target for the parser. Specs: `spec/ir.md`, `spec/flavours/rust.md`.
- **P1 — two automaton engines.** Lowering from IR to regular terms per operation (full match,
  search) with static anchor resolution; minterm alphabets; Thompson NFA + subset construction;
  symbolic Brzozowski derivatives with intersection and complement; Hopcroft and Brzozowski
  minimisation; canonical DFA forms. Double-construction and algebraic-law property tests.
  `refract dfa` (table or Graphviz).
- **P2 — equivalence and witnesses.** Product-automaton equivalence with the canonical shortest
  readable witness (lazy on derivatives, eager on Thompson DFAs); in-process real-engine replay for
  the Rust flavour with `confirmed` / `predicted` / `contradicted` labels; `refract equiv` with
  text and JSON output; golden CLI snapshots. Spec: `spec/witness.md`. ADRs 0001–0006.

### Fixed
- `[^]--]`: after a leading literal `]`, `--` is the difference operator, not two literal dashes
  (found by the real-engine language test).
