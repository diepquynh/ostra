import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { api } from "../../api";
import type { CodeGraph, CodeGraphEdge, CodeGraphNode, CodeLocation, CodeReindex, SymbolKind } from "../../api/types";
import { Banner, Button, Chip, Combobox, Icon, IconButton, ICONS, Spinner, Tabs, type ComboItem, type IconName } from "../../design";
import { useAsync } from "../../lib/hooks";
import { useFileIndex } from "../../lib/live";
import { useNav } from "../../lib/nav";
import { fileId } from "../../lib/resource";
import "./graph.css";

/** What the graph shows: every package, one package's files, one file's neighborhood, or one definition's calls. */
export type GraphView =
  | { kind: "packages" }
  | { kind: "package"; pkg: string }
  | { kind: "file"; path: string; depth: number }
  | { kind: "symbol"; path: string; symbol: string; line: number; depth: number };

const NODE_W = 208;
const NODE_H = 44;
const COL_GAP = 72;
const ROW_GAP = 14;
const PAD = 20;
const HEADER = 28;
const MAX_ROWS = 10;
const LANE_GAP = 16;

export interface Placed extends CodeGraphNode {
  x: number;
  y: number;
}

/**
 * Columns left to right as the server numbered them (a node uses nodes to its right), each column
 * ordered by the average row of its neighbors so links cross less. Pure, for tests.
 */
export function layout(g: CodeGraph): { nodes: Placed[]; width: number; height: number; columns: { column: number; x: number }[] } {
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
  const top = PAD + (g.view === "file" || g.view === "symbol" ? HEADER : 0);
  // A column taller than MAX_ROWS wraps into side-by-side lanes, so a wide fan-out stays on screen.
  const lanes = byCol.map((c) => Math.max(1, Math.ceil(c.length / MAX_ROWS)));
  const rowsOf = (ci: number) => Math.ceil(byCol[ci].length / lanes[ci]);
  const tallest = Math.max(1, ...byCol.map((_, ci) => rowsOf(ci)));
  const colHeight = (k: number) => k * NODE_H + (k - 1) * ROW_GAP;
  const height = top + PAD + colHeight(tallest);
  const colX: number[] = [];
  let x = PAD;
  for (let ci = 0; ci < byCol.length; ci++) {
    colX.push(x);
    x += lanes[ci] * NODE_W + (lanes[ci] - 1) * LANE_GAP + COL_GAP;
  }
  const nodes = byCol.flatMap((c, ci) => {
    const rows = rowsOf(ci);
    const y0 = top + (colHeight(tallest) - colHeight(rows)) / 2;
    return c.map((n, i) => {
      const lane = Math.floor(i / rows);
      return { ...n, x: colX[ci] + lane * (NODE_W + LANE_GAP), y: y0 + (i % rows) * (NODE_H + ROW_GAP) };
    });
  });
  return {
    nodes,
    width: Math.max(PAD * 2, x - COL_GAP + PAD),
    height,
    columns: cols.map((column, ci) => ({ column, x: colX[ci] + (lanes[ci] * NODE_W + (lanes[ci] - 1) * LANE_GAP) / 2 })),
  };
}

function edgePath(a: Placed, b: Placed): string {
  const ay = a.y + NODE_H / 2;
  const by = b.y + NODE_H / 2;
  if (b.x > a.x) {
    const x1 = a.x + NODE_W;
    const x2 = b.x - 4;
    const mid = (x1 + x2) / 2;
    return `M${x1},${ay} C${mid},${ay} ${mid},${by} ${x2},${by}`;
  }
  // Same column or leftwards: a loop, drawn around the right side.
  const x = Math.max(a.x, b.x) + NODE_W;
  const bulge = x + 44;
  return `M${a.x + NODE_W},${ay} C${bulge},${ay} ${bulge},${by} ${b.x + NODE_W + 4},${by}`;
}

const TYPE_KINDS: SymbolKind[] = ["class", "enum", "interface", "type"];

/** A color and an icon per node kind, and per symbol kind for definitions. */
export function kindStyle(n: Pick<CodeGraphNode, "kind" | "symbol">): { color: string; icon: IconName; word: string } {
  if (n.kind === "package") return { color: "var(--syntax-keyword)", icon: "box", word: "package" };
  if (n.kind === "file") return { color: "var(--syntax-string)", icon: "file-code-2", word: "file" };
  return symbolStyle(n.symbol?.kind ?? "function");
}

