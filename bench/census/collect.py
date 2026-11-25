"""Collect the census corpus: every regex literal `refract check` finds in five ecosystems'
library code (tests, examples and benchmarks excluded).

Writes `corpus.jsonl` (patterns; local only, gitignored: the code belongs to its authors) and
`manifest.json` (committed: sources, versions, and per pattern the file, line and a hash).

    python3 bench/census/collect.py --sources DIR   # DIR holds go-src/ and jdk-src/ (README)
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
import sysconfig
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
EXCLUDE = {"test", "tests", "testdata", "testing", "idle_test", "examples", "example", "benches", "bench", "fixtures", "__tests__", "spec"}


def excluded(path: str) -> bool:
    parts = Path(path).parts
    name = parts[-1]
    return (
        any(p in EXCLUDE for p in parts)
        or name.endswith(("_test.go", ".d.ts", ".min.js", "_test.py", ".test.js", ".spec.js", ".test.ts", ".spec.ts"))
        or name.startswith("test_")
    )


def literals(refract: Path, root: Path, flavor: str) -> tuple[list[dict[str, object]], int]:
    out = subprocess.run(
        [str(refract), "check", "--format", "literals", str(root)],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    rows = []
    skipped = 0
    for line in out.splitlines():
        r = json.loads(line)
        r["path"] = str(Path(r["path"]).relative_to(root))
        if r["flavor"] != flavor or excluded(r["path"]):
            continue
        if r["unsupported"]:
            skipped += 1  # built at run time, or flags that are not constants
            continue
        rows.append(r)
    return rows, skipped


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--sources", type=Path, required=True)
    ap.add_argument("--refract", type=Path, default=ROOT / "target" / "release" / "refract")
    args = ap.parse_args()
    src: Path = args.sources
    cargo = Path.home() / ".cargo" / "registry" / "src"
    ecosystems = [
        ("python", Path(sysconfig.get_paths()["stdlib"]), f"CPython {sys.version.split()[0]} standard library"),
        ("js", ROOT / "web" / "node_modules", "npm packages installed by web/package-lock.json"),
        ("js", ROOT / "bench" / "redos" / "node_modules", "npm packages installed by bench/redos/package-lock.json"),
        ("go", src / "go-src", (src / "go-version").read_text().strip() + " standard library"),
        ("java", src / "jdk-src", "OpenJDK jdk-25-ga, src/*/share/classes"),
        ("rust", next(cargo.iterdir()), "crates in the local cargo registry (dependencies of this workspace and its tools)"),
    ]
    corpus: dict[tuple[str, str], dict[str, object]] = {}
    manifest = []
    for flavor, root, what in ecosystems:
        rows, skipped = literals(args.refract, root, flavor)
        unique = 0
        for r in rows:
            key = (flavor, str(r["pattern"]))
            if key not in corpus:
                unique += 1
                corpus[key] = {"id": len(corpus) + 1, "from": flavor, "pattern": r["pattern"], "op": r["operation"],
                               "ecosystem": what, "path": r["path"], "line": r["line"]}
        manifest.append({
            "flavor": flavor,
            "source": what,
            "literals": len(rows),
            "skipped_dynamic": skipped,
            "new_unique_patterns": unique,
            "entries": [
                {"path": r["path"], "line": r["line"], "call": r["call"], "operation": r["operation"],
                 "sha256": hashlib.sha256(str(r["pattern"]).encode()).hexdigest()[:16]}
                for r in rows
            ],
        })
        print(f"{flavor:7} {len(rows):6} literals, {unique:6} new unique, {skipped:5} skipped (run-time)  ({what})", file=sys.stderr)
    with (HERE / "corpus.jsonl").open("w") as f:
        for v in corpus.values():
            f.write(json.dumps(v, ensure_ascii=False) + "\n")
    (HERE / "manifest.json").write_text(json.dumps({"patterns": len(corpus), "ecosystems": manifest}, indent=1) + "\n")
    print(f"{len(corpus)} unique patterns", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
