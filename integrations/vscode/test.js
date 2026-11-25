// Tests the pure helpers against the real CLI (needs `refract` built: REFRACT_BIN or PATH).
const assert = require("node:assert");
const { execFileSync } = require("node:child_process");
const { mkdtempSync, writeFileSync } = require("node:fs");
const { join } = require("node:path");
const { tmpdir } = require("node:os");
const { utf16Column, args, toDiagnostics } = require("./findings");

assert.strictEqual(utf16Column("😀 x = re", 3), 3); // the emoji is 2 code units
assert.strictEqual(utf16Column("abc", 1), 0);

const dir = mkdtempSync(join(tmpdir(), "refract-vscode-"));
const file = join(dir, "a.py");
const src = 'import re\nx = "😀"; R = re.compile(r"^(a+)+$")\n';
writeFileSync(file, src);
const bin = process.env.REFRACT_BIN ?? "refract";
const out = execFileSync(bin, args(file, { targets: ["js"], redos: true }), { encoding: "utf8" });
const lines = src.split("\n");
const d = toDiagnostics(out, (l) => lines[l] ?? "");
const redos = d.find((x) => x.code === "redos");
assert.ok(redos, JSON.stringify(d));
assert.strictEqual(redos.severity, "error");
assert.strictEqual(lines[1].slice(redos.start, redos.start + 2), 'r"');
console.log(`ok: ${d.length} diagnostics`);