function symbolStyle(k: SymbolKind): { color: string; icon: IconName; word: string } {
  if (TYPE_KINDS.includes(k)) return { color: "var(--syntax-number)", icon: "shapes", word: k };
  if (k === "function" || k === "method" || k === "macro") return { color: "var(--syntax-fn)", icon: "square-function", word: k };
  return { color: "var(--text-secondary)", icon: "braces", word: k };
}

const clip = (s: string, n: number) => (s.length > n ? `${s.slice(0, n - 1)}…` : s);

/** The second line of a node: where it lives. */
function subline(n: CodeGraphNode): string {
  if (n.kind === "package") return `${n.files} file${n.files === 1 ? "" : "s"}`;
  if (n.kind === "symbol" && n.symbol) return `${n.symbol.path.split("/").pop()}:${n.symbol.line}`;
  return n.package || "(project top)";
}

function columnTitle(view: CodeGraph["view"], column: number): string {
  if (column === 0) return view === "symbol" ? "Definition" : "File";
  const hops = Math.abs(column);
  const far = hops > 1 ? ` · ${hops} hops` : "";
  if (view === "symbol") return (column < 0 ? "Referenced by" : "Calls and uses") + far;
  return (column < 0 ? "Used by" : "Uses") + far;
}

/** Files whose path holds every part of the query, basename matches first. Pure, for tests. */
export function matchFiles(paths: string[], query: string, limit: number): string[] {
  const parts = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!parts.length) return [];
  const hits: [number, string][] = [];
  for (const p of paths) {
    const low = p.toLowerCase();
    if (!parts.every((q) => low.includes(q))) continue;
    const base = low.slice(low.lastIndexOf("/") + 1);
    const score = base.startsWith(parts[0]) ? 0 : base.includes(parts[0]) ? 1 : 2;
    hits.push([score * 1000 + p.length, p]);
  }
  return hits.sort((a, b) => a[0] - b[0]).slice(0, limit).map(([, p]) => p);
}

function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

type Found = { item: ComboItem; view: GraphView };

/** Search for a function, type, or file, and center the graph on the pick. */
function GraphSearch({ ws, projectKey, paths, onPick }: { ws: string; projectKey: string; paths: string[]; onPick: (v: GraphView) => void }) {
  const [q, setQ] = useState("");
  const dq = useDebounced(q.trim(), 150);
  const [symbols, setSymbols] = useState<{ q: string; items: CodeLocation[] }>({ q: "", items: [] });
  useEffect(() => {
    if (!dq) return;
    let live = true;
    api.codeSymbols(ws, projectKey, dq, 12).then(
      (r) => live && setSymbols({ q: dq, items: r.items }),
      () => live && setSymbols({ q: dq, items: [] }),
    );
    return () => {
      live = false;
    };
  }, [ws, projectKey, dq]);

  const found: Found[] = useMemo(() => {
    if (!q.trim()) return [];
    const syms = symbols.q === dq ? symbols.items : [];
    const out: Found[] = syms.map((s) => {
      const st = symbolStyle(s.kind ?? "function");
      return {
        item: {
          id: `s:${s.path}:${s.line}:${s.name}`,
          group: "Symbols",
          icon: st.icon,
          iconColor: st.color,
          label: <span style={{ fontFamily: "var(--font-mono)" }}>{s.container ? `${s.container}.${s.name}` : s.name}</span>,
          sub: `${s.path}:${s.line}`,
          hint: st.word,
        },
        view: { kind: "symbol", path: s.path, symbol: s.name, line: s.line, depth: 1 },
      };
    });
    for (const p of matchFiles(paths, q, 8))
      out.push({
        item: { id: `f:${p}`, group: "Files", icon: "file-code-2", iconColor: "var(--syntax-string)", label: <span style={{ fontFamily: "var(--font-mono)" }}>{p.split("/").pop()}</span>, sub: p },
        view: { kind: "file", path: p, depth: 1 },
      });
    return out;
  }, [q, dq, symbols, paths]);

  const pick = (v: GraphView) => {
    onPick(v);
    setQ("");
  };
  return (
    <Combobox
      size="sm"
      width={320}
      listWidth={440}
      label="Find a function, type, or file"
      placeholder="Find a function, type, or file"
      value={q}
      onChange={setQ}
      loading={!!q.trim() && symbols.q !== dq}
      items={found.map((f) => f.item)}
      onSelect={(it) => {
        const f = found.find((x) => x.item.id === it.id);
        if (f) pick(f.view);
      }}
      onSubmit={(text) => {
        const t = text.trim();
        if (paths.includes(t)) pick({ kind: "file", path: t, depth: 1 });
      }}
    />
  );
}

