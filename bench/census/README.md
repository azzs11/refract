# Census: real-world regexes across engines (RQ1, RQ2)

1. **Sources** (offline, pinned): the CPython 3.15 standard library, the npm packages installed by
   `web/` and `bench/redos/`, the Go 1.27 standard library (from the `golang:1.27` image), OpenJDK
   `jdk-25-ga` library classes, and the crates in the local cargo registry. Tests, examples and
   benchmarks are excluded.
2. **Extraction**: `refract check --format literals` (the same extractor as `refract check`).
   Literals joined across `+`/adjacent strings; patterns built at run time or with non-constant
   flags are skipped and counted; Python/Java flag arguments become inline flags.
3. **Analysis**: `refract batch` compares each pattern, under the operation its call performs,
   with what a person porting it writes in each other engine (JS literals reduced to their source
   with `i m s` inline; leading `(?ims)` becomes JS flags). Every DIFFERS witness and every
   rejection is replayed on the real engines.

```sh
# go-src/, go-version and jdk-src/ in $SRC:
docker run --rm -v "$SRC":/out golang:1.27 sh -c 'cp -r /usr/local/go/src /out/go-src; go version > /out/go-version'
curl -sSL https://github.com/openjdk/jdk/archive/refs/tags/jdk-25-ga.tar.gz | \
  tar xzf - -C "$SRC/jdk-src" --strip-components=1 'jdk-jdk-25-ga/src/*/share/classes/*'
cargo build --release -p refract-cli
python3 bench/census/collect.py --sources "$SRC"            # corpus.jsonl (local), manifest.json
./target/release/refract batch bench/census/corpus.jsonl > bench/census/results.jsonl
python3 bench/census/analyze.py                              # bench/results/census.{json,md}
```

`corpus.jsonl` and `results.jsonl` stay local (the code belongs to its authors); the manifest
records file, line and a hash per pattern so the corpus can be rebuilt.
