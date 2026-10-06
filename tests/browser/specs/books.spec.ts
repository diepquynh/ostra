/**
 * Documentation books: the Docs view renders a book whose every field carries test strings (global-setup writes it
 * into `.ostra/docs/pw_book/`), draws its Mermaid diagrams under the console's CSP, and exports one HTML file that
 * carries no script, loads nothing, and renders under its own meta CSP.
 */
import fs from "node:fs";
import { audit, domProblems } from "../lib/audit";
import { expect, test } from "../lib/guard";

const EXPORT_ORIGIN = "http://pw-export.localhost";

test("the Docs list and the book reader", async ({ page, state, guard }) => {
  await page.goto(`${state.app}/w/${state.ws}/docs`);
  await expect(page.getByRole("heading", { name: "Documentation" }).first()).toBeVisible();
  await page.getByText("PW-book-title").first().click();
  await expect(page.locator(".bk-bar__title")).toContainText("PW-book-title");
  await audit(page, guard, "PW-MARKER-book-overview", "book overview");

  await page.locator(".docs-nav").getByRole("button", { name: "Glossary", exact: true }).click();
  await audit(page, guard, "PW-MARKER-book-def", "book glossary");

  await page.locator(".docs-nav").getByRole("button", { name: "System architecture", exact: true }).click();
  await audit(page, guard, "PW-MARKER-book-arch", "book architecture");
  await expect(page.locator(".docs-main .md-diagram svg")).toHaveCount(1);

  await page.locator(".docs-nav").getByText("PW-book-title", { exact: false }).first().click();
  await audit(page, guard, "PW-MARKER-book-cell", "book section");
  // Two of each unit's three diagrams parse and draw as SVG text; the third shows its source.
  const drawn = page.locator(".docs-main .md-diagram svg");
  await expect(drawn).toHaveCount(4);
  await expect(drawn.getByText("PW-MERMAID-book").first()).toBeVisible();
  await expect(page.locator(".docs-main .md-pre").filter({ hasText: "mmbad" }).first()).toBeVisible();
  // The code reference's path and symbol stay text in one cell each.
  await expect(page.getByText("app/src/<img src=x onerror=window.__pwned='ref'>.rs").first()).toBeVisible();
  await expect(page.getByText("a`b|c").first()).toBeVisible();

  await page.locator(".docs-nav").getByText("PW-page-title", { exact: false }).first().click();
  await audit(page, guard, "PW-MARKER-page-body", "free book page");
  await expect(page.locator(".docs-main .md-diagram svg").getByText("PW-MERMAID-page").first()).toBeVisible();
  expect(await page.evaluate(() => typeof (window as unknown as { __pwned?: unknown }).__pwned)).toBe("undefined");

  await page.getByLabel("Search this book").fill("PW-MARKER-book-sub-assume");
  await expect(page.locator(".docs-hit").first()).toBeVisible();
  expect(await domProblems(page)).toEqual([]);
});

test("the exported HTML file", async ({ page, context, state, guard }) => {
  await page.goto(`${state.app}/w/${state.ws}/b/pw_book`);
  await expect(page.locator(".bk-bar__title")).toContainText("PW-book-title");
  const [download] = await Promise.all([
    page.waitForEvent("download", { timeout: 60_000 }),
    page.getByRole("button", { name: "Export HTML" }).click(),
  ]);
  expect(download.suggestedFilename()).toBe("pw_book.html");
  const html = fs.readFileSync((await download.path())!, "utf8");

  expect(html).not.toMatch(/<script/i);
  expect(html).not.toMatch(/<iframe|<object|<embed|<foreignObject/i);
  expect(html).not.toMatch(/<[a-z][^>]*\son[a-z]+\s*=/i);
  expect(html).toContain(
    `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data:">`,
  );

  guard.origins.push(EXPORT_ORIGIN);
  guard.cspOrigins.push(EXPORT_ORIGIN);
  await context.route(`${EXPORT_ORIGIN}/**`, (r) => r.fulfill({ status: 200, contentType: "text/html", body: html }));
  guard.allowPopups = true;
  const view = await context.newPage();
  await view.goto(`${EXPORT_ORIGIN}/pw_book.html`);
  // Every page is in the one file, in reading order, with the diagrams already drawn.
  for (const marker of [
    "PW-MARKER-book-overview",
    "PW-MARKER-book-def",
    "PW-MARKER-book-arch",
    "PW-MARKER-book-cell",
    "PW-MARKER-page-body",
  ])
    await expect(view.getByText(marker).first()).toBeVisible();
  await expect(view.locator(".md-diagram svg")).toHaveCount(7);
  await expect(view.locator(".bk-export__toc a")).toHaveCount(5);
  await view.locator(".bk-export__toc a", { hasText: "System architecture" }).click();
  expect(new URL(view.url()).hash).toBe("#architecture");
  expect(await domProblems(view)).toEqual([]);
  expect(await view.evaluate(() => document.scripts.length)).toBe(0);
  expect(await view.evaluate(() => typeof (window as unknown as { __pwned?: unknown }).__pwned)).toBe("undefined");
  await view.close();
});
