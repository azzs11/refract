# Architecture decision records

One record per semantic or structural decision (plan §8). Format: context, decision,
consequences. Status values: accepted, superseded (with a link).

| ADR | Title | Status |
|---|---|---|
| [0001](0001-rust-workspace.md) | Rust workspace with a pure core | accepted |
| [0002](0002-unicode-tables-rust-flavour.md) | Unicode tables for the Rust flavour come from regex-syntax | accepted |
| [0003](0003-minterm-alphabet.md) | Automata over minterms of Unicode scalar values | accepted |
| [0004](0004-assertions-before-p4.md) | Assertions: resolve statically or refuse | superseded by 0008 |
| [0005](0005-in-process-rust-oracle.md) | In-process oracle for the Rust flavour | accepted |
| [0006](0006-canonical-witness.md) | Canonical shortest readable witness | accepted |
| [0007](0007-analysis-domain.md) | Exact UTF-16 semantics for JS without `u` | accepted (revised P4) |
| [0010](0010-atomic-and-possessive.md) | Exact atomic groups and possessive quantifiers for prefix-free bodies | accepted |
| [0011](0011-bounded-analysis.md) | Bounded analysis with a backtracking reference interpreter | accepted |
| [0012](0012-redos-lab.md) | ReDoS lab: ambiguity prediction, engine models, measured confirmation | accepted |
| [0013](0013-verified-translation.md) | Verified translation by printing and proving; DFA-coverage tests | accepted |
| [0008](0008-lookaround-construction.md) | Exact look-around by continuations and look-behind context | accepted |
| [0009](0009-engine-measured-tables-and-oracle.md) | Tables measured from each engine; one oracle protocol | accepted |