/** A file's definitions, each a way into the symbol view. */
function FileSymbols({ ws, projectKey, path, onPick }: { ws: string; projectKey: string; path: string; onPick: (v: GraphView) => void }) {
  const file = useAsync(() => api.codeFile(ws, projectKey, path), [ws, projectKey, path]);
  if (file.loading && !file.data)
    return (
      <span style={{ color: "var(--text-muted)", display: "flex", gap: 6, alignItems: "center" }}>
        <Spinner size={10} /> Reading its definitions…
      </span>
    );
  const defs = (file.data?.symbols ?? []).filter((s) => s.kind !== "field" && s.kind !== "variable" && s.kind !== "module");
  if (!defs.length) return null;
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
      <span style={{ font: "var(--type-label)", letterSpacing: "var(--tracking-label)", textTransform: "uppercase", color: "var(--text-muted)" }}>
        Definitions: pick one to see its calls
      </span>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 6, maxHeight: 160, overflow: "auto" }}>
        {defs.slice(0, 80).map((s) => {
          const st = symbolStyle(s.kind);
          return (
            <button
              key={`${s.line}:${s.name}`}
              type="button"
              className="dg-def"
              title={`${st.word} ${s.container ? `${s.container}.` : ""}${s.name}, line ${s.line}`}
              onClick={() => onPick({ kind: "symbol", path, symbol: s.name, line: s.line, depth: 1 })}
            >
              <Icon name={st.icon} size={12} style={{ color: st.color }} />
              {s.container ? `${s.container}.${s.name}` : s.name}
            </button>
          );
        })}
      </div>
    </div>
  );
}

const ZOOMS = [0.5, 0.67, 0.8, 1, 1.25, 1.5];

