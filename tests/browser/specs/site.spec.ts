/**
 * The homepage and docs (site/), served as a static host serves them: the only policy is the CSP meta tag each page
 * carries. Docs render repository Markdown, so a page of test strings goes through the same checks as the console's
 * render paths. The homepage frames the console built with mock data and drives it with messages, which only the
 * site's own origin may send. No page loads anything from another origin.
 */
import type { Page } from "@playwright/test";
import { attackerPage } from "../lib/attacker";
import { audit, domProblems } from "../lib/audit";
import { EVIL_HOST } from "../lib/env";
import { expect, type Guard, test } from "../lib/guard";
import { js } from "../lib/payloads";

// The theme checks start from dark, the design's default.
test.use({ colorScheme: "dark" });

/** Records every request the page makes that is not to `origin` (or a data: URL). */
function foreignRequests(page: Page, origin: string): string[] {
  const out: string[] = [];
  page.on("request", (r) => {
    const u = r.url();
    if (!u.startsWith(`${origin}/`) && !u.startsWith("data:")) out.push(u);
  });
  return out;
}

function onSite(guard: Guard, site: string) {
  guard.origins.push(site);
  guard.cspOrigins.push(site);
}

/** The homepage's one frame is the console shot, from the site itself. Anything else is a problem. */
async function homeProblems(page: Page, site: string) {
  const frames = await page.locator("iframe").evaluateAll((els) => els.map((e) => (e as HTMLIFrameElement).src));
  expect(frames.map((f) => new URL(f).pathname)).toEqual(["/console/index.html"]);
  expect(frames.every((f) => f.startsWith(`${site}/`))).toBe(true);
  return (await domProblems(page)).filter((p) => p !== "<iframe> element");
}

const consoleFrame = (page: Page) => page.frames().find((f) => f.url().includes("/console/index.html"));

test("docs render a page of test strings as text", async ({ page, state, guard }) => {
  onSite(guard, state.site);
  const foreign = foreignRequests(page, state.site);
  await page.goto(`${state.site}/docs/#test-doc`);
  await audit(page, guard, "PW-MARKER-docs", "docs test page", state.site);
  // Images render as links to the file, so none of the payload's images loads.
  expect(await page.locator("article img").count()).toBe(0);
  expect(foreign).toEqual([]);
});

test("every docs page renders with a clean DOM and nothing from another origin", async ({ page, state, guard }) => {
  test.setTimeout(180_000);
  onSite(guard, state.site);
  const foreign = foreignRequests(page, state.site);
  await page.goto(`${state.site}/docs/`);
  const items = page.locator(".docs-nav__item");
  const n = await items.count();
  expect(n).toBeGreaterThan(20);
  for (let i = 0; i < n; i++) {
    const title = await items.nth(i).locator("span").first().innerText();
    await items.nth(i).click();
    await expect(page.locator(".docs-h1")).toHaveText(title);
    expect(await domProblems(page), `docs page ${title}`).toEqual([]);
  }
  expect(foreign).toEqual([]);
});

test("search results and addresses show test strings as text", async ({ page, state, guard }) => {
  onSite(guard, state.site);
  await page.goto(`${state.site}/docs/`);
  await page.getByLabel("Search the docs").fill("onerror");
  const testHit = page.locator(".docs-hit").filter({ hasText: "Test document" });
  await expect(testHit).toBeVisible();
  expect(await domProblems(page)).toEqual([]);
  await testHit.click();
  await expect(page.getByText("PW-MARKER-docs").first()).toBeVisible();
  const evil = encodeURIComponent(`<img src=x onerror="${js("docs-hash")}">`);
  for (const hash of [evil, `engine/${evil}`, `test-doc/${evil}`, "%E0%A4%A", `javascript:${js("docs-hashjs")}`]) {
    await page.goto(`${state.site}/docs/#${hash}`);
    await expect(page.locator(".docs-h1")).toBeVisible();
    expect(await domProblems(page), `hash ${hash}`).toEqual([]);
  }
});

