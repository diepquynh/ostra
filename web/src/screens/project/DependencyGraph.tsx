import { useMemo, useState } from "react";
import { api } from "../../api";
import type { CodeGraph, CodeGraphEdge, CodeGraphNode, CodeReindex } from "../../api/types";
import { Banner, Button, Chip, Icon, Input, Spinner, Tabs } from "../../design";
import { useAsync } from "../../lib/hooks";
import { useFileIndex } from "../../lib/live";
import { useNav } from "../../lib/nav";
import { fileId } from "../../lib/resource";

/** What the graph shows: every package, one package's files, or one file's neighborhood. */
export type GraphView = { kind: "packages" } | { kind: "package"; pkg: string } | { kind: "file"; path: string; depth: number };

const NODE_W = 184;
const NODE_H = 30;
const COL_GAP = 60;
const ROW_GAP = 12;
const PAD = 12;

export interface Placed extends CodeGraphNode {
  x: number;
  y: number;
}

/**
 * Columns left to right as the server numbered them (a node uses nodes to its right), each column
 * ordered by the average row of its neighbors so links cross less. Pure, for tests.
 */
export function layout(g: CodeGraph): { nodes: Placed[]; width: number; height: number } {
  const cols = [...new Set(g.nodes.map((n) => n.column))].sort((a, b) => a - b);
  const byCol = cols.map((c) => g.nodes.filter((n) => n.column === c).sort((a, b) => a.label.localeCompare(b.label)));
  const near = new Map<string, string[]>();
  for (const e of g.edges) {
    near.set(e.from, [...(near.get(e.from) ?? []), e.to]);
    near.set(e.to, [...(near.get(e.to) ?? []), e.from]);
  }
  const row = new Map<string, number>();
  byCol.forEach((c) => c.forEach((n, i) => row.set(n.id, i)));
  const sweep = (order: number[]) => {
    for (const ci of order) {
      const score = (n: CodeGraphNode) => {
        const r = (near.get(n.id) ?? []).map((m) => row.get(m)).filter((v): v is number => v !== undefined);
        return r.length ? r.reduce((a, b) => a + b, 0) / r.length : (row.get(n.id) ?? 0);
      };
      byCol[ci].sort((a, b) => score(a) - score(b));
      byCol[ci].forEach((n, i) => row.set(n.id, i));
    }
  };
  const idx = byCol.map((_, i) => i);
  sweep(idx);
  sweep([...idx].reverse());
  const tallest = Math.max(1, ...byCol.map((c) => c.length));
  const colHeight = (k: number) => k * NODE_H + (k - 1) * ROW_GAP;
  const height = PAD * 2 + colHeight(tallest);
  const nodes = byCol.flatMap((c, ci) => {
    const top = PAD + (colHeight(tallest) - colHeight(c.length)) / 2;
    return c.map((n, i) => ({ ...n, x: PAD + ci * (NODE_W + COL_GAP), y: top + i * (NODE_H + ROW_GAP) }));
  });
  return { nodes, width: PAD * 2 + cols.length * NODE_W + Math.max(0, cols.length - 1) * COL_GAP, height };
}

function edgePath(a: Placed, b: Placed): string {
  const ay = a.y + NODE_H / 2;
  const by = b.y + NODE_H / 2;
  if (b.x > a.x) {
    const x1 = a.x + NODE_W;
    const mid = (x1 + b.x) / 2;
    return `M${x1},${ay} C${mid},${ay} ${mid},${by} ${b.x},${by}`;
  }
  // Same column or leftwards: a loop, drawn around the right side.
  const x = Math.max(a.x, b.x) + NODE_W;
  const bulge = x + 40;
  return `M${a.x + NODE_W},${ay} C${bulge},${ay} ${bulge},${by} ${b.x + NODE_W},${by}`;
}

const clip = (s: string, n = 23) => (s.length > n ? `${s.slice(0, n - 1)}…` : s);

