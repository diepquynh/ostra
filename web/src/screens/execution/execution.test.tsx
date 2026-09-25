import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as f from "../../api/mock/fixtures";
import { activityFor } from "../../api/mock/fixtures.execution";
import type { ActivityItem, ExecutionView } from "../../api/types";
import { foldActivity } from "../../lib/events";
import { ConsoleContext, type ConsoleContextValue, type Nav } from "../../lib/nav";
import { ExecutionScreen } from "../ExecutionScreen";
import { ActivityStream } from "./ActivityStream";
import {
  diffStat,
  hookRows,
  pendingAsk,
  policyInfo,
  relativize,
  replaceDiff,
  siblingRuns,
  terminalMode,
  toolDiff,
} from "./model";

// xterm.js needs a real layout engine; the stream around it is what these tests cover.
vi.mock("./XtermScreen", () => ({
  default: ({ readOnly }: { readOnly: boolean }) => <div data-testid="xterm" data-readonly={String(readOnly)} />,
}));

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let host: HTMLDivElement | null = null;

async function settle(rounds = 4) {
  for (let i = 0; i < rounds; i++) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, 100));
    });
  }
}

async function render(node: ReactNode) {
  host = document.createElement("div");
  document.body.appendChild(host);
  await act(async () => {
    root = createRoot(host!);
    root.render(node);
  });
  await settle();
}

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
});

function withNav(node: ReactNode, open: Nav["open"] = () => {}) {
  const noop = () => {};
  const ctx: ConsoleContextValue = {
    nav: { ws: f.WS, activeId: null, tabs: [], open, close: noop, keep: noop, href: (id) => id },
    shell: {
      openDock: noop,
      closeDock: noop,
      taskDraft: null,
      setTaskDraft: noop,
      newWorkspace: noop,
      addProject: noop,
      runSetup: noop,
      browseFiles: noop,
      theme: "dark",
      toggleTheme: noop,
    },
    workspace: { detail: null, reload: noop },
  };
  return (
    <MemoryRouter>
      <ConsoleContext.Provider value={ctx}>{node}</ConsoleContext.Provider>
    </MemoryRouter>
  );
}

const text = () => host?.textContent ?? "";
const tabLabels = () =>
  Array.from(host!.querySelectorAll('[aria-label="Execution views"] [role="tab"]')).map((t) =>
    t.textContent?.replace(/\d+$/, ""),
  );
const view = (id: string) => f.executions.find((x) => x.id === id)!;

describe("stream selection", () => {
  it("picks the terminal mode from the PTY and the stored transcript", () => {
    const e = (p: Partial<ExecutionView>) => ({ has_terminal: false, has_transcript: false, ...p });
    expect(terminalMode(e({ has_terminal: true }), "ok")).toBe("live");
    expect(terminalMode(e({}), "running")).toBe("live");
    expect(terminalMode(e({ has_transcript: true }), "ok")).toBe("replay");
    expect(terminalMode(e({ has_transcript: true, has_terminal: true }), "ok")).toBe("live");
    expect(terminalMode(e({}), "cancelled")).toBe("none");
  });

  it("shows the Activity stream for `stream: activity`", async () => {
    await render(withNav(<ExecutionScreen ws={f.WS} id="x_imp2" />));
    expect(tabLabels()).toEqual(["Activity"]);
    expect(host!.querySelector(".ex-activity")).not.toBeNull();
    expect(host!.querySelector('[data-testid="xterm"]')).toBeNull();
  });

  it("shows the Terminal stream and Tool calls for `stream: terminal`, whatever the executor string says", async () => {
    await render(withNav(<ExecutionScreen ws={f.WS} id="x_imp3" />));
    expect(tabLabels()).toEqual(["Terminal", "Tool calls"]);
    expect(host!.querySelector('[data-testid="xterm"]')?.getAttribute("data-readonly")).toBe("false");
    expect(host!.querySelector(".ex-activity")).toBeNull();
    expect(text()).toContain("The harness is paused on a permission ask");
    const tools = host!.querySelectorAll<HTMLElement>('[aria-label="Execution views"] [role="tab"]')[1];
    await act(async () => tools.click());
    expect(text()).toContain("Tool calls seen by the hook bridge");
    expect(text()).toContain("no-tests-from-implementer");
    expect(text()).toContain("Asking you");
  });
});

