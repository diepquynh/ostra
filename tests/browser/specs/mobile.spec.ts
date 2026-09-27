/**
 * The render paths of `render.spec.ts` in the phone layout (`web/src/mobile/`), which renders agent and
 * repo text through its own screens: the session board, executions, artifacts, repo files and names, git,
 * skills, memory, the search sheet, and the quick-question sheet.
 */
import fs from "node:fs";
import path from "node:path";
import { audit, domProblems } from "../lib/audit";
import { expect, test } from "../lib/guard";
import { TREE_NAMES } from "../lib/scenario";

test.use({ viewport: { width: 412, height: 880 }, isMobile: true, hasTouch: true });

test("the phone layout replaces the tabbed console", async ({ page, state }) => {
  await page.goto(`${state.app}/w/${state.ws}`);
  await expect(page.locator(".m-shell")).toBeVisible();
  await expect(page.locator(".shell-tabs")).toHaveCount(0);
});

test("session board and completion report", async ({ page, state, guard }) => {
  await page.goto(`${state.app}/w/${state.ws}/s/${state.session}`);
  await audit(page, guard, "PW-MARKER-completion", "mobile completion report");
  await expect(page.getByText("PW-title").first()).toBeVisible();
  for (const tab of ["Executions", "Artifacts", "Decisions", "Event log"]) {
    const t = page.getByRole("tab", { name: new RegExp(`^${tab}`) });
    if (!(await t.isVisible().catch(() => false))) continue;
    await t.click();
    await page.waitForTimeout(300);
    expect(await domProblems(page), `mobile session ${tab}`).toEqual([]);
  }
});

test("every execution's activity, with tool calls expanded", async ({ page, state, guard }) => {
  for (const x of state.executions) {
    await page.goto(`${state.app}/w/${state.ws}/x/${x.id}`);
    await expect(page.locator(".m-shell")).toBeVisible();
    await page.waitForTimeout(800);
    const rows = page.locator(".os-tool__head[aria-expanded='false'], details > summary");
    for (let i = 0, n = await rows.count(); i < Math.min(n, 30); i++)
      await rows
        .nth(i)
        .click({ timeout: 2000 })
        .catch(() => {});
    await page.waitForTimeout(300);
    const marker = {
      explore: "PW-MARKER-explore-mcp-t0",
      "generate-spec": "PW-MARKER-spec-t0",
      plan: "PW-MARKER-plan-t0",
      implementer: "PW-MARKER-implementer-t0",
    }[x.agent];
    if (marker) await audit(page, guard, marker, `mobile execution ${x.agent}`);
    else expect(await domProblems(page), `mobile execution ${x.agent}`).toEqual([]);
  }
});

test("every session artifact, as a document and as Markdown", async ({ page, state, guard }) => {
  const files = (fs.readdirSync(state.sessionDir, { recursive: true }) as string[]).filter((f) => f.endsWith(".md"));
  expect(files.length).toBeGreaterThan(5);
  for (const f of files) {
    const p = path.join(state.sessionDir, f);
    await page.goto(`${state.app}/w/${state.ws}/artifact?path=${encodeURIComponent(p)}`);
    await expect(page.locator(".m-shell")).toBeVisible();
    await page.waitForTimeout(800);
    const markers = [...fs.readFileSync(p, "utf8").matchAll(/PW-MARKER-[\w-]+/g)].map((m) => m[0]);
    let shown: string | undefined;
    for (const m of markers)
      if (await page.getByText(m).first().isVisible()) {
        shown = m;
        break;
      }
    if (shown) await audit(page, guard, shown, `mobile artifact ${f}`);
    else expect(await domProblems(page), `mobile artifact ${f}`).toEqual([]);
    const toggle = page.getByRole("tab", { name: "Markdown" });
    if (await toggle.isVisible().catch(() => false)) {
      await toggle.click();
      if (markers[0]) await audit(page, guard, markers[0], `mobile artifact ${f} as Markdown`);
    }
  }
});

test("repo file names, file contents, and git", async ({ page, state }) => {
  await page.goto(`${state.app}/w/${state.ws}/p/app#files`);
  await expect(page.getByText(/tree-img/).first()).toBeVisible();
  await expect(page.getByText(/tree-dir/).first()).toBeVisible();
  expect(await domProblems(page), "mobile file list").toEqual([]);
  await page.getByText(/tree-dir/).first().click();
  await expect(page.getByText("inside.txt").first()).toBeVisible();
  expect(await domProblems(page), "mobile folder").toEqual([]);

  for (const f of ["README.md", "notes.md", "payload.html", "payload.svg", "script.js", ...TREE_NAMES]) {
    await page.goto(`${state.app}/w/${state.ws}/f/app/${encodeURIComponent(f)}`);
    await expect(page.getByText(/__pwned/).first()).toBeVisible();
    await page.waitForTimeout(500);
    expect(await domProblems(page), `mobile file ${f}`).toEqual([]);
  }

  await page.goto(`${state.app}/w/${state.ws}/p/app#git`);
  await expect(page.getByText("untracked").first()).toBeVisible();
  expect(await domProblems(page), "mobile git changes").toEqual([]);
  await page.getByTitle("Switch or create a branch").click();
  await expect(page.getByText("onerror=window.__pwned='branch'").first()).toBeVisible();
  await expect(page.getByText("PW-commit").first()).toBeVisible();
  expect(await domProblems(page), "mobile git").toEqual([]);
});

test("memory, skills, and the search sheet", async ({ page, state, guard }) => {
  const w = `${state.app}/w/${state.ws}`;
  await page.goto(`${w}/memory`);
  await audit(page, guard, "PW-mem-area", "mobile memory");

  await page.goto(`${w}/skills`);
  await page.getByText("pw-skill").first().click();
  await page.waitForTimeout(800);
  expect(await domProblems(page), "mobile skill").toEqual([]);

  await page.goto(w);
  await page.getByRole("button", { name: "Search", exact: true }).click();
  await page.getByRole("searchbox", { name: "Search" }).fill("pwned");
  await page.waitForTimeout(1500);
  expect(await domProblems(page), "mobile search sheet").toEqual([]);
});

test("the quick question sheet", async ({ page, state, guard }) => {
  await page.goto(`${state.app}/w/${state.ws}`);
  await page.getByRole("button", { name: "Ask a quick question" }).click();
  const box = page.getByRole("textbox", { name: "Question" });
  await box.fill("Where is the greeting?");
  await box.press("Enter");
  await audit(page, guard, "PW-MARKER-ask-answer", "mobile quick answer");
});
