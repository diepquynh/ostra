import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CodeFile } from "../../../api/types";
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
    const { container, rerender } = render(
      <SourceView code={"fn main() {}\n    main();\n"} file={file} onSymbol={onSymbol} />,
    );
    expect(container.querySelector(".os-tok-k")?.textContent).toBe("fn");
    fireEvent.click(screen.getAllByText("main")[1]);
    expect(onSymbol).toHaveBeenCalledWith({ name: "main", line: 2, col: 4 });
    rerender(<SourceView code={"fn main() {}\n    main();\n"} file={file} onSymbol={onSymbol} selected="main" />);
    expect(container.querySelectorAll(".code-sym--sel")).toHaveLength(2);
  });

  it("colors lines the way CodeView does without tokens", () => {
    const { container } = render(
      <SourceView code="let x = 1;" file={{ ...file, tokens: [] }} language="ts" highlightLine={1} />,
    );
    expect(container.querySelector(".code-sym")).toBeNull();
    expect(container.querySelector(".os-code__line--hl")).not.toBeNull();
  });
});
