import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CodeFile } from "../../../api/types";
import { changeBlocks } from "../diff";
import { SourceView } from "./SourceView";

afterEach(cleanup);

const file: CodeFile = {
  path: "a.rs",
  provider: "native",
  language: "rust",
  classes: ["keyword", "function", "name"],
  tokens: [1, 0, 2, 0, 1, 3, 4, 1, 2, 4, 4, 2],
  symbols: [],
  imports: [],
};

describe("SourceView", () => {
  it("reports a clicked name and tints its occurrences", () => {
    const onSymbol = vi.fn();
    const { container, rerender } = render(<SourceView code={"fn main() {}\n    main();\n"} file={file} onSymbol={onSymbol} />);
    expect(container.querySelector(".os-tok-k")?.textContent).toBe("fn");
    fireEvent.click(screen.getAllByText("main")[1]);
    expect(onSymbol).toHaveBeenCalledWith({ name: "main", line: 2, col: 4 });
    rerender(<SourceView code={"fn main() {}\n    main();\n"} file={file} onSymbol={onSymbol} selected="main" />);
    expect(container.querySelectorAll(".code-sym--sel")).toHaveLength(2);
  });

  it("colors lines the way CodeView does without tokens", () => {
    const { container } = render(<SourceView code="let x = 1;" file={{ ...file, tokens: [] }} language="ts" highlightLine={1} />);
    expect(container.querySelector(".code-sym")).toBeNull();
    expect(container.querySelector(".os-code__line--hl")).not.toBeNull();
  });

  it("marks changed lines and shows the HEAD text under a clicked mark", () => {
    const hunks = [
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
    ] as const;
    const blocks = changeBlocks(hunks.map((h) => ({ ...h, lines: [...h.lines] })));
    expect(blocks.map((b) => [b.kind, b.lines, b.anchor])).toEqual([
      ["mod", [2, 3], 3],
      ["del", [4], 4],
    ]);
    const { container } = render(<SourceView code={"a\nnew b\nextra\nc\n"} file={null} language="txt" changes={blocks} />);
    expect(container.querySelectorAll(".code-gutter--mod")).toHaveLength(2);
    expect(container.querySelectorAll(".code-gutter--del")).toHaveLength(1);
    fireEvent.click(container.querySelector('[data-mark="2"]')!);
    expect(screen.getByRole("region", { name: "Changes against HEAD" }).textContent).toContain("old b");
    expect(screen.getByText("HEAD line 2")).toBeTruthy();
    fireEvent.click(container.querySelector('[data-mark="4"]')!);
    expect(screen.getByRole("region").textContent).toContain("gone");
    fireEvent.click(screen.getByRole("button", { name: "Close the comparison" }));
    expect(screen.queryByRole("region")).toBeNull();
  });
});
