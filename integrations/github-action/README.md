# Refract GitHub Action

Annotates pull requests with regex findings from `refract check`: patterns the file's own engine
rejects, predicted ReDoS (Python, JavaScript, Java — Go and Rust are linear), and, with
`targets`, the shortest string on which a pattern means something else in another engine.

```yaml
# .github/workflows/regex.yml
on: [pull_request]
jobs:
  refract:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: azzs11/refract/integrations/github-action@main
        with:
          paths: src
          targets: js,python   # optional: portability notes
          fail-on: warning     # error | warning | note | never
```

Findings are static and labelled *predicted*; each annotation carries the command that confirms
it on the real engine (`refract redos`, `refract equiv`). Patterns built at run time can be
marked with a comment so they are checked too:

```python
# refract: python
EMAIL = r"^[\w.+-]+@[\w-]+\.[\w.]+$"
```

The first run compiles Refract (a few minutes); the binary is cached per `version`.
