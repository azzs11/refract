"""Refract oracle runner for Python `re` (protocol: oracle/protocol.md)."""

import json
import platform
import re
import sys
import warnings

warnings.simplefilter("ignore")  # FutureWarning for possible nested sets must not pollute stdout


def handle(req):
    if req.get("op") == "version":
        return {"id": req["id"], "ok": True, "engine": "python-re", "version": platform.python_version()}
    try:
        rx = re.compile(req["pattern"])
    except (re.error, OverflowError, RecursionError, ValueError) as e:
        return {"id": req["id"], "ok": False, "error": str(e)}
    op = req.get("op", "full")
    if op == "compile":
        return {"id": req["id"], "ok": True, "results": []}
    run = rx.fullmatch if op == "full" else rx.search
    return {"id": req["id"], "ok": True, "results": [run(s) is not None for s in req.get("inputs", [])]}


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            resp = handle(json.loads(line))
        except Exception as e:  # never die: the harness depends on one response per request
            resp = {"id": None, "ok": False, "error": f"runner: {type(e).__name__}: {e}"}
        sys.stdout.write(json.dumps(resp) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
