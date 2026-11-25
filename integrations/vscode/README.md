# Refract for VS Code

Shows `refract check` findings as diagnostics when a Python, JavaScript/TypeScript, Go, Java or
Rust file is opened or saved: patterns the file's engine rejects (error), predicted ReDoS
(exponential = error, polynomial = warning), and — with `refract.targets` — where a pattern means
something else in another engine, with the shortest witness (information). Each diagnostic
carries the command that confirms it on the real engine.

Needs the CLI: `cargo install --git https://github.com/azzs11/refract refract-cli` (or set
`refract.path`). Analysis runs locally; nothing is sent anywhere.

Status: helpers tested against the CLI (`npm test`, `REFRACT_BIN=target/debug/refract`); the
extension is not yet packaged or published.
