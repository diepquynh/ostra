/**
 * Render paths for untrusted text: agent messages and tool output, Markdown artifacts, the
 * completion report, session titles, repo file names and contents, git refs and commits, MCP
 * metadata, and the quick-question answer. Every string would set `window.__pwned` or request a
 * canary if it ran or loaded; the guard fixture fails the test on either and on CSP violations.
 */
import fs from "node:fs";
import path from "node:path";
import { audit, domProblems } from "../lib/audit";
import { expect, test } from "../lib/guard";
import { TREE_NAMES } from "../lib/scenario";


test("session board, sidebar, and completion report", async ({ page, state, guard }) => {
  await page.goto(`${state.app}/w/${state.ws}/s/${state.session}`);
  await audit(page, guard, "PW-MARKER-completion", "completion report");
  await expect(page.getByText("PW-title").first()).toBeVisible();
  // The document title carries the session title too.
  expect(await page.evaluate(() => typeof (window as any).__pwned)).toBe("undefined");
});

test("every execution's activity, with tool calls expanded", async ({ page, state, guard }) => {
  for (const x of state.executions) {
    await page.goto(`${state.app}/w/${state.ws}/x/${x.id}`);
    await expect(page.getByText("Activity").first()).toBeVisible();
    // Open every tool row, so arguments and results (Bash output, the MCP result) render.
    const rows = page.locator(".ex-activity .os-tool__head[aria-expanded='false'], .ex-activity details > summary");
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
    if (marker) await audit(page, guard, marker, `execution ${x.agent}`);
    else expect(await domProblems(page), `execution ${x.agent}`).toEqual([]);
  }
});

test("MCP tool output and Bash output reach the activity view as text", async ({ page, state }) => {
  const explore = state.executions.find((x) => x.agent === "explore")!;
  await page.goto(`${state.app}/w/${state.ws}/x/${explore.id}`);
  await page.getByText("mcp__pw__echo").first().click();
  await expect(page.getByText("PW-MARKER-mcp-result").first()).toBeVisible();
  expect(await domProblems(page)).toEqual([]);
  const imp = state.executions.find((x) => x.agent === "implementer")!;
  await page.goto(`${state.app}/w/${state.ws}/x/${imp.id}`);
  await page.getByText(/^Bash$/).first().click();
  await expect(page.getByText("PW-MARKER-bash-output").first()).toBeVisible();
  expect(await domProblems(page)).toEqual([]);
});

test("every session artifact, as a document and as Markdown", async ({ page, state, guard }) => {
  const files = (fs.readdirSync(state.sessionDir, { recursive: true }) as string[]).filter((f) => f.endsWith(".md"));
  expect(files.length).toBeGreaterThan(5);
  for (const f of files) {
    const p = path.join(state.sessionDir, f);
    await page.goto(`${state.app}/w/${state.ws}/artifact?path=${encodeURIComponent(p)}`);
    await expect(page.locator(".art-toolbar").first()).toBeVisible();
    await page.waitForTimeout(500);
    // A typed document shows some fields collapsed, so audit on the first marker it shows.
    const markers = [...fs.readFileSync(p, "utf8").matchAll(/PW-MARKER-[\w-]+/g)].map((m) => m[0]);
    let shown: string | undefined;
    for (const m of markers)
      if (await page.getByText(m).first().isVisible()) {
        shown = m;
        break;
      }
    if (shown) await audit(page, guard, shown, `artifact ${f}`);
    else expect(await domProblems(page), `artifact ${f}`).toEqual([]);
    const toggle = page.getByRole("button", { name: "Markdown", exact: true });
    if (await toggle.isVisible().catch(() => false)) {
      await toggle.click();
      if (markers[0]) await audit(page, guard, markers[0], `artifact ${f} as Markdown`);
    }
  }
});