/** The project's dependency graph: packages, a package's files, or one file with what uses it and what it uses. */
export function DependencyGraph({ ws, projectKey }: { ws: string; projectKey: string }) {
  const nav = useNav();
  const [view, setView] = useState<GraphView>({ kind: "packages" });
  const [selected, setSelected] = useState<string | null>(null);
  const [hover, setHover] = useState<string | null>(null);
  const [find, setFind] = useState("");
  const [rebuilt, setRebuilt] = useState<{ ok: CodeReindex } | { error: string } | null>(null);
  const [rebuilding, setRebuilding] = useState(false);
  const [generation, setGeneration] = useState(0);
  const index = useFileIndex(ws, projectKey);

  const graph = useAsync(() => {
    const at =
      view.kind === "packages"
        ? {}
        : view.kind === "package"
          ? { package: view.pkg || "." }
          : { path: view.path, depth: view.depth };
    return api.codeGraph(ws, projectKey, at);
  }, [ws, projectKey, JSON.stringify(view), generation]);

  const placed = useMemo(() => (graph.data ? layout(graph.data) : null), [graph.data]);
  const byId = useMemo(() => new Map((placed?.nodes ?? []).map((n) => [n.id, n])), [placed]);

  const go = (v: GraphView) => {
    setView(v);
    setSelected(null);
  };
  const primary = (n: CodeGraphNode) => {
    if (n.kind === "package") go({ kind: "package", pkg: n.package });
    else go({ kind: "file", path: n.id, depth: view.kind === "file" ? view.depth : 1 });
  };
  const rebuild = () => {
    setRebuilding(true);
    setRebuilt(null);
    api.codeReindex(ws, projectKey).then(
      (ok) => {
        setRebuilt({ ok });
        setGeneration((g) => g + 1);
      },
      (e: Error) => setRebuilt({ error: e.message }),
    ).finally(() => setRebuilding(false));
  };

  const focusId = hover ?? selected;
  const touches = (e: CodeGraphEdge) => focusId !== null && (e.from === focusId || e.to === focusId);
  const crumbs: { label: string; view: GraphView }[] = [{ label: "Packages", view: { kind: "packages" } }];
  if (view.kind === "package") crumbs.push({ label: view.pkg || "(project top)", view });
  if (view.kind === "file") {
    const pkg = graph.data?.nodes.find((n) => n.id === view.path)?.package;
    if (pkg !== undefined) crumbs.push({ label: pkg || "(project top)", view: { kind: "package", pkg } });
    crumbs.push({ label: view.path, view });
  }
  const sel = selected ? byId.get(selected) : undefined;
  const selEdges = sel && graph.data ? graph.data.edges.filter((e) => e.from === sel.id || e.to === sel.id) : [];

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 10, padding: "0 24px 24px", minHeight: 0 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
        <nav aria-label="Graph view" style={{ display: "flex", alignItems: "center", gap: 4, font: "var(--type-ui)", minWidth: 0, flexWrap: "wrap" }}>
          {crumbs.map((c, i) => (
            <span key={i} style={{ display: "flex", alignItems: "center", gap: 4 }}>
              {i > 0 && <Icon name="chevron-right" size={12} style={{ color: "var(--text-muted)" }} />}
              {i < crumbs.length - 1 ? (
                <button type="button" onClick={() => go(c.view)} style={{ background: "none", border: 0, padding: 0, color: "var(--text-link)", cursor: "pointer", font: "inherit" }}>
                  {c.label}
                </button>
              ) : (
                <span style={{ fontFamily: "var(--font-mono)", wordBreak: "break-all" }}>{c.label}</span>
              )}
            </span>
          ))}
        </nav>
        <div style={{ flex: 1 }} />
        {view.kind === "file" && (
          <Tabs
            variant="segmented"
            label="Hops shown"
            tabs={[1, 2, 3].map((d) => ({ id: String(d), label: `${d} hop${d > 1 ? "s" : ""}` }))}
            value={String(view.depth)}
            onChange={(d) => go({ ...view, depth: Number(d) })}
          />
        )}
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (find.trim()) go({ kind: "file", path: find.trim(), depth: 1 });
          }}
          style={{ display: "flex", gap: 6 }}
        >
          <Input
            size="sm"
            mono
            icon="search"
            list={`graph-files-${projectKey}`}
            placeholder="Find a file"
            aria-label="Show a file's dependencies"
            value={find}
            onChange={(e) => setFind(e.target.value)}
            style={{ width: 260 }}
          />
          <datalist id={`graph-files-${projectKey}`}>
            {index.paths.slice(0, 5000).map((p) => (
              <option key={p} value={p} />
            ))}
          </datalist>
          <Button size="sm" icon="locate-fixed" type="submit" disabled={!find.trim()}>
            Show
          </Button>
        </form>
        <Button size="sm" icon="refresh-ccw" disabled={rebuilding} onClick={rebuild} title="Read every source file again and rebuild the code index and the graph">
          {rebuilding ? "Rebuilding…" : "Rebuild index"}
        </Button>
      </div>

      {rebuilt &&
        ("error" in rebuilt ? (
          <Banner tone="bad">{rebuilt.error}</Banner>
        ) : (
          <Banner tone={rebuilt.ok.truncated ? "warn" : "info"}>
            Indexed {rebuilt.ok.indexed_files} files in {rebuilt.ok.millis} ms.
            {rebuilt.ok.truncated && " The index size cap left some files out."}
          </Banner>
        ))}
      {graph.error && <Banner tone="bad">{graph.error.message}</Banner>}
      {graph.loading && !graph.data && (
        <div style={{ display: "flex", gap: 8, alignItems: "center", color: "var(--text-muted)" }}>
          <Spinner size={11} /> Building the graph…
        </div>
      )}

      {graph.data && placed && (
        <>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center", color: "var(--text-muted)", font: "var(--text-sm)/1.4 var(--font-sans)" }}>
            <span>
              {graph.data.nodes.length} {graph.data.view === "packages" ? "packages" : "nodes"}, {graph.data.edges.length} links, {graph.data.indexed_files} files indexed.
            </span>
            {graph.data.view === "file" && <span>What uses the file is to its left; what it uses is to its right.</span>}
            {graph.data.view !== "file" && <span>Each node uses what is to its right. Double-click to open it.</span>}
            {graph.data.truncated && <Chip tone="warn">some nodes left out</Chip>}
          </div>
          {graph.data.nodes.length === 0 ? (
            <Banner tone="info">The code index holds no source files for this project yet.</Banner>
          ) : (
            <div style={{ overflow: "auto", border: "1px solid var(--border-subtle)", borderRadius: "var(--radius-md)", background: "var(--surface-sunken)", maxHeight: "70vh" }}>
              <svg width={placed.width} height={placed.height} role="img" aria-label="Dependency graph" style={{ display: "block" }}>
                {graph.data.edges.map((e, i) => {
                  const a = byId.get(e.from);
                  const b = byId.get(e.to);
                  if (!a || !b) return null;
                  const lit = touches(e);
                  return (
                    <path
                      key={i}
                      d={edgePath(a, b)}
                      fill="none"
                      stroke={lit ? "var(--accent)" : "var(--border-strong)"}
                      strokeOpacity={focusId && !lit ? 0.25 : 1}
                      strokeWidth={Math.min(4, 1 + Math.log2(e.weight))}
                    >
                      <title>
                        {e.from} uses {e.to}
                        {e.names.length ? `: ${e.names.join(", ")}` : e.weight > 1 ? ` (${e.weight} links)` : ""}
                      </title>
                    </path>
                  );
                })}
                {placed.nodes.map((n) => {
                  const isFocus = graph.data!.view === "file" && n.id === graph.data!.focus;
                  const isSel = n.id === selected;
                  return (
                    <g
                      key={n.id}
                      transform={`translate(${n.x},${n.y})`}
                      style={{ cursor: "pointer" }}
                      onMouseEnter={() => setHover(n.id)}
                      onMouseLeave={() => setHover(null)}
                      onClick={() => setSelected(n.id === selected ? null : n.id)}
                      onDoubleClick={() => primary(n)}
                    >
                      <rect
                        width={NODE_W}
                        height={NODE_H}
                        rx={6}
                        fill={isFocus ? "var(--accent-soft)" : n.kind === "package" ? "var(--surface-raised)" : "var(--surface-panel)"}
                        stroke={isSel || isFocus ? "var(--accent)" : "var(--border-default)"}
                        strokeWidth={isSel ? 2 : 1}
                        strokeDasharray={n.test ? "4 3" : undefined}
                      />
                      <text x={10} y={NODE_H / 2 + 4} style={{ font: "12px var(--font-mono)", fill: "var(--text-primary)" }}>
                        {n.kind === "package" ? "▣ " : ""}
                        {clip(n.label)}
                      </text>
                      <title>
                        {n.id}
                        {`\n${n.dependents} dependents, ${n.dependencies} dependencies`}
                        {n.kind === "package" ? `, ${n.files} files` : ""}
                      </title>
                    </g>
                  );
                })}
              </svg>
            </div>
          )}

          {sel && (
            <div style={{ border: "1px solid var(--border-subtle)", borderRadius: "var(--radius-md)", padding: 12, display: "flex", flexDirection: "column", gap: 8 }}>
              <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
                <Icon name={sel.kind === "package" ? "box" : "file-code-2"} size={14} />
                <code style={{ wordBreak: "break-all" }}>{sel.kind === "package" ? sel.label : sel.id}</code>
                <Chip>{sel.dependents} dependents</Chip>
                <Chip>{sel.dependencies} dependencies</Chip>
                {sel.kind === "package" && <Chip>{sel.files} files</Chip>}
                {sel.test && <Chip tone="info">test</Chip>}
                <div style={{ flex: 1 }} />
                {sel.kind === "package" ? (
                  <Button size="sm" icon="box" onClick={() => primary(sel)}>
                    Show its files
                  </Button>
                ) : (
                  <>
                    {!(graph.data.view === "file" && graph.data.focus === sel.id) && (
                      <Button size="sm" icon="git-fork" onClick={() => primary(sel)}>
                        Center on this file
                      </Button>
                    )}
                    <Button size="sm" icon="file-code-2" onClick={() => nav.open(fileId(projectKey, sel.id))}>
                      Open file
                    </Button>
                  </>
                )}
              </div>
              {selEdges.length > 0 && (
                <ul style={{ margin: 0, paddingLeft: 18, font: "var(--text-sm)/1.5 var(--font-mono)", color: "var(--text-secondary)", maxHeight: 220, overflow: "auto" }}>
                  {selEdges.map((e, i) => {
                    const out = e.from === sel.id;
                    const other = out ? e.to : e.from;
                    return (
                      <li key={i}>
                        {out ? "uses " : "used by "}
                        {byId.get(other)?.label ?? other}
                        {e.names.length > 0 && `: ${e.names.join(", ")}`}
                        {e.names.length === 0 && e.weight > 1 && ` (${e.weight} links)`}
                      </li>
                    );
                  })}
                </ul>
              )}
            </div>
          )}
        </>
      )}
    </div>
  );
}
