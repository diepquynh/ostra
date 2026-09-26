import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { mockDiff } from "../../api/mock/projectFiles";
import type { DiffHunk, ProjectTreeEntry } from "../../api/types";
import { changedByLabel } from "../FileScreen";
import { DiffPane } from "./DiffPane";
import { diffRows } from "./diff";
import { kindOf, modifiedLabel, parentsOf, sortEntries, toggleSort } from "./files";

afterEach(cleanup);

const entry = (name: string, extra: Partial<ProjectTreeEntry> = {}): ProjectTreeEntry => ({
  name,
  path: name,
  is_dir: false,
  is_symlink: false,
  size: 0,
  modified: null,
  ignored: false,
  git: null,
  staged: false,
  has_changes: false,
  changed_by: null,
  hidden_from_agents: false,
  ...extra,
});

describe("folder list sorting", () => {
  const rows = [
    entry("b.rs", { size: 300, modified: "2026-09-20T00:00:00Z" }),
    entry("src", { is_dir: true, modified: "2026-09-01T00:00:00Z" }),
    entry("A.md", { size: 50, modified: "2026-09-23T00:00:00Z" }),
    entry(".github", { is_dir: true }),
    entry("c.toml", { size: 1000, modified: "2026-08-01T00:00:00Z" }),
    entry("file10.rs", { size: 1 }),
    entry("file9.rs", { size: 1 }),
  ];
  const names = (s: Parameters<typeof sortEntries>[1]) => sortEntries(rows, s).map((e) => e.name);

  it("puts folders first and sorts names case-insensitively with numbers in order", () => {
    expect(names({ key: "name", dir: 1 })).toEqual([
      ".github",
      "src",
      "A.md",
      "b.rs",
      "c.toml",
      "file9.rs",
      "file10.rs",
    ]);
  });

  it("keeps folders first when descending", () => {
    expect(names({ key: "name", dir: -1 })).toEqual([
      "src",
      ".github",
      "file10.rs",
      "file9.rs",
      "c.toml",
      "b.rs",
      "A.md",
    ]);
  });

  it("sorts by size and breaks ties by name", () => {
    expect(names({ key: "size", dir: 1 })).toEqual([
      ".github",
      "src",
      "file9.rs",
      "file10.rs",
      "A.md",
      "b.rs",
      "c.toml",
    ]);
  });

  it("sorts by modified time and by kind", () => {
    expect(names({ key: "modified", dir: -1 }).slice(2, 5)).toEqual(["A.md", "b.rs", "c.toml"]);
    expect(names({ key: "kind", dir: 1 }).slice(2)).toEqual(["A.md", "b.rs", "file9.rs", "file10.rs", "c.toml"]);
  });

  it("flips the direction on the same column and starts ascending on a new one", () => {
    expect(toggleSort({ key: "name", dir: 1 }, "name")).toEqual({ key: "name", dir: -1 });
    expect(toggleSort({ key: "name", dir: -1 }, "size")).toEqual({ key: "size", dir: 1 });
  });

  it("labels kinds and modified times", () => {
    expect(kindOf(entry("x.tsx"))).toBe("TSX");
    expect(kindOf(entry("Makefile"))).toBe("File");
    expect(kindOf(entry("src", { is_dir: true }))).toBe("Folder");
    const now = new Date(2026, 8, 24, 12).getTime();
    expect(modifiedLabel(new Date(2026, 8, 24, 1).toISOString(), now)).toBe("today");
    expect(modifiedLabel(new Date(2026, 8, 23, 23).toISOString(), now)).toBe("yesterday");
    expect(modifiedLabel(new Date(2026, 8, 20).toISOString(), now)).toBe("4 days ago");
    expect(modifiedLabel(new Date(2026, 8, 10).toISOString(), now)).toBe("2 weeks ago");
    expect(parentsOf("a/b/c.rs")).toEqual(["a", "a/b"]);
  });
});

describe("diff hunks", () => {
  const hunks: DiffHunk[] = [
    {
      old_start: 4,
      old_lines: 3,
      new_start: 4,
      new_lines: 3,
      header: "fn get",
      lines: [
        { type: "context", text: "a", old_no: 4, new_no: 4 },
        { type: "del", text: "b", old_no: 5, new_no: null },
        { type: "add", text: "B", old_no: null, new_no: 5 },
        { type: "context", text: "c", old_no: 6, new_no: 6 },
      ],
    },
    {
      old_start: 20,
      old_lines: 1,
      new_start: 20,
      new_lines: 2,
      header: "",
      lines: [
        { type: "context", text: "x", old_no: 20, new_no: 20 },
        { type: "add", text: "y", old_no: null, new_no: 21 },
      ],
    },
  ];

  it("keeps old and new line numbers and counts the unchanged lines between hunks", () => {
    const rows = diffRows(hunks);
    expect(rows[0]).toEqual({ type: "gap", skipped: 3, header: "fn get" });
    expect(rows.slice(1, 5)).toEqual([
      { type: "ctx", text: "a", old: 4, new: 4 },
      { type: "del", text: "b", old: 5, new: null },
      { type: "add", text: "B", old: null, new: 5 },
      { type: "ctx", text: "c", old: 6, new: 6 },
    ]);
    expect(rows[5]).toEqual({ type: "gap", skipped: 13, header: "" });
    expect(rows).toHaveLength(8);
  });

  it("has no leading gap when the first hunk starts at line 1", () => {
    const rows = diffRows([
      {
        ...hunks[1],
        old_start: 1,
        new_start: 1,
        lines: hunks[1].lines.map((l) => ({ ...l, old_no: l.old_no && 1, new_no: l.new_no && l.new_no - 19 })),
      },
    ]);
    expect(rows[0].type).toBe("ctx");
  });

  it("renders the mock service.rs diff with both line number columns", () => {
    const d = mockDiff("backend", "crates/orders/src/service.rs");
    const { container } = render(<DiffPane hunks={d.hunks} />);
    const lines = [...container.querySelectorAll(".os-diff__line")];
    const del = lines.find((l) => l.classList.contains("os-diff__line--del"))!;
    const add = lines.find((l) => l.classList.contains("os-diff__line--add"))!;
    const nums = (el: Element) => [...el.querySelectorAll("span")].slice(0, 2).map((s) => s.textContent);
    expect(nums(del)).toEqual(["23", ""]);
    expect(nums(add)).toEqual(["", "23"]);
    expect(screen.getByTestId("diff").textContent).toContain("unchanged lines");
    expect(d.added).toBe(7);
    expect(d.removed).toBe(3);
  });

  it("names the execution that changed a file", () => {
    const by = {
      session: "s",
      execution: "x",
      agent: "write-test" as const,
      phase: 3,
      tests: true,
      staged: false,
      running: true,
      at: "",
    };
    expect(changedByLabel(by)).toBe("Write test · Phase 3 tests");
    expect(changedByLabel({ ...by, agent: "implementer", phase: null, tests: false })).toBe("Implementer");
  });
});
