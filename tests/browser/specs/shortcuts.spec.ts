/**
 * The web app manifest loads under the server's CSP, and shortcuts set on the Settings tab stay in this
 * browser's storage for one workspace and run in the console.
 */
import { expect, test } from "../lib/guard";

test("the app manifest and its icons are served from the app origin", async ({ page, state }) => {
  await page.goto(`${state.app}/w/${state.ws}`);
  await expect(page.locator("body")).toContainText("PW-", { timeout: 20_000 });
  const href = await page.locator('link[rel="manifest"]').getAttribute("href");
  expect(href).toBe("/manifest.webmanifest");
  const res = await page.request.get(`${state.app}${href}`);
  expect(res.status()).toBe(200);
  expect(res.headers()["content-type"]).toBe("application/manifest+json");
  const manifest = await res.json();
  expect(manifest.start_url).toBe("/");
  for (const icon of manifest.icons as { src: string; type: string }[]) {
    expect(icon.src.startsWith("/")).toBe(true);
    const r = await page.request.get(`${state.app}${icon.src}`);
    expect(r.status(), icon.src).toBe(200);
    expect(r.headers()["content-type"]).toBe(icon.type);
  }
});

test("a recorded shortcut is stored for the workspace and runs", async ({ page, state }) => {
  await page.goto(`${state.app}/w/${state.ws}/settings#setting:shortcuts`);
  await expect(page.getByRole("tab", { name: "Shortcuts" })).toHaveAttribute("aria-selected", "true", {
    timeout: 20_000,
  });
  await expect(page.getByText("cannot override the browser's built-in shortcuts")).toBeVisible();
  await expect(page.getByRole("button", { name: "Install Ostra as an app" })).toBeVisible();

  const row = page.locator("main tbody tr", { hasText: "Toggle light and dark theme" });
  await row.getByRole("button", { name: "Set" }).click();
  await expect(page.getByRole("textbox", { name: /Press the new shortcut/ })).toBeFocused();
  await page.keyboard.press("Alt+KeyY");
  const stored = () =>
    page.evaluate((ws) => JSON.parse(localStorage.getItem(`ostra.shortcuts.${ws}`) ?? "{}").bindings, state.ws);
  await expect.poll(stored).toEqual({ theme: ["alt+y"] });
  await expect(row).toContainText(/Alt|⌥/);

  const theme = () => page.evaluate(() => document.documentElement.dataset.theme);
  const before = await theme();
  await page.locator("main").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("Alt+KeyY");
  await expect.poll(theme).not.toBe(before);
  await page.keyboard.press("Alt+KeyY");
  await expect.poll(theme).toBe(before);
});
