import { describe, expect, it } from "vitest";
import type { SearchHit, TreeSession } from "../api/nav";
import type { ProjectView } from "../api/types";
import { hitToItem, localItems, mergePalette, paletteCommands, paletteTarget } from "./palette";

const session: TreeSession = {
  id: "s1",
  title: "Order cancellation",
  request: "Add order cancellation",
  kind: { kind: "pipeline" },
  status: "waiting",
  open_gates: 1,
  cost_usd: 1.2,
  updated_at: "2026-09-22T10:00:00Z",
  groups: [
    {
      group: "implementer:backend",
      agent: "implementer",
      project: "backend",
      status: "running",
      cost_usd: 0.5,
      runs: [{ id: "x1", run_label: "Phase 2", status: "running", stream: "terminal", summary: null }],
    },
  ],
  artifacts: [{ path: "/s/ostra-spec.md", kind: "spec", label: "Spec", project: null }],
};
const project = { key: "backend", path: "/code/backend" } as ProjectView;
const commands = paletteCommands({ dock: "⌘/" });
const local = localItems([session], [project]);

describe("palette", () => {
  it("lists the design's commands with their ids", () => {
    expect(commands.map((c) => c.label)).toEqual([
      "New task",
      "New workspace…",
      "Add project…",
      "Cost",
      "Settings",
      "Keyboard shortcuts",
      "Ask a quick question",
      "Toggle light and dark theme",
    ]);
    expect(commands.map((c) => c.id)).toEqual([
      "ws:overview",
      "cmd:new-workspace",
      "cmd:add-project",
      "ws:cost",
      "ws:settings",
      "setting:shortcuts",
      "cmd:dock",
      "cmd:theme",
    ]);
  });

  it("offers the keyboard lock only where the browser supports it", () => {
    expect(commands.some((c) => c.id === "cmd:keyboard-lock")).toBe(false);
    expect(paletteCommands({ dock: "⌘/" }, "unlocked").at(-1)?.label).toBe("Lock keyboard shortcuts in fullscreen");
    expect(paletteCommands({ dock: "⌘/" }, "locked").at(-1)?.label).toBe("Release keyboard shortcuts");
  });

  it("shows the user's shortcut on each bound command", () => {
    const hinted = paletteCommands({ dock: "Ctrl+J", theme: "F9", "new-task": "Ctrl+K N" });
    expect(hinted.find((c) => c.id === "cmd:dock")?.hint).toBe("Ctrl+J");
    expect(hinted.find((c) => c.id === "cmd:theme")?.hint).toBe("F9");
    expect(hinted.find((c) => c.id === "ws:overview")?.hint).toBe("Ctrl+K N");
    expect(paletteCommands().find((c) => c.id === "cmd:dock")?.hint).toBeUndefined();
  });

  it("builds local rows for sessions, executions, artifacts and projects", () => {
    expect(local.map((i) => [i.group, i.id])).toEqual([
      ["Sessions", "session:s1"],
      ["Executions", "exec:x1"],
      ["Artifacts", "artifact:/s/ostra-spec.md"],
      ["Projects", "project:backend"],
    ]);
    expect(local[1].label).toBe("Implementer · Phase 2 in backend");
    expect(local[1].icon).toBe("square-terminal");
  });

  it("shows local rows then commands for an empty query", () => {
    const rows = mergePalette("", null, local, commands);
    expect(rows.map((r) => r.id)).toEqual([...local.map((r) => r.id), ...commands.map((c) => c.id)]);
  });

  it("filters local rows while the server has not answered", () => {
    const rows = mergePalette("spec", null, local, commands);
    expect(rows.map((r) => r.id)).toEqual(["artifact:/s/ostra-spec.md"]);
  });

  it("puts server hits first, grouped in a fixed order and by score, then matching commands", () => {
    const hits: SearchHit[] = [
      {
        kind: "setting",
        id: "setting:limits.session_budget_usd",
        label: "limits.session_budget_usd",
        hint: null,
        score: 0.9,
      },
      { kind: "file", id: "file:backend:src/cost.rs", label: "src/cost.rs", hint: "backend", score: 0.5 },
      { kind: "session", id: "session:s2", label: "Cost report", hint: "completed", score: 0.2 },
      { kind: "file", id: "file:backend:cost.md", label: "cost.md", hint: "backend", score: 0.8 },
    ];
    const rows = mergePalette("cost", hits, local, commands);
    expect(rows.map((r) => r.id)).toEqual([
      "session:s2",
      "file:backend:cost.md",
      "file:backend:src/cost.rs",
      "setting:limits.session_budget_usd",
      "ws:cost",
    ]);
    expect(rows.map((r) => r.group)).toEqual(["Sessions", "Files", "Files", "Settings", "Workspace"]);
  });

  it("drops a hit whose id repeats a command", () => {
    const hits: SearchHit[] = [{ kind: "setting", id: "ws:settings", label: "Settings", hint: null, score: 1 }];
    expect(mergePalette("settings", hits, local, commands).filter((r) => r.id === "ws:settings")).toHaveLength(1);
  });

  it("maps hit kinds to groups and icons", () => {
    expect(
      hitToItem({ kind: "lesson", id: "lesson:backend:3", label: "Use the state machine", hint: "orders", score: 1 }),
    ).toEqual({
      id: "lesson:backend:3",
      group: "Lessons",
      icon: "brain",
      label: "Use the state machine",
      hint: "orders",
    });
  });

  it("routes a selection to a command or a resource", () => {
    expect(paletteTarget("cmd:theme")).toEqual({ kind: "command", command: "theme" });
    expect(paletteTarget("lesson:backend:3")).toEqual({ kind: "open", id: "ws:memory", anchor: "lesson:backend:3" });
    expect(paletteTarget("setting:yolo.default")).toEqual({
      kind: "open",
      id: "ws:settings",
      anchor: "setting:yolo.default",
    });
    expect(paletteTarget("file:web:src/a.ts")).toEqual({ kind: "open", id: "file:web:src/a.ts" });
  });
});
