import { describe, expect, it } from "vitest";
import type { SessionStatus, SessionSummary } from "../../api/types";
import { completeTag, filterHome, homeCounts, stageLine, taggedFiles } from "./homeLogic";

const s = (id: string, status: SessionStatus, updated_at: string) =>
  ({ id, status, updated_at, stage_label: "Build" }) as SessionSummary;

describe("home session filters", () => {
  const list = [
    s("a", "running", "2026-01-01"),
    s("b", "waiting", "2026-01-03"),
    s("c", "stalled", "2026-01-02"),
    s("d", "completed", "2026-01-04"),
    s("e", "paused", "2026-01-05"),
  ];

  it("counts each filter, with waiting in both Active and Blocked", () => {
    expect(homeCounts(list)).toEqual({ all: 5, active: 2, blocked: 3, done: 1 });
  });

  it("sorts newest update first", () => {
    expect(filterHome(list, "blocked").map((x) => x.id)).toEqual(["e", "b", "c"]);
  });

  it("puts the blocking state before the stage", () => {
    expect(stageLine(list[1])).toEqual({ text: "Waiting for you · Build", tone: "warn" });
    expect(stageLine(list[0])).toEqual({ text: "Build", tone: null });
  });
});

describe("taggedFiles", () => {
  const known = new Set(["backend/src/main.rs", "backend/src/"]);

  it("marks unknown tags and drops sentence punctuation", () => {
    expect(taggedFiles("See @backend/src/main.rs, and @backend/nope.rs.", ["backend"], known)).toEqual([
      { tag: "backend/src/main.rs", ok: true },
      { tag: "backend/nope.rs", ok: false },
    ]);
  });

  it("keeps folders, skips other projects and duplicates, and trusts tags while loading", () => {
    expect(taggedFiles("@backend/src/ @web/a.ts @backend/src/", ["backend"], known)).toEqual([
      { tag: "backend/src/", ok: true },
    ]);
    expect(taggedFiles("@backend/x.rs", ["backend"], null)).toEqual([{ tag: "backend/x.rs", ok: true }]);
  });
});

describe("completeTag", () => {
  it("replaces the query before the caret and moves the caret past the space", () => {
    const text = "fix @ma please";
    expect(completeTag(text, 7, 4, { project: "backend", path: "src/main.rs" })).toEqual({
      text: "fix @backend/src/main.rs  please",
      caret: 25,
    });
  });
});
