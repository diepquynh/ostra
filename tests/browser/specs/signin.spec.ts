/**
 * The one-time sign-in token travels in the URL fragment. After sign-in the address bar must not
 * show it, no request may carry it, and wherever the browser profile recorded it (History,
 * session restore), it must already be spent.
 */
import fs from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { chromium } from "@playwright/test";
import { mintUrl } from "../lib/api";
import { BROWSER, ROOT } from "../lib/env";
import { expect, test } from "../lib/guard";

function filesContaining(dir: string, needle: string): string[] {
  const out: string[] = [];
  const walk = (d: string) => {
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p);
      else if (e.isFile() && fs.statSync(p).size < 50 * 1024 * 1024 && fs.readFileSync(p).includes(needle))
        out.push(path.relative(dir, p));
    }
  };
  walk(dir);
  return out.sort();
}

test("the sign-in token leaves the URL and is spent wherever the profile kept it", async ({ state }) => {
  const profile = path.join(ROOT, "profile-signin");
  fs.rmSync(profile, { recursive: true, force: true });
  const url = mintUrl();
  const token = url.split("#token=")[1];
  expect(token).toMatch(/^[0-9a-f]{32,}$/);

  const ctx = await chromium.launchPersistentContext(profile, {
    ...BROWSER,
    headless: true,
  });
  const page = ctx.pages()[0] ?? (await ctx.newPage());
  const carried: string[] = [];
  page.on("request", async (r) => {
    const h = await r.allHeaders().catch(() => ({}) as Record<string, string>);
    if (r.url().includes(token) || Object.values(h).some((v) => v.includes(token))) carried.push(r.url());
  });
  await page.goto(url);
  await expect(page.getByText(/PW-wsname|Workspaces/).first()).toBeVisible({ timeout: 20_000 });
  expect(page.url(), "the address bar still shows the token").not.toContain("token=");
  expect(await page.evaluate(() => location.hash)).toBe("");
  // The exchange carries it in the body; no URL, Referer, or other header does.
  expect(carried, "a request URL or header carried the token").toEqual([]);
  await page.goto(`${state.app}/w/${state.ws}`);
  await expect(page.getByText("PW-title").first()).toBeVisible();
  await ctx.close();

  const hits = filesContaining(profile, token);
  const db = new DatabaseSync(path.join(profile, "Default", "History"), { readOnly: true });
  const rows = db.prepare("SELECT url FROM urls").all() as { url: string }[];
  db.close();
  const inHistory = rows.filter((r) => r.url.includes(token)).map((r) => r.url.replace(token, "<token>"));
  console.log(`[pw report] profile files holding the token: ${JSON.stringify(hits)}`);
  console.log(`[pw report] History rows with the token: ${JSON.stringify(inHistory)}`);
  test.info().annotations.push({ type: "token-in-profile", description: JSON.stringify({ hits, inHistory }) });

  // Whatever the profile kept is a spent token: replaying it signs nobody in.
  const replay = await fetch(`${state.app}/api/auth/exchange`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ token }),
  });
  expect(replay.status).toBe(401);
  expect(await replay.text()).toContain("already used");
});

test("a sign-in page opened in the background does not spend the token", async ({ browser, state }) => {
  // A second tab that only loads the page (no user) must still exchange once, and a reload of the
  // stripped URL must not try again.
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  const exchanges: number[] = [];
  page.on("response", (r) => {
    if (r.url().endsWith("/api/auth/exchange")) exchanges.push(r.status());
  });
  await page.goto(mintUrl());
  await expect(page.getByText(/PW-wsname|Workspaces/).first()).toBeVisible({ timeout: 20_000 });
  await page.reload();
  await expect(page.getByText(/PW-wsname|Workspaces/).first()).toBeVisible({ timeout: 20_000 });
  expect(exchanges).toEqual([204]);
  expect(page.url()).not.toContain("token=");
  await ctx.close();
});
