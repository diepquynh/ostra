/**
 * Uploads are user files kept in the session directory. SVG and HTML with script content must
 * never render on the app origin: chips and download buttons save them, raw links are
 * attachments with `application/octet-stream` and nosniff, and the artifact view shows text.
 */
import fs from "node:fs";
import type { Page } from "@playwright/test";
import { audit, domProblems } from "../lib/audit";
import { expect, test } from "../lib/guard";
import { htmlFile, svgFile } from "../lib/payloads";

const cookieOf = (file: string) => JSON.parse(fs.readFileSync(file, "utf8")).cookies[0].value as string;
const dl = (p: string) => `/api/artifacts/download?path=${encodeURIComponent(p)}`;

async function expectDownload(page: Page, act: () => Promise<unknown>, name: string) {
  const [d] = await Promise.all([page.waitForEvent("download", { timeout: 10_000 }), act()]);
  expect(d.suggestedFilename()).toBe(name);
  const body = fs.readFileSync(await d.path());
  return body;
}

test("raw download links are attachments that cannot render", async ({ state }) => {
  const cookie = cookieOf(state.storageState);
  for (const u of state.uploads) {
    const r = await fetch(state.app + dl(u.path), { headers: { cookie: `ostra_session=${cookie}` } });
    expect(r.status, u.name).toBe(200);
    expect(r.headers.get("content-type"), u.name).toBe("application/octet-stream");
    expect(r.headers.get("content-disposition"), u.name).toMatch(/^attachment; filename\*=UTF-8''/);
    expect(r.headers.get("x-content-type-options")).toBe("nosniff");
    expect(r.headers.get("content-security-policy")).toContain("script-src 'self'");
  }
});

test("upload chips, the artifact view, and raw links never render the file", async ({ page, state, guard }) => {
  const svg = state.uploads.find((u) => u.name === "evil.svg")!;
  const html = state.uploads.find((u) => u.name === "evil.html")!;
  await page.goto(`${state.app}/w/${state.ws}/s/${state.session}`);
  const board = page.url();
  for (const u of [svg, html]) {
    const body = await expectDownload(page, () => page.locator("a.ctx-chip__open", { hasText: u.name }).click(), u.name);
    expect(body.toString()).toContain("<script>");
    expect(page.url(), "a chip navigated").toBe(board);
  }

  for (const u of state.uploads) {
    await page.goto(`${state.app}/w/${state.ws}/artifact?path=${encodeURIComponent(u.path)}`);
    await expect(page.locator(".art-toolbar").first()).toBeVisible();
    await page.waitForTimeout(500);
    if (u.name === "evil.md") await audit(page, guard, "PW-MARKER-upload-md", "uploaded Markdown");
    else expect(await domProblems(page), `artifact view of ${u.name}`).toEqual([]);
    if (u.name.startsWith("evil.")) {
      const btn = page.locator(`a[download="${u.name}"]`).first();
      await expectDownload(page, () => btn.click(), u.name);
    }
  }

  // Typed into the address bar, opened in a new tab, or followed from a link: always a download.
  for (const u of [svg, html]) {
    await expectDownload(page, () => page.goto(state.app + dl(u.path)).catch(() => {}), u.name);
    guard.allowPopups = true;
    await expectDownload(
      page,
      () => page.evaluate((href) => window.open(href, "_blank"), state.app + dl(u.path)),
      u.name,
    ).catch(async () => {
      // A download opened in a new tab is reported on that tab.
      const p = guard.popups.at(-1);
      if (p) await p.waitForEvent("download", { timeout: 5000 });
    });
    for (const p of guard.popups.splice(0)) {
      expect(await p.evaluate(() => document.contentType).catch(() => "closed")).not.toMatch(/svg|html/);
      await p.close().catch(() => {});
    }
    guard.allowPopups = false;
  }

  // The JSON view of the file is text, whatever the file holds.
  const r = await page.goto(`${state.app}/api/artifacts?path=${encodeURIComponent(svg.path)}`);
  expect(r?.headers()["content-type"]).toContain("application/json");
  expect(await page.evaluate(() => document.contentType)).toBe("application/json");
  expect(await page.evaluate(() => document.querySelectorAll("svg, script:not([src])").length)).toBe(0);
});

test("files uploaded through the New task box are staged and kept, not rendered", async ({ page, state, guard }) => {
  await page.goto(`${state.app}/w/${state.ws}`);
  await page.getByLabel("Upload files").setInputFiles([
    { name: "ui-evil.svg", mimeType: "image/svg+xml", buffer: Buffer.from(svgFile("ui-upload", state.fake)) },
    { name: "ui-evil.html", mimeType: "text/html", buffer: Buffer.from(htmlFile("ui-upload", state.fake)) },
  ]);
  await expect(page.locator(".ctx-chip--upload", { hasText: "ui-evil.svg" })).toBeVisible();
  await expect(page.locator(".ctx-chip--upload", { hasText: "ui-evil.html" })).toBeVisible();
  await expect(page.locator(".ctx-chip--upload .os-spinner, .ctx-chip--upload [class*=spinner]")).toHaveCount(0);
  expect(await domProblems(page)).toEqual([]);
  const staged = fs.readdirSync(`${state.sessionDir}/../../uploads`, { recursive: true }) as string[];
  expect(staged.some((f) => f.endsWith("ui-evil.svg"))).toBe(true);
  void guard;
});
