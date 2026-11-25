// Web refraction benchmark (plan §10: median < 50 ms, p95 < 500 ms for a five-engine refraction
// in the browser). Runs only with BENCH=1; writes bench/results/web.json.
import { expect, test } from "@playwright/test";
import { readFileSync, writeFileSync } from "node:fs";

const corpus = JSON.parse(readFileSync("../bench/web/corpus.json", "utf8")) as { patterns: string[] };

test("five-engine refraction latency in WebAssembly", async ({ page, browser }) => {
  test.skip(!process.env.BENCH, "set BENCH=1");
  test.setTimeout(600_000);
  const rows: { pattern: string; ms: number | null; outcome: string }[] = [];
  for (const p of corpus.patterns) {
    // A hash-only change does not reload the app: start from a blank page every time.
    await page.goto("about:blank");
    await page.goto(`/#${new URLSearchParams({ p, f: "python", op: "full" })}`);
    await expect(page.locator("input:not([type])")).toHaveValue(p);
    const status = page.locator(".status, #perr");
    await expect(status.first()).toBeVisible({ timeout: 30_000 });
    const text = (await status.first().textContent()) ?? "";
    const m = text.match(/(\d+) ms in WebAssembly/);
    rows.push({ pattern: p, ms: m ? Number(m[1]) : null, outcome: m ? text.split("·")[0]!.trim() : "error" });
  }
  const ms = rows.flatMap((r) => (r.ms === null ? [] : [r.ms])).sort((a, b) => a - b);
  const q = (x: number) => ms[Math.min(ms.length - 1, Math.floor(x * ms.length))]!;
  const out = {
    browser: `${browser.browserType().name()} ${browser.version()}`,
    patterns: rows.length,
    measured: ms.length,
    median_ms: q(0.5),
    p95_ms: q(0.95),
    max_ms: ms[ms.length - 1],
    rows,
  };
  writeFileSync("../bench/results/web.json", JSON.stringify(out, null, 1) + "\n");
  console.log(`median ${out.median_ms} ms, p95 ${out.p95_ms} ms over ${out.measured}`);
});
