#!/usr/bin/env python3
"""ReDoS benchmark: Refract vs recheck vs regexploit, judged by real engines.

    cargo build --release -p refract-cli
    (cd bench/redos && npm ci && uv venv --python 3.12 .venv && uv pip install -r requirements.txt)
    python3 bench/redos/run.py            # writes bench/results/redos.{json,md}

Ground truth, per (pattern, engine): the pattern is **super-linear** on that engine when some
attack (Refract's, recheck's or regexploit's) measures super-linear there, with every attack
timed by the same harness (`refract redos --attack`, timeout per point). A tool's verdict is a
prediction; it is scored against that truth on Python re, V8 and java.util.regex.
"""

from __future__ import annotations

import json
import os
import pathlib
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[1]
REFRACT = os.environ.get("REFRACT", str(ROOT / "target/release/refract"))
ENGINES = ["python", "js", "java"]
TIMEOUT = "2"


def sh(cmd: list[str], timeout: float = 1800) -> str:
    return subprocess.run(cmd, capture_output=True, text=True, timeout=timeout).stdout


def refract(pattern: str, attack: dict | None = None, measure: bool = True) -> dict:
    cmd = [REFRACT, "redos", "-f", "python", "--op", "search", "--engines", ",".join(ENGINES),
           "--json", "--timeout", TIMEOUT]
    if not measure:
        cmd.append("--no-measure")
    if attack:
        cmd += ["--attack", json.dumps(attack)]
    return json.loads(sh(cmd + ["--", pattern]))


def super_linear(measured: str | None) -> bool | None:
    if measured is None:
        return None
    return not measured.startswith("linear")


def recheck(pattern: str) -> dict:
    out = sh(["node", str(HERE / "recheck.mjs"), pattern], timeout=120)
    return json.loads(out) if out.strip() else {"status": "error"}


def regexploit(pattern: str) -> dict:
    out = sh([str(HERE / ".venv/bin/python"), str(HERE / "regexploit_check.py"), pattern], timeout=120)
    return json.loads(out) if out.strip() else {"error": "no output"}


def by_engine(report: dict) -> dict[str, dict]:
    return {e["engine"]: e for e in report["engines"]}


def main() -> int:
    corpus = json.loads((HERE / "corpus.json").read_text())
    rows = []
    t0 = time.time()
    for item in corpus["patterns"]:
        p = item["pattern"]
        own = by_engine(refract(p))
        rc = recheck(p)
        rx = regexploit(p)
        measured: dict[str, dict[str, bool | None]] = {e: {} for e in ENGINES}
        for e in ENGINES:
            measured[e]["refract"] = super_linear(own[e].get("measured"))
        for tool, res in (("recheck", rc), ("regexploit", rx)):
            if res.get("attack"):
                m = by_engine(refract(p, res["attack"]))
                for e in ENGINES:
                    measured[e][tool] = super_linear(m[e].get("measured"))
        row = {
            **item,
            "refract": {e: {"predicted": own[e].get("predicted"), "measured": own[e].get("measured"),
                            "status": own[e].get("status"), "engine": own[e].get("engine_version")}
                        for e in ENGINES},
            "recheck": rc,
            "regexploit": rx,
            "attacks_measured": measured,
            "truth": {e: any(v for v in measured[e].values() if v) for e in ENGINES},
        }
        rows.append(row)
        print(f"[{item['id']:>2}] {p[:40]:<40} truth=" +
              " ".join(f"{e}:{'SL' if row['truth'][e] else 'lin'}" for e in ENGINES), file=sys.stderr)

    def predicted(row: dict, tool: str, engine: str) -> bool | None:
        if tool == "refract":
            g = row["refract"][engine]["predicted"]
            return None if g is None else not g.startswith("linear")
        if tool == "recheck":
            s = row["recheck"].get("status")
            return {"vulnerable": True, "safe": False}.get(s)
        r = row["regexploit"]
        return None if "error" in r else bool(r.get("vulnerable"))

    score = {}
    for tool in ("refract", "recheck", "regexploit"):
        for e in ENGINES:
            tp = fp = fn = tn = unk = 0
            for row in rows:
                pr, t = predicted(row, tool, e), row["truth"][e]
                if pr is None:
                    unk += 1
                elif pr and t:
                    tp += 1
                elif pr and not t:
                    fp += 1
                elif not pr and t:
                    fn += 1
                else:
                    tn += 1
            score[f"{tool}/{e}"] = {
                "tp": tp, "fp": fp, "fn": fn, "tn": tn, "unknown": unk,
                "precision": tp / (tp + fp) if tp + fp else None,
                "recall": tp / (tp + fn) if tp + fn else None,
            }
    engines = {e: rows[0]["refract"][e]["engine"] for e in ENGINES}
    meta = {
        "operation": corpus["operation"], "patterns": len(rows), "timeout_s": float(TIMEOUT),
        "engines": engines, "tools": {"recheck": "4.5.0 (npm)", "regexploit": "1.0.0 (PyPI, Python 3.12)"},
        "refract": sh([REFRACT, "--version"]).strip(), "seconds": round(time.time() - t0),
    }
    res = ROOT / "bench/results"
    res.mkdir(parents=True, exist_ok=True)
    (res / "redos.json").write_text(json.dumps({"meta": meta, "score": score, "rows": rows},
                                               indent=1, ensure_ascii=False) + "\n")
    pct = lambda x: "-" if x is None else f"{100 * x:.1f}%"
    md = ["# ReDoS benchmark results", "",
          "Generated by `bench/redos/run.py`; do not edit by hand. Method: bench/redos/README.md.", "",
          f"- Corpus: {meta['patterns']} patterns (`bench/redos/corpus.json`), operation: search.",
          f"- Engines: " + "; ".join(f"{e}: {v}" for e, v in engines.items()) + ".",
          f"- Tools: Refract ({meta['refract']}), recheck {meta['tools']['recheck']}, "
          f"regexploit {meta['tools']['regexploit']}.",
          f"- Truth: super-linear when any tool's attack measures super-linear on that engine "
          f"(timeout {TIMEOUT} s per point).", "",
          "| tool | engine | TP | FP | FN | TN | no verdict | precision | recall |",
          "|---|---|---:|---:|---:|---:|---:|---:|---:|"]
    for k, v in score.items():
        tool, e = k.split("/")
        md.append(f"| {tool} | {e} | {v['tp']} | {v['fp']} | {v['fn']} | {v['tn']} | {v['unknown']} | "
                  f"{pct(v['precision'])} | {pct(v['recall'])} |")
    confirmed = sum(1 for r in rows for e in ENGINES
                    if r["refract"][e]["status"] == "confirmed" and super_linear(r["refract"][e]["measured"]))
    md += ["", f"Refract verdicts confirmed super-linear on the engine: {confirmed}.", ""]
    (res / "redos.md").write_text("\n".join(md))
    print("\n".join(md))
    return 0


if __name__ == "__main__":
    sys.exit(main())
