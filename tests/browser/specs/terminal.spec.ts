/**
 * Harness output reaches the browser through xterm.js. The stub harness (a setup login terminal,
 * the same XtermScreen a harness execution uses) prints OSC 52 clipboard writes, OSC 8 links,
 * title changes, an iTerm inline image, and device queries. Nothing may reach the clipboard or
 * the title, links may open only after a confirm and only for http(s), and the browser must not
 * answer any query: the server already does, once, so the PTY input must match a run with no
 * browser attached byte for byte.
 */
import fs from "node:fs";
import { Api } from "../lib/api";
import { expect, test } from "../lib/guard";
import { PTY_GO, PTY_INPUT_LOG } from "../lib/scenario";

const cookieOf = (file: string) => JSON.parse(fs.readFileSync(file, "utf8")).cookies[0].value as string;

async function until(f: () => boolean, ms: number) {
  const end = Date.now() + ms;
  while (!f()) {
    if (Date.now() > end) throw new Error("timed out");
    await new Promise((r) => setTimeout(r, 100));
  }
}

const log = () => (fs.existsSync(PTY_INPUT_LOG) ? fs.readFileSync(PTY_INPUT_LOG) : Buffer.alloc(0));
/** The PTY input, with cursor positions masked: the server's CPR reply names wherever the cursor was. */
const show = (b: Buffer) => JSON.stringify(b.toString("latin1").replace(/\x1b\[(\??)\d+;\d+R/g, "\x1b[$1<row>;<col>R"));

test("terminal escape sequences stay inside the terminal", async ({ page, context, state, guard }) => {
  test.setTimeout(120_000);
  const api = new Api(state.app, cookieOf(state.storageState));

  // Baseline: the same program with no browser attached. What reaches its input is the server's own replies.
  await api.post("/api/harnesses/claude/setup", { action: "login" });
  await new Promise((r) => setTimeout(r, 1000));
  fs.writeFileSync(PTY_GO, "");
  await new Promise((r) => setTimeout(r, 6000));
  const baseline = log();
  console.log(`server-only replies: ${show(baseline)}`);

  await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: state.app });
  await page.goto(`${state.app}/`);
  await page.evaluate(() => navigator.clipboard.writeText("PW-CLIPBOARD-ORIGINAL"));
  const title = await page.title();

  await page.goto(`${state.app}/w/${state.ws}`);
  await page.getByRole("banner").getByRole("button", { name: /PW-wsname/ }).click();
  await page.getByText("Run the setup guide again").click();
  await page.getByRole("button", { name: "Get started" }).click();
  await expect(page.getByText("Harness CLIs")).toBeVisible();
  const login = page.getByRole("button", { name: "Log in" }).first();
  await expect(login).toBeVisible({ timeout: 30_000 });
  await login.click();

  const xterm = page.locator(".ex-xterm");
  await expect(xterm.getByText("PW-TERM-READY")).toBeVisible({ timeout: 20_000 });
  await xterm.click();
  await page.keyboard.type("g");
  await expect(xterm.getByText("PW-TERM-DONE")).toBeVisible({ timeout: 10_000 });
  await expect(xterm.getByText("PW-TERM-EXIT")).toBeVisible({ timeout: 15_000 });
  const withBrowser = log();
  console.log(`replies with a browser: ${show(withBrowser)}`);
  expect(show(withBrowser), "the browser answered a terminal query").toBe(show(baseline));
  expect(withBrowser.includes("\x1b]52")).toBe(false);

  expect(await page.title(), "title sequences changed the document title").toBe(title);
  expect(await page.evaluate(() => navigator.clipboard.readText()), "OSC 52 wrote the clipboard").toBe(
    "PW-CLIPBOARD-ORIGINAL",
  );

  // OSC 8: a javascript: or file: link does nothing; an http link asks first and opens only on OK.
  guard.expectedDialogs.push(/^Open this link from the terminal\?/);
  // xterm's screen layer takes the pointer, so click where the text is drawn.
  const clickText = async (text: string) => {
    const box = await xterm.getByText(text).boundingBox();
    if (!box) throw new Error(`${text} not drawn`);
    await page.mouse.move(box.x + 4, box.y + box.height / 2);
    await page.waitForTimeout(200);
    await page.mouse.click(box.x + 4, box.y + box.height / 2);
    await page.waitForTimeout(300);
  };
  await clickText("PW-JS-LINK");
  await clickText("PW-FILE-LINK");
  expect(guard.dialogs, "a non-http OSC 8 link reached the confirm").toEqual([]);
  await clickText("PW-HTTP-LINK");
  await expect.poll(() => guard.dialogs.length).toBe(1);
  expect(guard.dialogs[0]).toContain(`${state.fake}/canary/osc8`);
  // The dialog was dismissed, so nothing opened and the canary stays silent.
  expect(await page.evaluate(() => document.querySelectorAll(".ex-xterm img, .ex-xterm iframe").length)).toBe(0);
});
