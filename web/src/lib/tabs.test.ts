import { describe, expect, it } from "vitest";
import {
  type CloseScope,
  emptyTabs,
  fromUiTabs,
  normalizeTabs,
  type TabsAction,
  type TabsState,
  tabsReducer,
  toUiTabs,
} from "./tabs";

const run = (actions: TabsAction[], from: TabsState = emptyTabs) => actions.reduce(tabsReducer, from);
const ids = (s: TabsState) => s.tabs.map((t) => (t.preview ? `(${t.id})` : t.pinned ? `*${t.id}` : t.id));

describe("tab model", () => {
  it("opens normal tabs in order and focuses the last one", () => {
    const s = run([
      { type: "open", id: "ws:overview" },
      { type: "open", id: "session:s1" },
    ]);
    expect(ids(s)).toEqual(["ws:overview", "session:s1"]);
    expect(s.active).toBe("session:s1");
  });

  it("opens a tab beside the active one when asked, and leaves an open tab where it is", () => {
    const s = run([
      { type: "open", id: "file:app:a.rs" },
      { type: "open", id: "file:app:b.rs" },
      { type: "open", id: "file:app:c.rs" },
      { type: "activate", id: "file:app:a.rs" },
      { type: "open", id: "file:app:d.rs", beside: true },
      { type: "open", id: "file:app:e.rs", beside: true },
      { type: "open", id: "file:app:c.rs", beside: true },
    ]);
    expect(ids(s)).toEqual(["file:app:a.rs", "file:app:d.rs", "file:app:e.rs", "file:app:b.rs", "file:app:c.rs"]);
    expect(s.active).toBe("file:app:c.rs");
    expect(ids(run([{ type: "open", id: "x:1", beside: true }]))).toEqual(["x:1"]);
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

  it("keeps a preview tab, so the next preview open adds a new tab", () => {
    const s = run([
      { type: "open", id: "exec:x1", preview: true },
      { type: "keep", id: "exec:x1" },
      { type: "open", id: "exec:x2", preview: true },
    ]);
    expect(ids(s)).toEqual(["exec:x1", "(exec:x2)"]);
  });

  it("keeps a preview tab when the same resource is opened normally", () => {
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
    expect(tabsReducer(s, { type: "keep", id: "session:s1" })).toBe(s);
    expect(tabsReducer(s, { type: "setPinned", id: "session:s1", pinned: false })).toBe(s);
    expect(tabsReducer(s, { type: "closeMany", scope: "others", id: "session:s1" })).toBe(s);
    expect(tabsReducer(s, { type: "close", id: "nope:1" })).toBe(s);
  });

  describe("close", () => {
    const three = run([
      { type: "open", id: "a:1" },
      { type: "open", id: "b:2" },
      { type: "open", id: "c:3" },
    ]);

    it("focuses the right neighbour of the closed active tab", () => {
      const s = run(
        [
          { type: "activate", id: "b:2" },
          { type: "close", id: "b:2" },
        ],
        three,
      );
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
    const s = run([
      { type: "open", id: "exec:x1", preview: true },
      { type: "activate", id: "session:s9" },
    ]);
    expect(ids(s)).toEqual(["(exec:x1)", "session:s9"]);
    expect(s.active).toBe("session:s9");
  });

  it("restore drops duplicates and extra previews and repairs active", () => {
    const s = tabsReducer(emptyTabs, {
      type: "restore",
      state: {
        tabs: [
          { id: "a:1", preview: true, pinned: false },
          { id: "a:1", preview: false, pinned: false },
          { id: "b:2", preview: true, pinned: false },
          { id: "c:3", preview: true, pinned: true },
        ],
        active: "gone:1",
      },
    });
    expect(ids(s)).toEqual(["*c:3", "(a:1)", "b:2"]);
    expect(s.active).toBe("c:3");
  });

  describe("pinned tabs", () => {
    const four = run([
      { type: "open", id: "a:1" },
      { type: "open", id: "b:2" },
      { type: "open", id: "c:3", preview: true },
      { type: "open", id: "d:4" },
    ]);

    it("moves a pinned tab to the end of the pinned group and an unpinned one just after it", () => {
      const s = run(
        [
          { type: "setPinned", id: "c:3", pinned: true },
          { type: "setPinned", id: "d:4", pinned: true },
        ],
        four,
      );
      expect(ids(s)).toEqual(["*c:3", "*d:4", "a:1", "b:2"]);
      expect(ids(run([{ type: "setPinned", id: "c:3", pinned: false }], s))).toEqual(["*d:4", "c:3", "a:1", "b:2"]);
    });

    it("opens a tab beside a pinned one after the pinned group", () => {
      const s = run(
        [
          { type: "setPinned", id: "d:4", pinned: true },
          { type: "setPinned", id: "b:2", pinned: true },
          { type: "activate", id: "d:4" },
          { type: "open", id: "e:5", beside: true },
        ],
        four,
      );
      expect(ids(s)).toEqual(["*d:4", "*b:2", "e:5", "a:1", "(c:3)"]);
    });

    it("keeps pinned tabs through every bulk close", () => {
      const pinned = run([{ type: "setPinned", id: "b:2", pinned: true }], four);
      expect(ids(pinned)).toEqual(["*b:2", "a:1", "(c:3)", "d:4"]);
      const close = (scope: CloseScope, id: string) => ids(run([{ type: "closeMany", scope, id }], pinned));
      expect(close("all", "a:1")).toEqual(["*b:2"]);
      expect(close("others", "c:3")).toEqual(["*b:2", "(c:3)"]);
      expect(close("left", "c:3")).toEqual(["*b:2", "(c:3)", "d:4"]);
      expect(close("right", "a:1")).toEqual(["*b:2", "a:1"]);
    });

    it("focuses the tab a bulk close was asked on when the active tab goes", () => {
      const pinned = run([{ type: "setPinned", id: "b:2", pinned: true }], four);
      expect(run([{ type: "closeMany", scope: "others", id: "a:1" }], pinned).active).toBe("a:1");
      expect(run([{ type: "closeMany", scope: "all", id: "a:1" }], pinned).active).toBe("b:2");
      expect(run([{ type: "closeMany", scope: "all", id: "a:1" }], four)).toEqual({ tabs: [], active: null });
      const keep = run([{ type: "activate", id: "b:2" }], pinned);
      expect(run([{ type: "closeMany", scope: "right", id: "a:1" }], keep).active).toBe("b:2");
    });
  });

  it("converts to and from the server's UiTab shape", () => {
    const tabs = [
      { id: "a:1", preview: false, pinned: true },
      { id: "b:2", preview: true, pinned: false },
    ];
    expect(toUiTabs(tabs)).toEqual(tabs);
    expect(fromUiTabs(toUiTabs(tabs))).toEqual(tabs);
    expect(normalizeTabs({ tabs, active: "b:2" }).active).toBe("b:2");
  });
});