/** The project's dependency graph: packages, a package's files, one file's neighborhood, or one definition's calls. */
export function DependencyGraph({ ws, projectKey }: { ws: string; projectKey: string }) {
  const nav = useNav();
  const [view, setView] = useState<GraphView>({ kind: "packages" });
  const [selected, setSelected] = useState<string | null>(null);
  const [hover, setHover] = useState<string | null>(null);
  const [zoom, setZoom] = useState(1);
  const [rebuilt, setRebuilt] = useState<{ ok: CodeReindex } | { error: string } | null>(null);
  const [rebuilding, setRebuilding] = useState(false);
  const [generation, setGeneration] = useState(0);
  const index = useFileIndex(ws, projectKey);
  const canvas = useRef<HTMLDivElement>(null);

  const graph = useAsync(() => {
    const at =
      view.kind === "packages"
        ? {}
        : view.kind === "package"
          ? { package: view.pkg || "." }
          : view.kind === "file"
            ? { path: view.path, depth: view.depth }
            : { path: view.path, symbol: view.symbol, line: view.line, depth: view.depth };
    return api.codeGraph(ws, projectKey, at);
  }, [ws, projectKey, JSON.stringify(view), generation]);

  const placed = useMemo(() => (graph.data ? layout(graph.data) : null), [graph.data]);
  const byId = useMemo(() => new Map((placed?.nodes ?? []).map((n) => [n.id, n])), [placed]);
  const focusNode = graph.data && (graph.data.view === "file" || graph.data.view === "symbol") ? byId.get(graph.data.focus) : undefined;

  // Bring the focus into the middle of the canvas whenever a new view lands.
  useEffect(() => {
    const el = canvas.current;
    if (!el || !placed) return;
    if (!focusNode) {
      el.scrollTo({ left: 0, top: 0 });
      return;
    }
    el.scrollTo({
      left: (focusNode.x + NODE_W / 2) * zoom - el.clientWidth / 2,
      top: (focusNode.y + NODE_H / 2) * zoom - el.clientHeight / 2,
      behavior: "smooth",
    });
    // Zoom changes keep the scroll position the user chose.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [placed, focusNode]);

  const go = (v: GraphView) => {
    setView(v);
    setSelected(null);
    setHover(null);
  };
  const depth = view.kind === "file" || view.kind === "symbol" ? view.depth : 1;
  const primary = (n: CodeGraphNode) => {
    if (n.kind === "package") go({ kind: "package", pkg: n.package });
    else if (n.kind === "symbol" && n.symbol) go({ kind: "symbol", path: n.symbol.path, symbol: n.symbol.name, line: n.symbol.line, depth });
    else go({ kind: "file", path: n.id, depth });
  };
  const openSource = (n: CodeGraphNode) => {
    if (n.symbol) nav.open(fileId(projectKey, n.symbol.path), { anchor: `L${n.symbol.line}` });
    else nav.open(fileId(projectKey, n.id));
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
  const neighbors = useMemo(() => {
    if (!focusId || !graph.data) return null;
    const s = new Set([focusId]);
    for (const e of graph.data.edges) {
      if (e.from === focusId) s.add(e.to);
      if (e.to === focusId) s.add(e.from);
    }
    return s;
  }, [focusId, graph.data]);
  const touches = (e: CodeGraphEdge) => focusId !== null && (e.from === focusId || e.to === focusId);

  const crumbs: { label: string; view: GraphView }[] = [{ label: "Packages", view: { kind: "packages" } }];
  if (view.kind === "package") crumbs.push({ label: view.pkg || "(project top)", view });
  if (view.kind === "file" || view.kind === "symbol") {
    const pkg = focusNode?.package;
    if (pkg !== undefined) crumbs.push({ label: pkg || "(project top)", view: { kind: "package", pkg } });
    crumbs.push({ label: view.path, view: { kind: "file", path: view.path, depth: view.depth } });
    if (view.kind === "symbol") crumbs.push({ label: focusNode?.label ?? view.symbol, view });
  }
  const sel = selected ? byId.get(selected) : undefined;
  const selEdges = sel && graph.data ? graph.data.edges.filter((e) => e.from === sel.id || e.to === sel.id) : [];
  const isSymbolView = graph.data?.view === "symbol";
  const zoomAt = (d: number) => setZoom((z) => ZOOMS[Math.max(0, Math.min(ZOOMS.length - 1, ZOOMS.indexOf(z) + d))]);
  const fit = () => {
    const el = canvas.current;
    if (!el || !placed) return;
    const s = Math.min(el.clientWidth / placed.width, el.clientHeight / placed.height, 1);
    setZoom([...ZOOMS].reverse().find((z) => z <= s) ?? ZOOMS[0]);
  };

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
        {(view.kind === "file" || view.kind === "symbol") && (
          <Tabs
            variant="segmented"
            label="Hops shown"
            tabs={[1, 2, 3].map((d) => ({ id: String(d), label: `${d} hop${d > 1 ? "s" : ""}` }))}
            value={String(view.depth)}
            onChange={(d) => go({ ...view, depth: Number(d) })}
          />
        )}
        <GraphSearch ws={ws} projectKey={projectKey} paths={index.paths} onPick={go} />
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
          <div style={{ display: "flex", gap: 12, flexWrap: "wrap", alignItems: "center", color: "var(--text-muted)", font: "var(--text-sm)/1.4 var(--font-sans)" }}>
            <span>
              {graph.data.nodes.length} {graph.data.view === "packages" ? "packages" : "nodes"}, {graph.data.edges.length} links, {graph.data.indexed_files} files indexed.
            </span>
            <span>{isSymbolView ? "Double-click a definition to center on it." : "Double-click to open a node."}</span>
            {graph.data.truncated && <Chip tone="warn">some nodes left out</Chip>}
            {graph.loading && <Spinner size={10} />}
            <div style={{ flex: 1 }} />
            <Legend view={graph.data.view} />
          </div>
          {graph.data.nodes.length === 0 ? (
            <Banner tone="info">The code index holds no source files for this project yet.</Banner>
          ) : (
            <div className="dg-frame">
              <div
                ref={canvas}
                className="dg-canvas"
                onWheel={(e) => {
                  if (!e.ctrlKey && !e.metaKey) return;
                  e.preventDefault();
                  zoomAt(e.deltaY < 0 ? 1 : -1);
                }}
              >
                <svg width={placed.width * zoom} height={placed.height * zoom} viewBox={`0 0 ${placed.width} ${placed.height}`} role="img" aria-label="Dependency graph" style={{ display: "block" }}>
                  <defs>
                    <marker id="dg-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
                      <path d="M0,0 L8,4 L0,8 z" fill="var(--border-strong)" />
                    </marker>
                    <marker id="dg-arrow-lit" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
                      <path d="M0,0 L8,4 L0,8 z" fill="var(--accent)" />
                    </marker>
                  </defs>
                  {(graph.data.view === "file" || graph.data.view === "symbol") &&
                    placed.columns.map((c) => (
                      <text key={c.column} x={c.x} y={PAD + 12} textAnchor="middle" className="dg-colhead">
                        {columnTitle(graph.data!.view, c.column).toUpperCase()}
                      </text>
                    ))}
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
                        className={lit ? "dg-edge dg-edge--lit" : "dg-edge"}
                        stroke={lit ? "var(--accent)" : "var(--border-strong)"}
                        strokeOpacity={focusId && !lit ? 0.18 : 0.9}
                        strokeWidth={Math.min(3.5, 1.2 + Math.log2(e.weight)) + (lit ? 0.6 : 0)}
                        strokeDasharray={e.import || isSymbolView ? undefined : "5 4"}
                        markerEnd={`url(#${lit ? "dg-arrow-lit" : "dg-arrow"})`}
                      >
                        <title>
                          {edgeTitle(graph.data!, e, byId)}
                        </title>
                      </path>
                    );
                  })}
                  {placed.nodes.map((n) => {
                    const isFocus = n.id === focusNode?.id;
                    const isSel = n.id === selected;
                    const dim = neighbors !== null && !neighbors.has(n.id);
                    const st = kindStyle(n);
                    const Glyph = ICONS[st.icon];
                    return (
                      <g
                        key={n.id}
                        className="dg-node"
                        transform={`translate(${n.x},${n.y})`}
                        opacity={dim ? 0.35 : 1}
                        onMouseEnter={() => setHover(n.id)}
                        onMouseLeave={() => setHover(null)}
                        onClick={() => setSelected(n.id === selected ? null : n.id)}
                        onDoubleClick={() => primary(n)}
                      >
                        {isFocus && <rect x={-4} y={-4} width={NODE_W + 8} height={NODE_H + 8} rx={10} fill="none" stroke="var(--accent)" strokeOpacity={0.35} strokeWidth={4} />}
                        <rect
                          width={NODE_W}
                          height={NODE_H}
                          rx={8}
                          fill={isFocus ? "var(--accent-soft)" : n.kind === "package" ? "var(--surface-raised)" : "var(--surface-panel)"}
                          stroke={isSel || isFocus ? "var(--accent)" : "var(--border-default)"}
                          strokeWidth={isSel ? 2 : 1}
                          strokeDasharray={n.test ? "4 3" : undefined}
                          className="dg-node__box"
                        />
                        <rect x={0} y={8} width={3} height={NODE_H - 16} rx={1.5} fill={st.color} />
                        <Glyph x={12} y={(NODE_H - 16) / 2} width={16} height={16} color={st.color} strokeWidth={1.75} />
                        <text x={36} y={19} className="dg-node__label">
                          {clip(n.label, 22)}
                        </text>
                        <text x={36} y={33} className="dg-node__sub">
                          {clip(subline(n), 28)}
                        </text>
                        {(n.dependents > 0 || n.dependencies > 0) && n.kind !== "symbol" && (
                          <text x={NODE_W - 8} y={19} textAnchor="end" className="dg-node__count">
                            {n.dependents}↘ {n.dependencies}↗
                          </text>
                        )}
                        <title>
                          {n.symbol ? `${st.word} ${n.label}\n${n.symbol.path}:${n.symbol.line}` : n.id}
                          {n.kind === "symbol"
                            ? `\n${n.dependents} references in view, ${n.dependencies} calls in view`
                            : `\n${n.dependents} dependents, ${n.dependencies} dependencies${n.kind === "package" ? `, ${n.files} files` : ""}`}
                        </title>
                      </g>
                    );
                  })}
                </svg>
              </div>
              <div className="dg-zoom" role="group" aria-label="Zoom">
                <IconButton icon="zoom-out" label="Zoom out" size="sm" disabled={zoom === ZOOMS[0]} onClick={() => zoomAt(-1)} />
                <button type="button" className="dg-zoom__pct" onClick={() => setZoom(1)} title="Reset zoom">
                  {Math.round(zoom * 100)}%
                </button>
                <IconButton icon="zoom-in" label="Zoom in" size="sm" disabled={zoom === ZOOMS[ZOOMS.length - 1]} onClick={() => zoomAt(1)} />
                <IconButton icon="scan" label="Fit to view" size="sm" onClick={fit} />
              </div>
            </div>
          )}

          {sel && (
            <div style={{ border: "1px solid var(--border-subtle)", borderRadius: "var(--radius-md)", padding: 12, display: "flex", flexDirection: "column", gap: 10, background: "var(--surface-panel)" }}>
              <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
                <Icon name={kindStyle(sel).icon} size={14} style={{ color: kindStyle(sel).color }} />
                <code style={{ wordBreak: "break-all" }}>{sel.kind === "file" ? sel.id : sel.label}</code>
                {sel.symbol && <span style={{ color: "var(--text-muted)", fontFamily: "var(--font-mono)", fontSize: "var(--text-sm)" }}>{sel.symbol.path}:{sel.symbol.line}</span>}
                {sel.kind === "symbol" ? (
                  <>
                    <Chip>{kindStyle(sel).word}</Chip>
                    <Chip>{sel.dependents} references shown</Chip>
                    <Chip>{sel.dependencies} calls shown</Chip>
                  </>
                ) : (
                  <>
                    <Chip>{sel.dependents} dependents</Chip>
                    <Chip>{sel.dependencies} dependencies</Chip>
                  </>
                )}
                {sel.kind === "package" && <Chip>{sel.files} files</Chip>}
                {sel.test && <Chip tone="info">test</Chip>}
                <div style={{ flex: 1 }} />
                {sel.kind === "package" ? (
                  <Button size="sm" icon="box" onClick={() => primary(sel)}>
                    Show its files
                  </Button>
                ) : (
                  <>
                    {sel.id !== focusNode?.id && (
                      <Button size="sm" icon={sel.kind === "symbol" ? "workflow" : "git-fork"} onClick={() => primary(sel)}>
                        {sel.kind === "symbol" ? "Center on this definition" : "Center on this file"}
                      </Button>
                    )}
                    <Button size="sm" icon="file-code-2" onClick={() => openSource(sel)}>
                      {sel.symbol ? "Open at definition" : "Open file"}
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
                        {isSymbolView ? (out ? "calls or uses " : "referenced by ") : out ? "uses " : "used by "}
                        {byId.get(other)?.label ?? other}
                        {e.names.length > 0 && `: ${e.names.join(", ")}`}
                        {e.names.length === 0 && e.weight > 1 && ` (${e.weight} ${isSymbolView ? "lines" : "links"})`}
                      </li>
                    );
                  })}
                </ul>
              )}
              {sel.kind === "file" && <FileSymbols ws={ws} projectKey={projectKey} path={sel.id} onPick={go} />}
            </div>
          )}
        </>
      )}
    </div>
  );
}