test("the homepage loads only its own files and drives the console shot", async ({ page, state, guard }) => {
  onSite(guard, state.site);
  const foreign = foreignRequests(page, state.site);
  await page.goto(`${state.site}/`);
  await page.locator("iframe").scrollIntoViewIfNeeded();
  await expect.poll(() => consoleFrame(page)?.url() ?? "").toContain("/console/index.html");
  const shot = page.frameLocator("iframe");
  await expect(shot.getByRole("tab", { name: /Order cancellation/ })).toBeVisible();
  await page.getByRole("button", { name: "Initialize web" }).click();
  await expect(shot.getByRole("tab", { name: /Initialize web/, selected: true })).toBeVisible();
  await page.getByRole("button", { name: "Switch to light theme" }).click();
  await expect.poll(() => consoleFrame(page)?.evaluate(() => document.documentElement.dataset.theme)).toBe("light");
  expect(await homeProblems(page, state.site)).toEqual([]);
  expect(await consoleFrame(page)!.evaluate(() => document.querySelectorAll("iframe, object, embed").length)).toBe(0);
  expect(foreign).toEqual([]);
});

test("messages from another origin neither drive the console shot nor receive its reports", async ({
  page,
  state,
  guard,
}) => {
  const attacker = `http://${EVIL_HOST}:${state.attackerPort}`;
  guard.origins.push(attacker);
  guard.cspOrigins.push(state.site);
  const shotUrl = `${state.site}/console/index.html?path=${encodeURIComponent("/w/ws_demo/s/s_refund")}`;
  const drive = JSON.stringify({ type: "ostra-shot", tabs: ["session:s_demo"], open: "session:s_demo", theme: "light" });
  const report = JSON.stringify({ type: "ostra-shot-nav", open: "session:s_demo" });
  const html = `<!doctype html><body>
<iframe id="shot" src="${shotUrl}" width="1100" height="700"></iframe>
<iframe id="home" src="${state.site}/" width="1100" height="700"></iframe>
<script>
  window.got = [];
  addEventListener("message", (e) => window.got.push(JSON.stringify(e.data)));
  const send = () => {
    shot.contentWindow.postMessage(${drive}, "*");
    home.contentWindow.postMessage(${report}, "*");
    home.contentWindow.postMessage({ type: "ostra-shot-theme", theme: "light" }, "*");
  };
  setInterval(send, 300);
</script></body>`;
  await page.goto(`${attacker}${attackerPage("site-shot", html)}`);
  const shot = page.frameLocator("#shot");
  await expect(shot.getByRole("tab", { name: /Refund webhook retries/ })).toBeVisible();
  await page.waitForTimeout(2000);
  await expect(shot.getByRole("tab", { name: /Order cancellation/ })).toHaveCount(0);
  const shotFrame = page.frames().find((f) => f.url().startsWith(shotUrl))!;
  expect(await shotFrame.evaluate(() => document.documentElement.dataset.theme)).not.toBe("light");
  const home = page.frames().find((f) => f.url() === `${state.site}/`)!;
  expect(await home.evaluate(() => document.documentElement.dataset.theme)).not.toBe("light");
  // The shot reports to its own origin only, so the framing page hears nothing.
  expect(await page.evaluate(() => (window as unknown as { got: string[] }).got)).toEqual([]);
});

test("browser storage on the site holds only UI conveniences", async ({ browser, state }) => {
  const ctx = await browser.newContext({ colorScheme: "dark" });
  const page = await ctx.newPage();
  await page.goto(`${state.site}/`);
  await page.locator("iframe").scrollIntoViewIfNeeded();
  await expect(page.frameLocator("iframe").getByRole("tab").first()).toBeVisible();
  await page.getByRole("button", { name: "Switch to light theme" }).click();
  await page.goto(`${state.site}/docs/#test-doc`);
  await page.getByLabel("Search the docs").fill("PW-MARKER");
  await page.waitForTimeout(500);
  const dump = await page.evaluate(async () => ({
    local: { ...localStorage },
    session: { ...sessionStorage },
    databases: ((await indexedDB.databases?.()) ?? []).map((d) => d.name),
    caches: await caches.keys(),
  }));
  expect(await ctx.cookies(state.site)).toEqual([]);
  expect(dump.session).toEqual({});
  expect(dump.databases).toEqual([]);
  expect(dump.caches).toEqual([]);
  expect(dump.local["ostra-home-theme"]).toBe("light");
  for (const [k, v] of Object.entries(dump.local)) {
    expect(k, "a storage key").toMatch(/^ostra[.-]/);
    expect(v, `storage ${k}`).not.toContain("PW-");
    expect(v.length, `storage ${k}`).toBeLessThan(4096);
  }
  await ctx.close();
});
