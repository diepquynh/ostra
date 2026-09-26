/**
 * After a run through the app, browser storage on the app origin holds only UI conveniences: no
 * transcripts or agent text, no sign-in token or session cookie, no provider key.
 */
import fs from "node:fs";
import { mintUrl } from "../lib/api";
import { expect, test } from "../lib/guard";

test("browser storage holds no transcripts, tokens, or keys", async ({ browser, state }) => {
  test.setTimeout(120_000);
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  const url = mintUrl();
  const token = url.split("#token=")[1];
  await page.goto(url);
  await expect(page.getByText(/PW-wsname|Workspaces/).first()).toBeVisible({ timeout: 20_000 });
  const cookie = (await ctx.cookies()).find((c) => c.name === "ostra_session")?.value ?? "";
  expect(cookie).not.toBe("");

  const w = `${state.app}/w/${state.ws}`;
  const artifact = fs.readdirSync(state.sessionDir).find((f) => f.endsWith(".md"))!;
  for (const u of [
    w,
    `${w}/s/${state.session}`,
    ...state.executions.slice(0, 3).map((x) => `${w}/x/${x.id}`),
    `${w}/artifact?path=${encodeURIComponent(`${state.sessionDir}/${artifact}`)}`,
    `${w}/f/app/notes.md`,
    `${w}/settings`,
  ]) {
    await page.goto(u);
    await page.waitForTimeout(1200);
  }
  await page.goto(w);
  await page.getByRole("button", { name: "Quick question" }).click();
  const box = page.getByPlaceholder("Where is the order status changed?");
  await box.fill("Where is the greeting? PW-QUESTION-TEXT");
  await box.press("Control+Enter");
  await expect(page.getByText("PW-MARKER-ask-answer").first()).toBeVisible({ timeout: 30_000 });
  await page.waitForTimeout(1500);

  const dump = await page.evaluate(async () => {
    const out: Record<string, unknown> = {
      local: { ...localStorage },
      session: { ...sessionStorage },
      indexedDB: {} as Record<string, unknown>,
      caches: {} as Record<string, unknown>,
    };
    for (const info of (await indexedDB.databases?.()) ?? []) {
      if (!info.name) continue;
      const db = await new Promise<IDBDatabase>((res, rej) => {
        const r = indexedDB.open(info.name!);
        r.onsuccess = () => res(r.result);
        r.onerror = () => rej(r.error);
      });
      const stores: Record<string, unknown> = {};
      for (const name of Array.from(db.objectStoreNames)) {
        stores[name] = await new Promise((res) => {
          const r = db.transaction(name).objectStore(name).getAll();
          r.onsuccess = () => res(r.result);
          r.onerror = () => res(String(r.error));
        });
      }
      (out.indexedDB as Record<string, unknown>)[info.name] = stores;
      db.close();
    }
    for (const k of await caches.keys()) {
      const c = await caches.open(k);
      const entries: Record<string, string> = {};
      for (const req of await c.keys()) entries[req.url] = (await (await c.match(req))?.text())?.slice(0, 2000) ?? "";
      (out.caches as Record<string, unknown>)[k] = entries;
    }
    return out;
  });
  const text = JSON.stringify(dump);
  console.log(
    `[pw report] storage keys: local=${JSON.stringify(Object.keys(dump.local as object))} session=${JSON.stringify(Object.keys(dump.session as object))} idb=${JSON.stringify(Object.keys(dump.indexedDB as object))} caches=${JSON.stringify(Object.keys(dump.caches as object))} (${text.length} bytes)`,
  );
  for (const [what, needle] of [
    ["the sign-in token", token],
    ["the session cookie", cookie],
    ["the provider key", "sk-ant-fake"],
    ["agent text", "PW-MARKER"],
    ["the question", "PW-QUESTION-TEXT"],
    ["test strings", "__pwned"],
  ] as const)
    expect(text.includes(needle), `browser storage holds ${what}`).toBe(false);
  expect(text, "a 64-hex secret is stored").not.toMatch(/[0-9a-f]{64}/);
  await ctx.close();
});
