import { describe, expect, it } from "vitest";
import { emptyTabs, fromUiTabs, normalizeTabs, tabsReducer, toUiTabs, type TabsAction, type TabsState } from "./tabs";

const run = (actions: TabsAction[], from: TabsState = emptyTabs) => actions.reduce(tabsReducer, from);
const ids = (s: TabsState) => s.tabs.map((t) => (t.preview ? `(${t.id})` : t.id));

describe("tab model", () => {
  it("opens normal tabs in order and focuses the last one", () => {
    const s = run([
      { type: "open", id: "ws:overview" },
      { type: "open", id: "session:s1" },
    ]);
    expect(ids(s)).toEqual(["ws:overview", "session:s1"]);
    expect(s.active).toBe("session:s1");
  });

  it("reuses the single preview tab for the next preview open", () => {
    const s = run([
      { type: "open", id: "ws:overview" },
      { type: "open", id: "exec:x1", preview: true },
      { type: "open", id: "exec:x2", preview: true },
    ]);
    expect(ids(s)).toEqual(["ws:overview", "(exec:x2)"]);
    expect(s.active).toBe("exec:x2");
  });

  it("keeps the preview slot in place when it is replaced", () => {
    const s = run([
      { type: "open", id: "a:1", preview: true },
      { type: "open", id: "session:b" },
      { type: "open", id: "session:c", preview: true },
    ]);
    expect(ids(s)).toEqual(["(session:c)", "session:b"]);
  });

  it("pins a preview tab, so the next preview open adds a new tab", () => {
    const s = run([
      { type: "open", id: "exec:x1", preview: true },
      { type: "pin", id: "exec:x1" },
      { type: "open", id: "exec:x2", preview: true },
    ]);
    expect(ids(s)).toEqual(["exec:x1", "(exec:x2)"]);
  });

  it("pins a preview tab when the same resource is opened normally", () => {
    const s = run([
      { type: "open", id: "exec:x1", preview: true },
      { type: "open", id: "exec:x1" },
    ]);
    expect(ids(s)).toEqual(["exec:x1"]);
  });

  it("does not demote a normal tab when it is opened as a preview", () => {
    const s = run([
      { type: "open", id: "session:s1" },
      { type: "open", id: "ws:cost" },
      { type: "open", id: "session:s1", preview: true },
    ]);
    expect(ids(s)).toEqual(["session:s1", "ws:cost"]);
    expect(s.active).toBe("session:s1");
  });

  it("returns the same state for a no-op, so effects do not re-run", () => {
    const s = run([{ type: "open", id: "session:s1" }]);
    expect(tabsReducer(s, { type: "open", id: "session:s1" })).toBe(s);
    expect(tabsReducer(s, { type: "activate", id: "session:s1" })).toBe(s);
    expect(tabsReducer(s, { type: "pin", id: "session:s1" })).toBe(s);
    expect(tabsReducer(s, { type: "close", id: "nope:1" })).toBe(s);
  });

  describe("close", () => {
    const three = run([
      { type: "open", id: "a:1" },
      { type: "open", id: "b:2" },
      { type: "open", id: "c:3" },
    ]);

    it("focuses the right neighbour of the closed active tab", () => {
      const s = run([{ type: "activate", id: "b:2" }, { type: "close", id: "b:2" }], three);
      expect(ids(s)).toEqual(["a:1", "c:3"]);
      expect(s.active).toBe("c:3");
    });

    it("focuses the left neighbour when the last tab closes", () => {
      const s = run([{ type: "close", id: "c:3" }], three);
      expect(s.active).toBe("b:2");
    });

    it("keeps the active tab when another tab closes", () => {
      const s = run([{ type: "close", id: "a:1" }], three);
      expect(ids(s)).toEqual(["b:2", "c:3"]);
      expect(s.active).toBe("c:3");
    });

    it("leaves nothing active after the last tab closes", () => {
      const s = run([{ type: "close", id: "a:1" }], run([{ type: "open", id: "a:1" }]));
      expect(s).toEqual({ tabs: [], active: null });
    });
  });

  it("activate adds a missing resource as a normal tab (deep link, history step)", () => {
    const s = run([{ type: "open", id: "exec:x1", preview: true }, { type: "activate", id: "session:s9" }]);
    expect(ids(s)).toEqual(["(exec:x1)", "session:s9"]);
    expect(s.active).toBe("session:s9");
  });

  it("restore drops duplicates and extra previews and repairs active", () => {
    const s = tabsReducer(emptyTabs, {
      type: "restore",
      state: {
        tabs: [
          { id: "a:1", preview: true },
          { id: "a:1", preview: false },
          { id: "b:2", preview: true },
        ],
        active: "gone:1",
      },
    });
    expect(ids(s)).toEqual(["(a:1)", "b:2"]);
    expect(s.active).toBe("a:1");
  });

  it("converts to and from the server's UiTab shape", () => {
    const tabs = [
      { id: "a:1", preview: false },
      { id: "b:2", preview: true },
    ];
    expect(toUiTabs(tabs)).toEqual([
      { id: "a:1", preview: false, pinned: true },
      { id: "b:2", preview: true, pinned: false },
    ]);
    expect(fromUiTabs(toUiTabs(tabs))).toEqual(tabs);
    expect(normalizeTabs({ tabs, active: "b:2" }).active).toBe("b:2");
  });
});
