import { describe, expect, it } from "vitest";
import type { CodeGraph, CodeGraphNode } from "../../api/types";
import { mockCodeGraph } from "../../api/mock/mockCode";
import { layout } from "./DependencyGraph";

const node = (id: string, column: number): CodeGraphNode => ({
  id,
  kind: "file",
  label: id,
  package: "",
  column,
  files: 1,
  test: false,
  dependents: 0,
  dependencies: 0,
});

const graph = (nodes: CodeGraphNode[], edges: [string, string][]): CodeGraph => ({
  view: "file",
  focus: "c",
  nodes,
  edges: edges.map(([from, to]) => ({ from, to, weight: 1, names: [], import: true })),
  indexed_files: nodes.length,
  truncated: false,
});

describe("dependency graph layout", () => {
  it("places columns left to right in the server's order", () => {
    const g = graph([node("c", 0), node("user", -1), node("dep", 1)], [["user", "c"], ["c", "dep"]]);
    const { nodes, width, height } = layout(g);
    const x = (id: string) => nodes.find((n) => n.id === id)!.x;
    expect(x("user")).toBeLessThan(x("c"));
    expect(x("c")).toBeLessThan(x("dep"));
    expect(width).toBeGreaterThan(x("dep"));
    expect(height).toBeGreaterThan(0);
  });

  it("orders a column by its neighbors so links cross less", () => {
    // a1 links to b2 and a2 to b1: the right column flips to follow them.
    const g = graph([node("a1", 0), node("a2", 0), node("b1", 1), node("b2", 1)], [["a1", "b2"], ["a2", "b1"]]);
    const y = (id: string) => layout(g).nodes.find((n) => n.id === id)!.y;
    expect(y("b2") < y("b1")).toBe(y("a1") < y("a2"));
  });

  it("centers a short column against the tallest one", () => {
    const g = graph([node("a", 0), node("b", 0), node("c", 0), node("d", 1)], [["a", "d"]]);
    const p = layout(g).nodes;
    const ys = p.filter((n) => n.column === 0).map((n) => n.y);
    const d = p.find((n) => n.id === "d")!.y;
    expect(d).toBeGreaterThan(Math.min(...ys));
    expect(d).toBeLessThan(Math.max(...ys));
  });
});

describe("mock dependency graph", () => {
  const closed = (g: CodeGraph) => {
    const ids = new Set(g.nodes.map((n) => n.id));
    for (const e of g.edges) {
      expect(ids.has(e.from)).toBe(true);
      expect(ids.has(e.to)).toBe(true);
    }
  };

  it("answers every view with edges between listed nodes", () => {
    const pk = mockCodeGraph("backend", {});
    expect(pk.view).toBe("packages");
    expect(pk.nodes.length).toBeGreaterThan(0);
    closed(pk);
    const first = pk.nodes[0];
    const one = mockCodeGraph("backend", { package: first.package || "." });
    expect(one.view).toBe("package");
    closed(one);
    const file = one.nodes.find((n) => n.kind === "file")!;
    const around = mockCodeGraph("backend", { path: file.id, depth: 2 });
    expect(around.nodes.find((n) => n.id === file.id)!.column).toBe(0);
    closed(around);
  });
});
