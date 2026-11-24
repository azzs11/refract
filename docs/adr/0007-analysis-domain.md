# ADR-0007: Analysis domains (BMP for JavaScript without `u`)

**Context.** JavaScript without `u`/`v` matches UTF-16 code units: an astral character is two
units, `.` matches one half, and `[😀-😂]` is a "range out of order" error. Refract's alphabet is
Unicode scalar values (ADR-0003), which cannot represent a lone surrogate.

**Decision.** Every `Regex` carries a `Domain`: `Scalars`, or `Bmp` for JS without `u`. Lowering
intersects a `Bmp` language with `[\u{0}-\u{FFFF}]*`. When two patterns are compared, both are
restricted to their *common* domain, so a witness is a valid haystack for both engines. The parser
still models code units exactly (astral source chars become two never-matching units), so syntax
and acceptance are exact. Reports say "BMP haystacks only" whenever the domain is restricted.

**Consequences.** Differences that need an astral character with non-`u` JS (e.g. `/^.$/` on "😀")
are outside the analysed domain and are stated as such, never silently wrong. Exact UTF-16
modelling (inverse homomorphism from scalars to code units) is a recorded follow-up.
