// @ts-check
// Pure helpers (no vscode import) so they can be tested with plain node.

/**
 * `refract check --format json` reports 1-based columns in Unicode scalar values; editors count
 * 0-based UTF-16 code units. Converts using the line's text.
 * @param {string} lineText
 * @param {number} col1
 */
function utf16Column(lineText, col1) {
  let units = 0;
  let chars = 0;
  for (const ch of lineText) {
    if (chars === col1 - 1) break;
    units += ch.length;
    chars++;
  }
  return units;
}

/**
 * The CLI arguments for one file.
 * @param {string} file
 * @param {{ targets: string[], redos: boolean }} cfg
 */
function args(file, cfg) {
  const a = ["check", "--format", "json", "--fail-on", "never"];
  if (cfg.targets.length) a.push("--targets", cfg.targets.join(","));
  if (!cfg.redos) a.push("--no-redos");
  a.push("--", file);
  return a;
}

/**
 * Plain diagnostic records from the CLI's JSON (ranges cover the literal's first token).
 * @param {string} stdout
 * @param {(line: number) => string} lineText 0-based line → text
 */
function toDiagnostics(stdout, lineText) {
  const report = JSON.parse(stdout);
  return report.findings.map((/** @type {any} */ f) => {
    const line = f.line - 1;
    const text = lineText(line);
    const start = utf16Column(text, f.column);
    const len = Math.max(1, Math.min(text.length - start, f.pattern.length + 2));
    const confirm = f.confirm ? `\nConfirm on the real engine: ${f.confirm}` : "";
    return {
      line, start, end: start + len,
      severity: f.severity,
      code: f.rule,
      message: `${f.message}${confirm}`,
    };
  });
}

module.exports = { utf16Column, args, toDiagnostics };