test("repo files in the viewer, and Markdown rendered", async ({ page, state, guard }) => {
  for (const f of ["README.md", "notes.md", "payload.html", "payload.svg", "script.js", ...TREE_NAMES]) {
    await page.goto(`${state.app}/w/${state.ws}/f/app/${encodeURIComponent(f)}`);
    // Clean files open in Monaco, the dirty README in the diff view; both show the text.
    await expect(page.getByText(/__pwned/).first()).toBeVisible();
    await page.waitForTimeout(800);
    expect(await domProblems(page), `file ${f}`).toEqual([]);
    const rendered = page.getByRole("button", { name: "Rendered" });
    if (f.endsWith(".md") && (await rendered.isVisible().catch(() => false))) {
      await rendered.click();
      const marker = f === "README.md" ? "PW-MARKER-readme" : f === "notes.md" ? "PW-MARKER-file-md" : "PW-tree-content";
      await audit(page, guard, marker, `file ${f} rendered`);
    }
  }
});

test("file tree names and the Git dock (branches, commit subjects, changes)", async ({ page, state, guard }) => {
  await page.goto(`${state.app}/w/${state.ws}`);
  await page.getByText("Files", { exact: true }).first().click();
  await expect(page.getByRole("treeitem", { name: /tree-img/ })).toBeVisible();
  await expect(page.getByRole("treeitem", { name: /tree-dir/ })).toBeVisible();
  await page.getByRole("treeitem", { name: /tree-dir/ }).click();
  await expect(page.getByRole("treeitem", { name: "inside.txt" })).toBeVisible();
  expect(await domProblems(page), "file tree").toEqual([]);

  await page.getByText("Git", { exact: true }).first().click();
  await expect(page.getByText("untracked").first()).toBeVisible();
  expect(await domProblems(page), "git changes").toEqual([]);
  await page.getByTitle("Switch or create a branch").click();
  await expect(page.getByText("onerror=window.__pwned='branch'").first()).toBeVisible();
  await expect(page.getByText("PW-commit").first()).toBeVisible();
  expect(await domProblems(page), "branch menu").toEqual([]);
  await page.keyboard.press("Escape");
  void guard;
});

test("MCP server metadata in the workspace settings", async ({ page, state }) => {
  await page.goto(`${state.app}/w/${state.ws}/settings`);
  await page.getByText(/^MCP/).first().click();
  await expect(page.getByText("echo").first()).toBeVisible({ timeout: 30_000 });
  await page.getByText("echo").first().click().catch(() => {});
  await page.waitForTimeout(500);
  expect(await domProblems(page), "mcp settings").toEqual([]);
});

test("the quick question answer and its sources", async ({ page, state, guard }) => {
  await page.goto(`${state.app}/w/${state.ws}`);
  await page.getByRole("button", { name: "Quick question" }).click();
  const box = page.getByPlaceholder("Where is the order status changed?");
  await box.fill("Where is the greeting?");
  await box.press("Control+Enter");
  await audit(page, guard, "PW-MARKER-ask-answer", "quick answer");
});

test("memory lessons, skills, decisions, and search results", async ({ page, state, guard }) => {
  const w = `${state.app}/w/${state.ws}`;
  await page.goto(`${w}/memory`);
  await audit(page, guard, "PW-mem-area", "memory screen");
  await page.getByText("PW-mem-area").first().click().catch(() => {});
  expect(await domProblems(page), "memory lesson").toEqual([]);

  await page.goto(`${w}/skills`);
  await page.getByText("pw-skill").first().click();
  await page.waitForTimeout(800);
  expect(await domProblems(page), "skill").toEqual([]);
  const rendered = page.getByText("PW-MARKER-skill").first();
  if (await rendered.isVisible().catch(() => false)) await audit(page, guard, "PW-MARKER-skill", "skill");

  await page.goto(`${w}/s/${state.session}`);
  await page.getByRole("tab", { name: /Decisions/ }).click();
  await expect(page.getByText("PW-classify-reason").first()).toBeVisible();
  expect(await domProblems(page), "decisions").toEqual([]);

  await page.getByRole("button", { name: /Go to anything/ }).click();
  await page.keyboard.type("pwned");
  await page.waitForTimeout(1500);
  expect(await domProblems(page), "search palette").toEqual([]);
  await page.keyboard.press("Escape");
});