function edgeTitle(g: CodeGraph, e: CodeGraphEdge, byId: Map<string, Placed>): string {
  const a = byId.get(e.from)?.label ?? e.from;
  const b = byId.get(e.to)?.label ?? e.to;
  if (g.view === "symbol") return `${a} references ${b}${e.weight > 1 ? ` on ${e.weight} lines` : ""}`;
  return `${e.from} uses ${e.to}${e.names.length ? `: ${e.names.join(", ")}` : e.weight > 1 ? ` (${e.weight} links)` : ""}`;
}

function Legend({ view }: { view: CodeGraph["view"] }) {
  const items: { color: string; icon: IconName; label: string }[] =
    view === "symbol"
      ? [
          { color: "var(--syntax-fn)", icon: "square-function", label: "function" },
          { color: "var(--syntax-number)", icon: "shapes", label: "type" },
          { color: "var(--syntax-string)", icon: "file-code-2", label: "file top level" },
        ]
      : [
          { color: "var(--syntax-keyword)", icon: "box", label: "package" },
          { color: "var(--syntax-string)", icon: "file-code-2", label: "file" },
        ];
  const sw: CSSProperties = { display: "inline-flex", alignItems: "center", gap: 4 };
  return (
    <span style={{ display: "inline-flex", gap: 12, alignItems: "center" }}>
      {items.map((i) => (
        <span key={i.label} style={sw}>
          <Icon name={i.icon} size={12} style={{ color: i.color }} />
          {i.label}
        </span>
      ))}
      {view !== "symbol" && (
        <span style={sw}>
          <svg width={22} height={6} aria-hidden>
            <line x1={0} y1={3} x2={22} y2={3} stroke="var(--border-strong)" strokeWidth={1.5} strokeDasharray="5 4" />
          </svg>
          names only, no import
        </span>
      )}
    </span>
  );
}
