import { expect, test } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import type { Page } from "@playwright/test";

const row = (page: Page, name: string) =>
  page.locator(".beam").filter({ has: page.locator("strong", { hasText: new RegExp(`^${name}$`) }) });

test("the demo shows where engines bend, confirmed live for JavaScript", async ({ page }) => {
  await page.goto("/");
  const js = row(page, "JavaScript");
  await expect(js).toContainText("differs", { timeout: 20_000 });
  await expect(js.locator(".witness")).toBeVisible();
  await expect(js).toContainText("confirmed live");
  await expect(row(page, "Python")).toContainText("source");
});

test("a syntax error points at the problem", async ({ page }) => {
  await page.goto("/#p=a(b&f=python&op=full");
  await expect(page.getByRole("alert")).toContainText("missing )", { timeout: 20_000 });
});

test("permalink state and operation", async ({ page }) => {
  await page.goto("/#p=%5Eabc&f=js&op=search");
  await expect(page.locator("input:not([type])")).toHaveValue("^abc");
  await expect(page.getByLabel("search")).toBeChecked();
});

test("automaton lens lights the witness path", async ({ page }) => {
  await page.goto("/");
  await row(page, "JavaScript").getByText("see the path").click({ timeout: 20_000 });
  await expect(page.locator(".lens svg")).toBeVisible();
  expect(await page.locator(".edge.lit").count()).toBeGreaterThan(0);
});

test("stress lab predicts and measures in the browser", async ({ page }) => {
  await page.goto("/#p=%5E(a%2B)%2B%24&f=js&op=full");
  await page.getByRole("button", { name: "Stress lab" }).click({ timeout: 20_000 });
  await expect(page.locator(".stress")).toContainText("exponential");
  await page.getByRole("button", { name: /Measure on/ }).click();
  await expect(page.locator(".stress tbody tr").first()).toBeVisible({ timeout: 30_000 });
});

test("translate shows verified translations, confirmed live for JavaScript", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Translate" }).click({ timeout: 20_000 });
  const t = page.locator(".translate");
  const js = t.locator(".beam").filter({ hasText: "JavaScript" });
  await expect(js).toContainText("verified · full match and search");
  await expect(js.locator(".out")).toContainText("\\p{L}");
  await expect(js).toContainText("confirmed live");
  await expect(t.locator(".beam").filter({ hasText: "Go" })).toContainText("full-match only");
});

test("keyboard: / focuses the pattern", async ({ page }) => {
  await page.goto("/");
  await page.locator("body").click();
  await page.keyboard.press("/");
  await expect(page.locator("input:not([type])")).toBeFocused();
});

test("no accessibility violations on any screen, light and dark", async ({ page }) => {
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.goto("/");
    await expect(page.locator(".beam").first()).toBeVisible({ timeout: 20_000 });
    const check = async (screen: string) => {
      const r = await new AxeBuilder({ page }).analyze();
      expect(r.violations.map((v) => `${scheme} ${screen} ${v.id}: ${v.help}`)).toEqual([]);
    };
    await check("bench");
    for (const name of ["Automaton lens", "Stress lab", "Translate"]) {
      await page.getByRole("button", { name }).click();
      await page.waitForTimeout(300);
      await check(name);
    }
  }
});

test("screenshots for review", async ({ page }) => {
  await page.setViewportSize({ width: 1100, height: 900 });
  await page.goto("/");
  await expect(page.locator(".beam").first()).toBeVisible({ timeout: 20_000 });
  await page.screenshot({ path: "test-results/bench.png", fullPage: true });
  await page.getByRole("button", { name: "Translate" }).click();
  await page.waitForTimeout(300);
  await page.screenshot({ path: "test-results/translate.png", fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: "test-results/translate-phone.png", fullPage: true });
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
  expect(overflow).toBe(false);
});

test("refraction of five engines is fast in WebAssembly", async ({ page }) => {
  await page.goto("/");
  const s = page.locator(".status");
  await expect(s).toContainText("ms in WebAssembly", { timeout: 20_000 });
  const ms = Number((await s.textContent())!.match(/(\d+) ms/)![1]);
  expect(ms).toBeLessThan(500);
});
