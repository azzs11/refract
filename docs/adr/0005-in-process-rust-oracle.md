# ADR-0005: In-process oracle for the Rust flavour

**Context.** Plan rule: no user-visible "differs" without replay on the real engine. The Docker
oracle for eight engines is P3 work.

**Decision.** For the Rust flavour the real engine is a Rust library, so the CLI links the `regex`
crate and replays every witness in-process: search = `Regex::is_match`, full match =
`\A(?:pattern)\z`. Verdicts are labelled `confirmed` (both sides replayed and agreed),
`predicted` (some side has no oracle) or `contradicted` (exit code 3: a Refract bug). The test
suite uses the same crate for large-scale agreement testing (`tests/rust_oracle.rs`).

**Consequences.** Rust-flavour verdicts are confirmed today at zero cost. The `oracle::run`
signature is the interface the Docker harness will implement for the other engines. Known gap:
an `x`-mode pattern ending in a `#` comment cannot be wrapped for full-match replay; the replay
then reports an engine error and the verdict stays `predicted`.
