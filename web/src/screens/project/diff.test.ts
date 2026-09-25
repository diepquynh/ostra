import { describe, expect, it } from "vitest";
import type { DiffHunk } from "../../api/types";
import { changeBlocks } from "./diff";

describe("changeBlocks", () => {
  it("groups replaced lines into one block and marks a deletion on the line before it", () => {
    const hunks: DiffHunk[] = [
      {
        old_start: 1,
        old_lines: 4,
        new_start: 1,
        new_lines: 4,
        header: "",
        lines: [
          { type: "context", text: "a", old_no: 1, new_no: 1 },
          { type: "del", text: "old b", old_no: 2, new_no: null },
          { type: "add", text: "new b", old_no: null, new_no: 2 },
          { type: "add", text: "extra", old_no: null, new_no: 3 },
          { type: "context", text: "c", old_no: 3, new_no: 4 },
          { type: "del", text: "gone", old_no: 4, new_no: null },
        ],
      },
    ];
    const blocks = changeBlocks(hunks);
    expect(blocks.map((b) => [b.kind, b.lines, b.anchor, b.old.map((l) => l.text)])).toEqual([
      ["mod", [2, 3], 3, ["old b"]],
      ["del", [4], 4, ["gone"]],
    ]);
  });
});
