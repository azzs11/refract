# ADR-0002: Unicode tables for the Rust flavour come from regex-syntax

**Context.** `\w`, `\d`, `\s`, `\p{..}` and case folding depend on Unicode tables, and each engine
pins its own Unicode version; version differences are real behaviour differences. Generating
tables per Unicode version (ucd-generate style) is planned, but the bootstrap flavour needs
correct data now.

**Decision.** For the Rust flavour, take class and case-folding data from `regex-syntax` 0.8 — the
crate the real `regex` engine uses, so the data is the engine's by construction. Only data comes
from there: the parser is our own. Property names are validated before lookup. Access goes through
`refract_core::unicode`, so other flavours can plug in their own pinned tables.

**Consequences.** Rust-flavour classes match the real engine exactly (checked by differential
tests). Other flavours must not reuse these tables blindly: Python, Java, JS and .NET each need
their own Unicode version (P3).
