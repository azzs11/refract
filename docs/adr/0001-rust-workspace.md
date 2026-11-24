# ADR-0001: Rust workspace with a pure core

**Context.** The core must run natively (CLI), in the browser (WASM, plan §11: analysis never
leaves the user's machine) and from Python (bench, PyO3). It parses untrusted input.

**Decision.** A Cargo workspace: `refract-core` (library, no I/O, `#![forbid(unsafe_code)]`) and
`refract-cli` (clap). Bindings (`refract-wasm`, `refract-py`) and the Docker oracle are added as
separate crates/folders later. Edition 2024, MSRV 1.89. The fuzz crate lives outside the
workspace because cargo-fuzz needs nightly.

**Consequences.** Everything the web app shows is computed by the same code the CLI runs. The core
cannot shell out to real engines; oracles live in the CLI (and later the oracle harness).
