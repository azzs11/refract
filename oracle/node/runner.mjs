// Refract oracle runner for JavaScript (V8) RegExp (protocol: oracle/protocol.md).
// The request may carry "flags" (e.g. "u", "v", "i"); default is no flags.

function handle(req) {
  if (req.op === "version") {
    return { id: req.id, ok: true, engine: "v8", version: process.versions.v8, node: process.version, options: process.execArgv };
  }
  const flags = req.flags ?? "";
  let rx;
  try {
    // Full match: sticky forces the start to index 0 and a look-ahead checks the end, both
    // independent of the m flag. (A look-behind for the start is unreliable: with u, V8 tries
    // index 1 inside a surrogate pair, see docs/findings.md.)
    const full = req.op === "full";
    const source = full ? `(?:${req.pattern})(?![\\s\\S])` : req.pattern;
    rx = new RegExp(source, full ? flags.replace("y", "") + "y" : flags);
    // The bare pattern must compile too: wrapping can hide errors such as a trailing `\`.
    new RegExp(req.pattern, flags);
  } catch (e) {
    return { id: req.id, ok: false, error: String(e.message) };
  }
  if (req.op === "compile") return { id: req.id, ok: true, results: [] };
  if (req.op === "time") {
    // Full match: `^(?:p)$` when there is no m flag (a look-ahead wrapper would switch off
    // V8's linear fallback and so change what is measured); else the sticky wrapper.
    const plain = flags.replace("y", "");
    const timed = req.mode !== "full"
      ? new RegExp(req.pattern, flags)
      : plain.includes("m")
        ? new RegExp(`(?:${req.pattern})(?![\\s\\S])`, plain + "y")
        : new RegExp(`^(?:${req.pattern})$`, plain);
    const times = (req.inputs ?? []).map((s) => {
      timed.lastIndex = 0;
      const t0 = process.hrtime.bigint();
      timed.test(s);
      return Number(process.hrtime.bigint() - t0);
    });
    return { id: req.id, ok: true, times };
  }
  return { id: req.id, ok: true, // lastIndex persists across test() calls with g or y: reset it for every input.
    results: (req.inputs ?? []).map((s) => { rx.lastIndex = 0; return rx.test(s); }) };
}

// Split on "\n" only: readline would also split on U+2028/U+2029.
let buffered = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let nl;
  while ((nl = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, nl);
    buffered = buffered.slice(nl + 1);
    if (!line.trim()) continue;
    let resp;
    try {
      resp = handle(JSON.parse(line));
    } catch (e) {
      resp = { id: null, ok: false, error: `runner: ${e}` };
    }
    process.stdout.write(JSON.stringify(resp) + "\n");
  }
});
