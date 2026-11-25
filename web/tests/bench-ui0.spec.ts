// UI0 (static mock) acceptance: the spec's "Done when it matches this page at 1440, 1024 and
// 390 px wide and passes axe", plus the quality gates that already apply (tokens only, no
// third-party requests, grayscale-distinguishable engines, reduced motion).
import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const WIDTHS = [1440, 1024, 390] as const;
const lane = (page: Page, name: string) => page.getByRole("listitem", { name: new RegExp(`^${name.replace(/[()]/g, "\\$&")}:`) });

async function axe(page: Page, what: string) {
  // @axe-core/playwright's Page type lags @playwright/test's; the object is the same.
  const r = await new AxeBuilder({ page: page as never }).analyze();
  expect(r.violations.map((v) => `${what}: ${v.id}: ${v.help} (${v.nodes.length})`)).toEqual([]);
}

test("the default example: Python and Java match, JavaScript and Go do not", async ({ page }) => {
  await page.goto("./");
  await expect(page.locator("p.summary")).toContainText("Python and Java match; JavaScript and Go do not.");
  await expect(page.locator("li.lane")).toHaveCount(4);
  await expect(lane(page, "Python re")).toContainText("Differs");
  await expect(lane(page, "Java")).toContainText("Differs");
  await expect(lane(page, "JavaScript")).toContainText("Same");
  await expect(lane(page, "Go (RE2)")).toContainText("Same");
  await expect(lane(page, "Go (RE2)")).toContainText("Recorded on Go 1.27.4");
  await expect(page.locator("textarea")).toHaveValue("^\\d+$");
  await expect(page.getByLabel("Search")).toBeChecked();
  await expect(page.locator(".tests").getByRole("img", { name: "newline" })).toBeVisible();
});

test("the witness drawer opens from a bend, a badge or w, and closes with Esc", async ({ page }) => {
  await page.goto("./");
  await page.getByRole("button", { name: "Show the witness where Python re differs" }).click();
  const drawer = page.getByRole("dialog", { name: "Witness" });
  await expect(drawer).toContainText("$ before a final newline");
  await expect(drawer.getByRole("img", { name: "newline" }).first()).toBeVisible();
  await expect(drawer).toContainText("Matches");
  await expect(drawer).toContainText("No match");
  await page.keyboard.press("Escape");
  await expect(drawer).toBeHidden();
  await page.getByRole("button", { name: "Differs: show the witness for Java" }).click();
  await expect(drawer).toBeVisible();
  await page.keyboard.press("Escape");
  await page.locator("body").click({ position: { x: 5, y: 200 } });
  await page.keyboard.press("w");
  await expect(drawer).toBeVisible();
});

test("keys: / focuses the regex, 1–4 toggle lanes and their rays", async ({ page }) => {
  await page.goto("./");
  await page.locator("body").click({ position: { x: 5, y: 200 } });
  await page.keyboard.press("3");
  await expect(page.locator("li.lane")).toHaveCount(3);
  await expect(page.locator(".prism .ray")).toHaveCount(3);
  await page.getByRole("button", { name: /Go \(RE2\)/ }).click();
  await expect(page.locator("li.lane")).toHaveCount(4);
  await page.locator("body").click({ position: { x: 5, y: 200 } });
  await page.keyboard.press("/");
  await expect(page.locator("textarea")).toBeFocused();
});

for (const width of WIDTHS) {
  test(`layout and accessibility at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.goto("./");
    await expect(page.locator("li.lane").first()).toBeVisible();
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
    expect(overflow, "no horizontal scroll").toBe(false);
    // Prism zone beside the lanes at ≥ 1024 px; a header glyph below that.
    await expect(page.locator(".col-prism")).toBeVisible({ visible: width >= 1024 });
    await expect(page.getByRole("button", { name: "Menu" })).toBeVisible({ visible: width < 640 });
    await page.waitForTimeout(1400); // let the beams finish drawing (600 ms + bend)
    await page.screenshot({ path: `test-results/ui0-bench-${width}.png`, fullPage: true });
    await axe(page, `${width}px`);
    await page.keyboard.press("w");
    await expect(page.getByRole("dialog", { name: "Witness" })).toBeVisible();
    await page.screenshot({ path: `test-results/ui0-drawer-${width}.png` });
    await axe(page, `${width}px with the drawer`);
  });
}

test("engines stay distinguishable in grayscale: each has its own pattern and shape", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("./");
  const fills = await page.locator("li.lane svg.shape path").evaluateAll((ps) => ps.map((p) => p.getAttribute("fill")));
  const shapes = await page.locator("li.lane svg.shape path").evaluateAll((ps) => ps.map((p) => p.getAttribute("d")));
  expect(new Set(fills).size).toBe(4);
  expect(new Set(shapes).size).toBe(4);
  await page.addStyleTag({ content: "html { filter: grayscale(1); }" });
  await page.waitForTimeout(1400);
  await page.screenshot({ path: "test-results/ui0-bench-grayscale.png" });
});

test("reduced motion: nothing draws or eases", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("./");
  const d = await page.locator(".ray").first().evaluate((el) => getComputedStyle(el).animationDuration);
  expect(d).toBe("0s");
});

test("no third-party requests (fonts are self-hosted)", async ({ page, baseURL }) => {
  const origins = new Set<string>();
  page.on("request", (r) => origins.add(new URL(r.url()).origin));
  await page.goto("./");
  await expect(page.locator("li.lane").first()).toBeVisible();
  expect([...origins].filter((o) => o !== new URL(baseURL!).origin && !o.startsWith("data:"))).toEqual([]);
  const families = await page.evaluate(() => [...document.fonts].map((f) => f.family));
  expect(families.join(",")).toContain("Inter Variable");
});

test("tokens only: no hard-coded hex colours in components or routes", () => {
  const dirs = ["src/lib/components", "src/routes"];
  const offenders: string[] = [];
  for (const d of dirs)
    for (const f of readdirSync(d)) {
      const text = readFileSync(join(d, f), "utf8");
      const hits = text.match(/#[0-9a-fA-F]{3,8}\b/g);
      if (hits) offenders.push(`${f}: ${hits.join(" ")}`);
    }
  expect(offenders).toEqual([]);
});
