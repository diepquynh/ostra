import { describe, expect, it } from "vitest";
import type { CodeGraph, CodeGraphNode } from "../../api/types";
import { mockCodeGraph, mockCodeSymbols } from "../../api/mock/mockCode";
import { centerOn, clipLabel, fitCamera, inlineMembers, kindStyle, layout, matchFiles, MAX_ZOOM, MIN_ZOOM, nodeHeight, zoomAround } from "./DependencyGraph";

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
  edges: edges.map(([from, to]) => ({ from, to, weight: 1, names: [], import: true, kind: "uses" as const })),
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

  it("wraps a tall column into lanes", () => {
    const many = Array.from({ length: 25 }, (_, i) => node(`d${i}`, 1));
    const g = graph([node("c", 0), ...many], many.map((n) => ["c", n.id] as [string, string]));
    const p = layout(g);
    const xs = new Set(p.nodes.filter((n) => n.column === 1).map((n) => n.x));
    expect(xs.size).toBe(3);
    const c = p.nodes.find((n) => n.id === "c")!;
    expect(Math.min(...xs)).toBeGreaterThan(c.x);
    expect(p.width).toBeGreaterThan(Math.max(...xs));
    expect(p.columns.map((k) => k.column)).toEqual([0, 1]);
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

  it("answers a symbol view centered on the definition", () => {
    const def = mockCodeSymbols("backend", "pay", 5).items.find((s) => s.name === "pay")!;
    const g = mockCodeGraph("backend", { path: def.path, symbol: def.name, line: def.line });
    expect(g.view).toBe("symbol");
    const focus = g.nodes.find((n) => n.id === g.focus)!;
    expect(focus.column).toBe(0);
    expect(focus.symbol?.name).toBe("pay");
    closed(g);
  });
});

describe("graph search", () => {
  it("matches files by every word, basename first", () => {
    const paths = ["src/api/plan.rs", "src/plan/mod.rs", "docs/planning.md", "src/runner.rs"];
    expect(matchFiles(paths, "plan", 10)).toEqual(["src/api/plan.rs", "docs/planning.md", "src/plan/mod.rs"]);
    expect(matchFiles(paths, "src plan", 10)).toEqual(["src/api/plan.rs", "src/plan/mod.rs"]);
    expect(matchFiles(paths, "  ", 10)).toEqual([]);
  });

  it("colors functions and types apart", () => {
    const fn = kindStyle({ kind: "symbol", symbol: { path: "a.rs", name: "f", kind: "function", line: 1, signature: "fn f()", members: [], more_members: 0 } });
    const ty = kindStyle({ kind: "symbol", symbol: { path: "a.rs", name: "T", kind: "class", line: 1, signature: "struct T", members: [], more_members: 0 } });
    expect(fn.icon).toBe("square-function");
    expect(ty.icon).toBe("shapes");
    expect(kindStyle({ kind: "package" }).icon).toBe("box");
  });
});

describe("graph camera", () => {
  it("zooms around the pointer, keeping the point under it still", () => {
    const c = { k: 1, x: 40, y: 10 };
    const z = zoomAround(c, 2, 200, 100);
    // Graph point under (200, 100) before: ((200-40)/1, (100-10)/1) = (160, 90).
    expect(160 * z.k + z.x).toBeCloseTo(200);
    expect(90 * z.k + z.y).toBeCloseTo(100);
    expect(zoomAround(c, 100, 0, 0).k).toBe(MAX_ZOOM);
    expect(zoomAround(c, 0.001, 0, 0).k).toBe(MIN_ZOOM);
  });

  it("centers a point and fits a graph without enlarging it", () => {
    const c = centerOn(50, 20, 1, 800, 400);
    expect(50 * c.k + c.x).toBe(400);
    expect(20 * c.k + c.y).toBe(200);
    expect(fitCamera(400, 200, 800, 600).k).toBe(1);
    const small = fitCamera(4000, 200, 800, 600);
    expect(small.k).toBeLessThan(1);
    expect(2000 * small.k + small.x).toBeCloseTo(400);
  });
});

describe("type members in the graph", () => {
  const member = (name: string, line: number) => ({ path: "a.rs", name, kind: "method" as const, line, via: null, signature: `fn ${name}()` });
  const typeNode = (id: string, n: number, column = 0): CodeGraphNode => ({
    ...node(id, column),
    kind: "symbol",
    symbol: { path: "a.rs", name: id, kind: "class", line: 1, signature: `struct ${id}`, members: Array.from({ length: n }, (_, i) => member(`m${i}`, i + 2)), more_members: 0 },
  });

  it("lists members inside the focused type only, capped with a count", () => {
    const t = typeNode("T", 15);
    expect(inlineMembers(t, "T").rows).toHaveLength(12);
    expect(inlineMembers(t, "T").more).toBe(3);
    expect(inlineMembers(t, "other").rows).toHaveLength(0);
    expect(nodeHeight(t, "T")).toBeGreaterThan(nodeHeight(t, "other"));
  });

  it("stacks a tall focused node without overlapping its column", () => {
    const g: CodeGraph = { ...graph([typeNode("T", 6), node("a", 1), node("b", 1)], [["T", "a"], ["T", "b"]]), focus: "T", view: "symbol" };
    const p = layout(g).nodes;
    const [a, b] = ["a", "b"].map((id) => p.find((n) => n.id === id)!).sort((x, y) => x.y - y.y);
    expect(b.y).toBeGreaterThanOrEqual(a.y + a.h);
    const t = p.find((n) => n.id === "T")!;
    expect(t.h).toBe(nodeHeight(g.nodes[0], "T"));
    expect(layout(g).height).toBeGreaterThanOrEqual(t.y + t.h);
  });
});

describe("node labels", () => {
  it("keeps the member name and cuts the owner from the front", () => {
    expect(clipLabel("UserDeletionEventTransformer.transform", 22)).toBe("…Transformer.transform");
    expect(clipLabel("Planner::explore_components", 22).endsWith("explore_components")).toBe(true);
    expect(clipLabel("Planner::explore_components", 22).length).toBeLessThanOrEqual(22);
    expect(clipLabel("short", 22)).toBe("short");
    expect(clipLabel("a_very_long_function_name_here", 22)).toBe("a_very_long_function_…");
    expect(clipLabel("X.an_extremely_long_method_name_indeed", 22)).toBe("an_extremely_long_met…");
  });
});
