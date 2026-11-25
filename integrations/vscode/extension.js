// @ts-check
// Runs `refract check` on save/open and shows its findings as diagnostics.
const vscode = require("vscode");
const { execFile } = require("node:child_process");
const { args, toDiagnostics } = require("./findings");

const LANGS = new Set(["python", "javascript", "typescript", "javascriptreact", "typescriptreact", "go", "java", "rust"]);

/** @param {vscode.ExtensionContext} context */
function activate(context) {
  const diags = vscode.languages.createDiagnosticCollection("refract");
  const output = vscode.window.createOutputChannel("Refract");
  const severity = {
    error: vscode.DiagnosticSeverity.Error,
    warning: vscode.DiagnosticSeverity.Warning,
    note: vscode.DiagnosticSeverity.Information,
  };

  /** @param {vscode.TextDocument} doc */
  function check(doc) {
    if (!LANGS.has(doc.languageId) || doc.uri.scheme !== "file") return;
    const cfg = vscode.workspace.getConfiguration("refract");
    const bin = cfg.get("path", "refract");
    const a = args(doc.uri.fsPath, { targets: cfg.get("targets", []), redos: cfg.get("redos", true) });
    execFile(bin, a, { timeout: 60_000, maxBuffer: 16 << 20 }, (err, stdout, stderr) => {
      if (err && !stdout) {
        output.appendLine(`refract failed: ${stderr || err.message}`);
        return;
      }
      try {
        const found = toDiagnostics(stdout, (l) => (l < doc.lineCount ? doc.lineAt(l).text : ""));
        diags.set(doc.uri, found.map((/** @type {any} */ d) => {
          const x = new vscode.Diagnostic(new vscode.Range(d.line, d.start, d.line, d.end), d.message,
            severity[/** @type {"error"|"warning"|"note"} */ (d.severity)]);
          x.source = "refract";
          x.code = d.code;
          return x;
        }));
      } catch (e) {
        output.appendLine(`could not read refract output: ${e}`);
      }
    });
  }

  context.subscriptions.push(
    diags,
    output,
    vscode.workspace.onDidOpenTextDocument(check),
    vscode.workspace.onDidSaveTextDocument(check),
    vscode.workspace.onDidCloseTextDocument((d) => diags.delete(d.uri)),
    vscode.commands.registerCommand("refract.checkFile", () => {
      const d = vscode.window.activeTextEditor?.document;
      if (d) check(d);
    }),
  );
  vscode.workspace.textDocuments.forEach(check);
}

function deactivate() {}

module.exports = { activate, deactivate };
