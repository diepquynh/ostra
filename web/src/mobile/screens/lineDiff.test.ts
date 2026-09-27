import { describe, expect, it } from "vitest";
import { unifiedDiff } from "./lineDiff";
import { siblingRunsWithStatus } from "./MExecution";

const text = (lines: { type: string; text: string }[]) =>
  lines.map((l) => `${l.type === "add" ? "+" : l.type === "del" ? "-" : " "}${l.text}`);

describe("unifiedDiff", () => {
  it("marks a changed line inside unchanged ones", () => {
    expect(text(unifiedDiff("a\nb\nc\n", "a\nB\nc\n"))).toEqual([" a", "-b", "+B", " c"]);
  });

  it("finds insertions between kept lines", () => {
    expect(text(unifiedDiff("a\nc", "a\nb\nc\nd"))).toEqual([" a", "+b", " c", "+d"]);
  });

  it("collapses unchanged runs beyond the context", () => {
    const before = Array.from({ length: 20 }, (_, i) => `l${i}`).join("\n");
    const after = before.replace("l10", "L10");
    expect(text(unifiedDiff(before, after, 2))).toEqual([
      " … 8 unchanged lines",
      " l8",
      " l9",
      "-l10",
      "+L10",
      " l11",
      " l12",
      " … 7 unchanged lines",
    ]);
  });

  it("shows a new file as additions and a deleted one as deletions", () => {
    expect(text(unifiedDiff("", "x\ny"))).toEqual(["+x", "+y"]);
    expect(text(unifiedDiff("x\ny", ""))).toEqual(["-x", "-y"]);
  });
});

describe("siblingRunsWithStatus", () => {
  it("lists the group's runs with their status, and the open run's live status", () => {
    const detail = {
      execution_groups: [{ group: "implementer:backend", executions: ["x1", "x2"] }],
      executions: [
        { id: "x1", run_label: "Phase 1", status: "ok" },
        { id: "x2", run_label: "Phase 2", status: "ok" },
      ],
    } as unknown as Parameters<typeof siblingRunsWithStatus>[0];
    expect(
      siblingRunsWithStatus(detail, {
        id: "x2",
        group: "implementer:backend",
        run_label: "Phase 2",
        status: "running",
      }),
    ).toEqual([
      { id: "x1", label: "Phase 1", status: "ok" },
      { id: "x2", label: "Phase 2", status: "running" },
    ]);
  });

  it("falls back to the run itself without a session", () => {
    expect(siblingRunsWithStatus(null, { id: "x", group: "g", run_label: "Run 1", status: "ok" })).toEqual([
      { id: "x", label: "Run 1", status: "ok" },
    ]);
  });
});