describe("replay banner", () => {
  it("replays an ended run read-only, then goes live after Resume", async () => {
    await render(withNav(<ExecutionScreen ws={f.WS} id="x_imp1" />));
    expect(text()).toContain("Replaying the ended session. Resume to continue.");
    expect(host!.querySelector('[data-testid="xterm"]')?.getAttribute("data-readonly")).toBe("true");
    const resume = Array.from(host!.querySelectorAll("button")).find((b) => b.textContent === "Resume")!;
    await act(async () => resume.click());
    await settle();
    expect(text()).not.toContain("Replaying the ended session");
    expect(host!.querySelector('[data-testid="xterm"]')?.getAttribute("data-readonly")).toBe("false");
  });
});

describe("sibling runs", () => {
  it("lists the group's runs oldest first with their labels", () => {
    expect(siblingRuns(f.sessionDetail, view("x_imp1"))).toEqual([
      { value: "x_imp1", label: "Phase 1" },
      { value: "x_imp2", label: "Phase 2" },
    ]);
    expect(siblingRuns(null, view("x_imp1"))).toEqual([{ value: "x_imp1", label: "Phase 1" }]);
  });

  it("opens the chosen run from the select", async () => {
    const open = vi.fn();
    await render(withNav(<ExecutionScreen ws={f.WS} id="x_imp2" />, open));
    const select = host!.querySelector<HTMLSelectElement>('select[aria-label="Run"]')!;
    expect(Array.from(select.options).map((o) => o.text)).toEqual(["Phase 1", "Phase 2"]);
    await act(async () => {
      select.value = "x_imp1";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    expect(open).toHaveBeenCalledWith("exec:x_imp1");
  });
});

describe("pending gate", () => {
  it("links a failed run to the gate that names it, from ExecutionView.pending_gate", async () => {
    const open = vi.fn();
    await render(withNav(<ExecutionScreen ws={f.WS} id="x_rc3" />, open));
    expect(text()).toContain("A gate waits on this run");
    expect(text()).toContain("Implementer failed.");
    const button = Array.from(host!.querySelectorAll("button")).find((b) => b.textContent === "Open the gate")!;
    await act(async () => button.click());
    expect(open).toHaveBeenCalledWith(expect.stringMatching(/^session:/), { anchor: "gate-g_rc_failed" });
  });
});

describe("activity items", () => {
  const items: ActivityItem[] = [
    { seq: 1, at: "", delta: { kind: "status", message: "Started on native" } },
    { seq: 2, at: "", delta: { kind: "thinking", text: "Read the controller first." } },
    { seq: 3, at: "", delta: { kind: "text", text: "Adding the `cancel` handler." } },
    {
      seq: 4,
      at: "",
      delta: {
        kind: "tool_call",
        call_id: "e",
        call: { tool: "Edit", input: { file_path: "/r/a.ts", old_string: "a\nb\nc", new_string: "a\nB\nc" } },
      },
    },
    {
      seq: 5,
      at: "",
      delta: {
        kind: "policy",
        call_id: "e",
        decision: { decision: "allow", rule: { layer: "permission", rule: "mode:acceptEdits" } },
      },
    },
    { seq: 6, at: "", delta: { kind: "tool_result", call_id: "e", output: "ok", is_error: false, duration_ms: 5 } },
    {
      seq: 7,
      at: "",
      delta: {
        kind: "tool_call",
        call_id: "w",
        call: { tool: "Write", input: { file_path: "/r/a.test.ts", content: "x" } },
      },
    },
    {
      seq: 8,
      at: "",
      delta: {
        kind: "policy",
        call_id: "w",
        decision: {
          decision: "deny",
          reason: "No tests from the implementer.",
          rule: { layer: "guard", rule: "no-tests-from-implementer" },
        },
      },
    },
    { seq: 9, at: "", delta: { kind: "tool_result", call_id: "w", output: "Denied.", is_error: true, duration_ms: 0 } },
    {
      seq: 10,
      at: "",
      delta: { kind: "tool_call", call_id: "b", call: { tool: "Bash", input: { command: "npm test" } } },
    },
    { seq: 11, at: "", delta: { kind: "tool_output", call_id: "b", chunk: "running 3 tests" } },
  ];

  it("renders each delta kind", async () => {
    const s = foldActivity(items);
    await render(withNav(<ActivityStream entries={s.entries} live />));
    expect(host!.querySelector(".ex-status")?.textContent).toBe("Started on native");
    expect(host!.querySelector(".ex-thinking summary")?.textContent).toBe("Thinking summary");
    expect(host!.querySelector(".ex-text code")?.textContent).toBe("cancel");
    const tools = host!.querySelectorAll(".os-tool");
    expect(tools).toHaveLength(3);
    expect(tools[0].textContent).toContain("/r/a.ts  +1 −1");
    // A denial opens by default and says what to do instead.
    expect(tools[1].classList.contains("os-tool--denied")).toBe(true);
    expect(tools[1].textContent).toContain("Denied by guard rule no-tests-from-implementer");
    expect(tools[1].textContent).toContain("What to do instead: Tests belong to the write-test stage");
    // A call without a result is still running.
    expect(tools[2].querySelector(".os-spinner")).not.toBeNull();
    expect(text()).toContain("Streaming from the agent loop");
  });

  it("opens a row whose denial arrives after the call", async () => {
    const early = foldActivity(items.slice(0, 7));
    await render(withNav(<ActivityStream entries={early.entries} live />));
    expect(host!.querySelectorAll(".os-tool__body")).toHaveLength(0);
    const late = foldActivity(items.slice(0, 8));
    await act(async () => root!.render(withNav(<ActivityStream entries={late.entries} live />)));
    expect(host!.querySelectorAll(".os-tool--denied .os-tool__body")).toHaveLength(1);
  });

  it("renders the mock's native run with inline diffs and a denial", async () => {
    const s = foldActivity(activityFor("x_imp2"));
    await render(
      withNav(
        <ActivityStream
          entries={s.entries}
          live={false}
          summarize={(c) =>
            relativize(String((c.input as { file_path?: string }).file_path ?? c.tool), "/home/me/code/shop-backend")
          }
        />,
      ),
    );
    expect(host!.querySelectorAll(".os-tool--denied")).toHaveLength(1);
    expect(text()).toContain("src/main/java/shop/order/OrderService.java");
    expect(text()).not.toContain("Streaming from the agent loop");
  });
});

describe("model helpers", () => {
  it("diffs an Edit by its shared first and last lines", () => {
    expect(replaceDiff("a\nb\nc", "a\nX\nY\nc")).toEqual([
      { type: "ctx", text: "a" },
      { type: "del", text: "b" },
      { type: "add", text: "X" },
      { type: "add", text: "Y" },
      { type: "ctx", text: "c" },
    ]);
    expect(diffStat(replaceDiff("a", "b"))).toBe("+1 −1");
  });

  it("gives Write and ApplyPatch calls a diff and every other tool none", () => {
    expect(toolDiff({ tool: "Write", input: { file_path: "a", content: "x\ny\n" } })).toEqual([
      { type: "add", text: "x" },
      { type: "add", text: "y" },
    ]);
    expect(
      toolDiff({
        tool: "ApplyPatch",
        input: { patch: "*** Begin Patch\n*** Update File: a.rs\n-old\n+new\n ctx\n*** End Patch" },
      }),
    ).toEqual([
      { type: "ctx", text: "*** Update File: a.rs" },
      { type: "del", text: "old" },
      { type: "add", text: "new" },
      { type: "ctx", text: "ctx" },
    ]);
    expect(toolDiff({ tool: "Read", input: { file_path: "a" } })).toBeNull();
  });

  it("finds a call paused on an ask only while it is the last call", () => {
    const s = foldActivity(activityFor("x_imp3"));
    expect(pendingAsk(s.entries)?.call.tool).toBe("Bash");
    expect(pendingAsk(foldActivity(activityFor("x_imp2")).entries)).toBeNull();
  });

  it("lists hook-bridge decisions with their rule and advice", () => {
    const rows = hookRows(foldActivity(activityFor("x_imp3")).entries, (c) => c.tool);
    expect(rows.map((r) => r.decision)).toEqual(["allow", "allow", "allow", "allow", "deny", "ask"]);
    expect(rows[4].rule).toEqual({ layer: "guard", rule: "no-tests-from-implementer" });
    expect(rows[4].advice).toContain("write-test");
    expect(rows[0].rule).toBeNull();
  });

  it("maps a policy decision onto the design's policy line", () => {
    expect(policyInfo({ decision: "allow", rule: null })).toBeUndefined();
    expect(
      policyInfo({ decision: "deny", reason: "r", rule: { layer: "guard", rule: "build-streak" } })?.advice,
    ).toContain("STUCK");
  });

  it("strips the repo root from summaries", () => {
    const root = "/w/shop-backend";
    expect(root).toBe("/w/shop-backend");
    expect(relativize("/w/shop-backend/src/a.rs in /w/shop-backend/src", root)).toBe("src/a.rs in src");
  });
});
