import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent, type RefObject } from "react";
import { api } from "../../api";
import type { CodeFile, CodeGraph, CodeGraphEdge, CodeGraphMember, CodeGraphNode, CodeGraphSymbol, CodeLocation, CodeReindex, ProjectFile, SymbolKind } from "../../api/types";
import { Banner, Button, Chip, Combobox, Icon, IconButton, ICONS, Spinner, Tabs, type ComboItem, type IconName } from "../../design";
import { useAsync } from "../../lib/hooks";
import { useFileIndex } from "../../lib/live";
import { useNav } from "../../lib/nav";
import { fileId } from "../../lib/resource";
import { SourceView, type SymbolRef } from "./code/SourceView";
import "./graph.css";

/** What the graph shows: every package, one package's files, one file's neighborhood, or one definition's calls. */
export type GraphView =
  | { kind: "packages" }
  | { kind: "package"; pkg: string }
  | { kind: "file"; path: string; depth: number }
  | { kind: "symbol"; path: string; symbol: string; line: number; depth: number };

const NODE_W = 208;
const NODE_H = 44;
const MEMBER_H = 20;
const MAX_INLINE_MEMBERS = 12;
const COL_GAP = 80;
const ROW_GAP = 14;
const PAD = 20;
const HEADER = 28;
const MAX_ROWS = 10;
const LANE_GAP = 16;

export interface Placed extends CodeGraphNode {
  x: number;
  y: number;
  h: number;
}

const TYPE_KINDS: SymbolKind[] = ["class", "enum", "interface", "type"];

/** Member rows the focused type lists inside its box; other nodes list none. */
export function inlineMembers(n: CodeGraphNode, focus: string): { rows: CodeGraphMember[]; more: number } {
  const s = n.symbol;
  if (n.id !== focus || !s || !TYPE_KINDS.includes(s.kind) || s.members.length === 0) return { rows: [], more: 0 };
  const rows = s.members.slice(0, MAX_INLINE_MEMBERS);
  return { rows, more: s.members.length - rows.length + s.more_members };
}

export function nodeHeight(n: CodeGraphNode, focus: string): number {
  const { rows, more } = inlineMembers(n, focus);
  return rows.length ? NODE_H + 6 + (rows.length + (more > 0 ? 1 : 0)) * MEMBER_H + 6 : NODE_H;
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
  const h = (n: CodeGraphNode) => nodeHeight(n, g.focus);
  // A column taller than MAX_ROWS wraps into side-by-side lanes, so a wide fan-out stays on screen.
  const lanes = byCol.map((c) => Math.max(1, Math.ceil(c.length / MAX_ROWS)));
  const laneNodes = byCol.map((c, ci) => {
    const per = Math.ceil(c.length / lanes[ci]);
    return Array.from({ length: lanes[ci] }, (_, l) => c.slice(l * per, (l + 1) * per));
  });
  const stack = (ns: CodeGraphNode[]) => ns.reduce((s, n) => s + h(n), 0) + Math.max(0, ns.length - 1) * ROW_GAP;
  const tallest = Math.max(NODE_H, ...laneNodes.flat().map((ns) => stack(ns)));
  const colX: number[] = [];
  let x = PAD;
  for (let ci = 0; ci < byCol.length; ci++) {
    colX.push(x);
    x += lanes[ci] * NODE_W + (lanes[ci] - 1) * LANE_GAP + COL_GAP;
  }
  const nodes: Placed[] = laneNodes.flatMap((ls, ci) =>
    ls.flatMap((ns, l) => {
      let y = top + (tallest - stack(ns)) / 2;
      return ns.map((n) => {
        const p = { ...n, x: colX[ci] + l * (NODE_W + LANE_GAP), y, h: h(n) };
        y += p.h + ROW_GAP;
        return p;
      });
    }),
  );
  return {
    nodes,
    width: Math.max(PAD * 2, x - COL_GAP + PAD),
    height: top + PAD + tallest,
    columns: cols.map((column, ci) => ({ column, x: colX[ci] + (lanes[ci] * NODE_W + (lanes[ci] - 1) * LANE_GAP) / 2 })),
  };
}

function edgePath(a: Placed, b: Placed): string {
  const ay = a.y + NODE_H / 2;
  const by = b.y + NODE_H / 2;
  if (b.x > a.x) {
    const x1 = a.x + NODE_W;
    const x2 = b.x - 6;
    const mid = (x1 + x2) / 2;
    return `M${x1},${ay} C${mid},${ay} ${mid},${by} ${x2},${by}`;
  }
  // Same column or leftwards: a loop, drawn around the right side.
  const x = Math.max(a.x, b.x) + NODE_W;
  const bulge = x + 44;
  return `M${a.x + NODE_W},${ay} C${bulge},${ay} ${bulge},${by} ${b.x + NODE_W + 6},${by}`;
}

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

/** A node label cut to `n` characters. A qualified name keeps its last part and cuts the owner from the front. */
export function clipLabel(label: string, n: number): string {
  if (label.length <= n) return label;
  const at = Math.max(label.lastIndexOf("."), label.lastIndexOf("::") + 1);
  if (at <= 0) return clip(label, n);
  const tail = label.slice(at);
  const room = n - tail.length - 1;
  return room < 3 ? clip(tail.replace(/^[.:]+/, ""), n) : `…${label.slice(at - room, at)}${tail}`;
}

/**
 * The badge that says what else a node's file defines: "4 inside" on a file, "+3 in file" on a
 * definition, not counting a method's own type. Empty when there is nothing else. Pure, for tests.
 */
export function fileDefsHint(n: Pick<CodeGraphNode, "kind" | "symbol" | "file_defs" | "more_file_defs">): string {
  if (n.kind === "package") return "";
  const all = n.file_defs.length + n.more_file_defs;
  if (n.kind === "file") return all ? `${all} inside` : "";
  const others = all - n.file_defs.filter((t) => t.name === n.symbol?.container).length;
  return others ? `+${others} in file` : "";
}

/** The second line of a node: where it lives, and the trait its impl block implements. */
function subline(n: CodeGraphNode): string {
  if (n.kind === "package") return `${n.files} file${n.files === 1 ? "" : "s"}`;
  if (n.kind === "symbol" && n.symbol) return `${n.symbol.path.split("/").pop()}:${n.symbol.line}${n.symbol.via ? ` · ${n.symbol.via}` : ""}`;
  return n.package || "(project top)";
}

function columnTitle(view: CodeGraph["view"], column: number): string {
  if (column === 0) return view === "symbol" ? "Definition" : "File";
  const hops = Math.abs(column);
  const far = hops > 1 ? ` · ${hops} hops` : "";
  if (view === "symbol") return (column < 0 ? "Referenced or implemented by" : "Calls, uses, implements") + far;
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

/** The pan and zoom of the canvas: a point p of the graph sits at p * k + (x, y) on screen. */
export type Camera = { k: number; x: number; y: number };
export const MIN_ZOOM = 0.2;
export const MAX_ZOOM = 2.5;

/** Zoom by `factor` keeping the graph point under screen point (sx, sy) in place. Pure, for tests. */
export function zoomAround(c: Camera, factor: number, sx: number, sy: number): Camera {
  const k = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, c.k * factor));
  const r = k / c.k;
  return { k, x: sx - (sx - c.x) * r, y: sy - (sy - c.y) * r };
}

/** The camera that puts graph point (px, py) at the middle of a w×h canvas. */
export function centerOn(px: number, py: number, k: number, w: number, h: number): Camera {
  return { k, x: w / 2 - px * k, y: h / 2 - py * k };
}

/** The camera that shows the whole graph in a w×h canvas, no larger than 100%. */
export function fitCamera(gw: number, gh: number, w: number, h: number): Camera {
  const k = Math.max(MIN_ZOOM, Math.min(1, (w - 24) / gw, (h - 24) / gh));
  return centerOn(gw / 2, gh / 2, k, w, h);
}

function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

const qualified = (s: { name: string; container?: string | null }) => (s.container ? `${s.container}.${s.name}` : s.name);

type Found = { item: ComboItem; view: GraphView };

/** Search for a function, type, or file, and center the graph on the pick. Same-named definitions carry their signature. */
function GraphSearch({ ws, projectKey, paths, onPick }: { ws: string; projectKey: string; paths: string[]; onPick: (v: GraphView) => void }) {
  const [q, setQ] = useState("");
  const dq = useDebounced(q.trim(), 150);
  const [symbols, setSymbols] = useState<{ q: string; items: CodeLocation[] }>({ q: "", items: [] });
  useEffect(() => {
    if (!dq) return;
    let live = true;
    api.codeSymbols(ws, projectKey, dq, 20).then(
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
          label: <span style={{ fontFamily: "var(--font-mono)" }}>{qualified(s)}</span>,
          sub: (
            <>
              {s.preview && <span className="dg-sig">{s.preview}</span>}
              <span style={{ display: "block" }}>
                {s.path}:{s.line}
              </span>
            </>
          ),
          hint: s.via ? `${st.word} · ${s.via}` : st.word,
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
      listWidth={520}
      align="right"
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
      <span className="dg-label">Definitions: pick one to see its calls</span>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 6, maxHeight: 160, overflow: "auto" }}>
        {defs.slice(0, 80).map((s) => {
          const st = symbolStyle(s.kind);
          return (
            <button
              key={`${s.line}:${s.name}`}
              type="button"
              className="dg-def"
              title={`${st.word} ${qualified(s)}${s.via ? ` (${s.via})` : ""}, line ${s.line}`}
              onClick={() => onPick({ kind: "symbol", path, symbol: s.name, line: s.line, depth: 1 })}
            >
              <Icon name={st.icon} size={12} style={{ color: st.color }} />
              {qualified(s)}
              {s.via && <span style={{ color: "var(--text-muted)" }}>· {s.via}</span>}
            </button>
          );
        })}
      </div>
    </div>
  );
}

const memberKey = (m: { path: string; line: number }) => `${m.path}:${m.line}`;

/** A method or type as the preview shows it. */
export function asSymbol(m: CodeGraphMember, container: string | null): CodeGraphSymbol {
  return { path: m.path, name: m.name, kind: m.kind, line: m.line, end_line: m.end_line ?? null, container, via: m.via ?? null, signature: m.signature, members: [], more_members: 0 };
}

/**
 * Definitions to look through: a click shows one in the code preview, and the button at the start of
 * its row centers the graph on it.
 */
function DefList({ title, items, more = 0, active, onPeek, onCenter }: { title: string; items: CodeGraphMember[]; more?: number; active: string | null; onPeek: (m: CodeGraphMember) => void; onCenter: (m: CodeGraphMember) => void }) {
  if (!items.length) return null;
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      <span className="dg-label">
        {title} ({items.length + more})
      </span>
      <div className="dg-members">
        {items.map((m) => (
          <div key={memberKey(m)} className={`dg-member ${active === memberKey(m) ? "dg-member--active" : ""}`}>
            <IconButton icon="locate-fixed" label={`Center the graph on ${m.name}`} size="sm" onClick={() => onCenter(m)} />
            <button type="button" className="dg-member__peek" onClick={() => onPeek(m)} title={`Show ${m.path}:${m.line} in the preview`}>
              <Icon name={symbolStyle(m.kind).icon} size={12} style={{ color: symbolStyle(m.kind).color, flex: "none" }} />
              <span className="dg-member__sig">{m.signature || m.name}</span>
              {m.via && <Chip>{m.via}</Chip>}
              <span className="dg-member__where">{m.path.split("/").pop()}:{m.line}</span>
            </button>
          </div>
        ))}
        {more > 0 && <span style={{ color: "var(--text-muted)", fontSize: "var(--text-sm)", paddingLeft: 6 }}>and {more} more</span>}
      </div>
    </div>
  );
}

/** The definition's source, colored, with clickable names that move the graph to what they define. */
function DefinitionPreview({ ws, projectKey, symbol, onPick, onOpen }: { ws: string; projectKey: string; symbol: CodeGraphSymbol; onPick: (v: GraphView) => void; onOpen: () => void }) {
  const src = useAsync(
    () => Promise.all([api.projectFile(ws, projectKey, symbol.path), api.codeFile(ws, projectKey, symbol.path).catch(() => null)]) as Promise<[ProjectFile, CodeFile | null]>,
    [ws, projectKey, symbol.path],
  );
  const [miss, setMiss] = useState<string | null>(null);
  const follow = (s: SymbolRef) => {
    setMiss(null);
    api.codeUsages(ws, projectKey, s.name, { path: symbol.path, line: s.line, col: s.col, limit: 1 }).then(
      (u) => {
        const d = u.definitions[0];
        if (d) onPick({ kind: "symbol", path: d.path, symbol: d.name || s.name, line: d.line, depth: 1 });
        else setMiss(`${s.name} is not defined in this project.`);
      },
      (e: Error) => setMiss(e.message),
    );
  };
  const ext = symbol.path.split(".").pop() ?? "";
  const end = symbol.end_line ?? symbol.line;
  return (
    <div className="dg-preview">
      <div className="dg-preview__head">
        <Icon name={symbolStyle(symbol.kind).icon} size={13} style={{ color: symbolStyle(symbol.kind).color }} />
        <code>{qualified(symbol)}</code>
        <span className="dg-preview__where">
          {symbol.path}:{symbol.line}
          {end > symbol.line ? `–${end}` : ""}
        </span>
        <div style={{ flex: 1 }} />
        <Button size="sm" variant="ghost" icon="file-code-2" onClick={onOpen}>
          Open in editor
        </Button>
      </div>
      {miss && (
        <div style={{ padding: "4px 10px", color: "var(--text-muted)", fontSize: "var(--text-sm)" }}>
          {miss}
        </div>
      )}
      <div className="dg-preview__code">
        {src.error ? (
          <Banner tone="bad">{src.error.message}</Banner>
        ) : !src.data ? (
          <div style={{ display: "flex", gap: 6, alignItems: "center", color: "var(--text-muted)", padding: 10 }}>
            <Spinner size={10} /> Reading the file…
          </div>
        ) : src.data[0].content === null ? (
          <Banner tone="info">This file is binary.</Banner>
        ) : (
          <SourceView code={src.data[0].content} file={src.data[1]} language={ext} highlightLine={symbol.line} highlightEnd={end} scrollBlock="start" onSymbol={follow} />
        )}
      </div>
    </div>
  );
}

/** Pan with a drag, zoom with the wheel around the pointer. Programmatic moves animate. */
function usePanZoom(canvas: RefObject<HTMLDivElement | null>) {
  const [cam, setCam] = useState<Camera>({ k: 1, x: 0, y: 0 });
  const [animate, setAnimate] = useState(false);
  const drag = useRef<{ sx: number; sy: number; cx: number; cy: number; moved: boolean } | null>(null);
  const suppressClick = useRef(false);

  useEffect(() => {
    const el = canvas.current;
    if (!el) return;
    // React registers wheel listeners as passive, and a zoom must stop the page from scrolling.
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const r = el.getBoundingClientRect();
      const d = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
      setAnimate(false);
      setCam((c) => zoomAround(c, Math.exp(-d * 0.0015), e.clientX - r.left, e.clientY - r.top));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  });

  const move = useCallback((c: Camera) => {
    setAnimate(true);
    setCam(c);
  }, []);
  const size = () => ({ w: canvas.current?.clientWidth ?? 800, h: canvas.current?.clientHeight ?? 500 });
  const handlers = {
    onPointerDown: (e: ReactPointerEvent) => {
      if (e.button !== 0) return;
      drag.current = { sx: e.clientX, sy: e.clientY, cx: cam.x, cy: cam.y, moved: false };
    },
    onPointerMove: (e: ReactPointerEvent) => {
      const d = drag.current;
      if (!d) return;
      const dx = e.clientX - d.sx;
      const dy = e.clientY - d.sy;
      if (!d.moved && Math.hypot(dx, dy) < 4) return;
      if (!d.moved) {
        d.moved = true;
        (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
      }
      setAnimate(false);
      setCam((c) => ({ ...c, x: d.cx + dx, y: d.cy + dy }));
    },
    onPointerUp: () => {
      suppressClick.current = !!drag.current?.moved;
      drag.current = null;
    },
    // A drag that ends over a node must not also select it.
    onClickCapture: (e: ReactMouseEvent) => {
      if (suppressClick.current) {
        e.stopPropagation();
        suppressClick.current = false;
      }
    },
  };
  return { cam, animate, move, size, handlers };
}

/** The project's dependency graph: packages, a package's files, one file's neighborhood, or one definition's calls. */
export function DependencyGraph({ ws, projectKey }: { ws: string; projectKey: string }) {
  const nav = useNav();
  const [view, setView] = useState<GraphView>({ kind: "packages" });
  const [history, setHistory] = useState<GraphView[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [hover, setHover] = useState<string | null>(null);
  // A method or type picked from a list, shown in the preview without moving the graph.
  const [peek, setPeek] = useState<CodeGraphSymbol | null>(null);
  const [rebuilt, setRebuilt] = useState<{ ok: CodeReindex } | { error: string } | null>(null);
  const [rebuilding, setRebuilding] = useState(false);
  const [generation, setGeneration] = useState(0);
  const index = useFileIndex(ws, projectKey);
  const canvas = useRef<HTMLDivElement>(null);
  const pz = usePanZoom(canvas);

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

  // A new view centers its focus at 100%, or fits the whole graph when there is no focus.
  useEffect(() => {
    if (!placed) return;
    const { w, h } = pz.size();
    pz.move(focusNode ? centerOn(focusNode.x + NODE_W / 2, focusNode.y + Math.min(focusNode.h, 240) / 2, 1, w, h) : fitCamera(placed.width, placed.height, w, h));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [placed]);

  const show = (v: GraphView) => {
    setView(v);
    setSelected(null);
    setHover(null);
    setPeek(null);
  };
  const go = (v: GraphView) => {
    if (JSON.stringify(v) === JSON.stringify(view)) return;
    setHistory((h) => [...h.slice(-49), view]);
    show(v);
  };
  const back = () => {
    const prev = history[history.length - 1];
    if (!prev) return;
    setHistory((h) => h.slice(0, -1));
    show(prev);
  };
  // Alt+Left goes back, as in a browser, unless the user is typing.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (e.altKey && e.key === "ArrowLeft" && !(t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable))) {
        e.preventDefault();
        back();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
  const depth = view.kind === "file" || view.kind === "symbol" ? view.depth : 1;
  const primary = (n: CodeGraphNode) => {
    if (n.kind === "package") go({ kind: "package", pkg: n.package });
    else if (n.kind === "symbol" && n.symbol) go({ kind: "symbol", path: n.symbol.path, symbol: n.symbol.name, line: n.symbol.line, depth });
    else go({ kind: "file", path: n.id, depth });
  };
  const toMember = (m: CodeGraphMember) => go({ kind: "symbol", path: m.path, symbol: m.name, line: m.line, depth });
  const peekMember = (m: CodeGraphMember, container: string | null) => setPeek(asSymbol(m, container));
  const openAt = (path: string, line?: number) => nav.open(fileId(projectKey, path), line ? { anchor: `L${line}` } : undefined);
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
  const previewed = peek ?? sel?.symbol ?? (sel ? undefined : focusNode?.symbol);
  const peekKey = peek ? memberKey(peek) : null;
  const pick = (id: string | null) => {
    setSelected(id);
    setPeek(null);
  };
  const zoomBy = (f: number) => {
    const { w, h } = pz.size();
    pz.move(zoomAround(pz.cam, f, w / 2, h / 2));
  };
  const fit = () => {
    if (!placed) return;
    const { w, h } = pz.size();
    pz.move(fitCamera(placed.width, placed.height, w, h));
  };
  const recenter = () => {
    const { w, h } = pz.size();
    if (focusNode) pz.move(centerOn(focusNode.x + NODE_W / 2, focusNode.y + Math.min(focusNode.h, 240) / 2, 1, w, h));
    else if (placed) pz.move(centerOn(placed.width / 2, placed.height / 2, 1, w, h));
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 10, padding: "0 24px 24px", minHeight: 0 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
        <IconButton icon="arrow-left" label={history.length ? "Back to the previous view (Alt+Left)" : "No previous view"} size="sm" disabled={!history.length} onClick={back} />
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
            <span>Drag to move, scroll to zoom. {isSymbolView ? "Double-click a definition to center on it." : "Double-click to open a node."}</span>
            {graph.data.truncated && <Chip tone="warn">some nodes or links left out</Chip>}
            {graph.loading && <Spinner size={10} />}
            <div style={{ flex: 1 }} />
            <Legend view={graph.data.view} />
          </div>
          {graph.data.nodes.length === 0 ? (
            <Banner tone="info">The code index holds no source files for this project yet.</Banner>
          ) : (
            <div className="dg-frame">
              <div ref={canvas} className="dg-canvas" {...pz.handlers}>
                <svg width="100%" height="100%" role="img" aria-label="Dependency graph" style={{ display: "block" }}>
                  <defs>
                    <marker id="dg-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
                      <path d="M0,0 L8,4 L0,8 z" fill="var(--border-strong)" />
                    </marker>
                    <marker id="dg-arrow-lit" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
                      <path d="M0,0 L8,4 L0,8 z" fill="var(--accent)" />
                    </marker>
                    <marker id="dg-impl" viewBox="0 0 12 12" refX="11" refY="6" markerWidth="11" markerHeight="11" orient="auto-start-reverse">
                      <path d="M1,1 L11,6 L1,11 z" fill="var(--surface-sunken)" stroke="var(--syntax-keyword)" strokeWidth="1.3" />
                    </marker>
                  </defs>
                  <g className={pz.animate ? "dg-world dg-world--anim" : "dg-world"} style={{ transform: `translate(${pz.cam.x}px, ${pz.cam.y}px) scale(${pz.cam.k})` }}>
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
                      const impl = e.kind === "implements";
                      return (
                        <path
                          key={i}
                          d={edgePath(a, b)}
                          fill="none"
                          className={lit ? "dg-edge dg-edge--lit" : "dg-edge"}
                          stroke={lit ? "var(--accent)" : impl ? "var(--syntax-keyword)" : "var(--border-strong)"}
                          strokeOpacity={focusId && !lit ? 0.18 : 0.9}
                          strokeWidth={Math.min(3.5, 1.2 + Math.log2(e.weight)) + (lit ? 0.6 : 0)}
                          strokeDasharray={impl ? "7 3" : e.import || isSymbolView ? undefined : "5 4"}
                          markerEnd={`url(#${impl ? "dg-impl" : lit ? "dg-arrow-lit" : "dg-arrow"})`}
                        >
                          <title>{edgeTitle(graph.data!, e, byId)}</title>
                        </path>
                      );
                    })}
                    {placed.nodes.map((n) => {
                      const isFocus = n.id === focusNode?.id;
                      const isSel = n.id === selected;
                      const dim = neighbors !== null && !neighbors.has(n.id);
                      const st = kindStyle(n);
                      const Glyph = ICONS[st.icon];
                      const inline = inlineMembers(n, graph.data!.focus);
                      return (
                        <g
                          key={n.id}
                          className="dg-node"
                          transform={`translate(${n.x},${n.y})`}
                          opacity={dim ? 0.35 : 1}
                          onMouseEnter={() => setHover(n.id)}
                          onMouseLeave={() => setHover(null)}
                          onClick={() => pick(n.id === selected ? null : n.id)}
                          onDoubleClick={() => primary(n)}
                        >
                          {isFocus && <rect x={-4} y={-4} width={NODE_W + 8} height={n.h + 8} rx={10} fill="none" stroke="var(--accent)" strokeOpacity={0.35} strokeWidth={4} />}
                          <rect
                            width={NODE_W}
                            height={n.h}
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
                            {clipLabel(n.label, 22)}
                          </text>
                          <text x={36} y={33} className="dg-node__sub">
                            {clip(subline(n), fileDefsHint(n) ? 22 : 30)}
                          </text>
                          {(n.dependents > 0 || n.dependencies > 0) && n.kind !== "symbol" && (
                            <text x={NODE_W - 8} y={19} textAnchor="end" className="dg-node__count">
                              {n.dependents}↘ {n.dependencies}↗
                            </text>
                          )}
                          {n.kind === "symbol" && !isFocus && (n.symbol?.members.length ?? 0) > 0 && (
                            <text x={NODE_W - 8} y={19} textAnchor="end" className="dg-node__count">
                              {(n.symbol?.members.length ?? 0) + (n.symbol?.more_members ?? 0)} ƒ
                            </text>
                          )}
                          {fileDefsHint(n) && (
                            <text x={NODE_W - 8} y={33} textAnchor="end" className="dg-node__types">
                              {fileDefsHint(n)}
                              <title>{`${n.kind === "file" ? "In this file" : "Also in this file"}: ${n.file_defs.map((t) => t.name).join(", ")}${n.more_file_defs ? `, and ${n.more_file_defs} more` : ""}. Select the node to go to one.`}</title>
                            </text>
                          )}
                          {inline.rows.length > 0 && <line x1={8} x2={NODE_W - 8} y1={NODE_H + 2} y2={NODE_H + 2} stroke="var(--border-subtle)" />}
                          {inline.rows.map((m, i) => {
                            const ms = symbolStyle(m.kind);
                            const MGlyph = ICONS[ms.icon];
                            return (
                              <g
                                key={`${m.path}:${m.line}`}
                                className="dg-row"
                                transform={`translate(0,${NODE_H + 6 + i * MEMBER_H})`}
                                onClick={(e) => {
                                  e.stopPropagation();
                                  peekMember(m, n.symbol?.name ?? null);
                                }}
                                onDoubleClick={(e) => {
                                  e.stopPropagation();
                                  toMember(m);
                                }}
                              >
                                <rect x={4} y={0} width={NODE_W - 8} height={MEMBER_H} rx={4} className={peekKey === memberKey(m) ? "dg-row__bg dg-row__bg--active" : "dg-row__bg"} />
                                <MGlyph x={12} y={3} width={13} height={13} color={ms.color} strokeWidth={1.75} />
                                <text x={32} y={14} className="dg-row__label">
                                  {clip(m.name, m.via ? 16 : 22)}
                                </text>
                                {m.via && (
                                  <text x={NODE_W - 10} y={14} textAnchor="end" className="dg-row__via">
                                    {clip(m.via, 10)}
                                  </text>
                                )}
                                <title>{`${m.signature}\n${m.path}:${m.line}${m.via ? `\nimplements ${m.via}` : ""}\nClick to preview it, double-click to center on it.`}</title>
                              </g>
                            );
                          })}
                          {inline.more > 0 && (
                            <text x={32} y={NODE_H + 6 + inline.rows.length * MEMBER_H + 14} className="dg-node__sub">
                              and {inline.more} more
                            </text>
                          )}
                          <title>
                            {n.symbol ? `${st.word} ${n.label}\n${n.symbol.signature || ""}\n${n.symbol.path}:${n.symbol.line}` : n.id}
                            {n.kind === "symbol"
                              ? `\n${n.dependents} links in, ${n.dependencies} links out, in this view`
                              : `\n${n.dependents} dependents, ${n.dependencies} dependencies${n.kind === "package" ? `, ${n.files} files` : ""}`}
                          </title>
                        </g>
                      );
                    })}
                  </g>
                </svg>
              </div>
              <div className="dg-zoom" role="group" aria-label="Zoom">
                <IconButton icon="zoom-out" label="Zoom out" size="sm" disabled={pz.cam.k <= MIN_ZOOM} onClick={() => zoomBy(1 / 1.25)} />
                <button type="button" className="dg-zoom__pct" onClick={recenter} title="Back to 100% on the focus">
                  {Math.round(pz.cam.k * 100)}%
                </button>
                <IconButton icon="zoom-in" label="Zoom in" size="sm" disabled={pz.cam.k >= MAX_ZOOM} onClick={() => zoomBy(1.25)} />
                <IconButton icon="scan" label="Fit to view" size="sm" onClick={fit} />
              </div>
            </div>
          )}

          <div className="dg-below">
            {sel && (
              <div className="dg-details">
                <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
                  <Icon name={kindStyle(sel).icon} size={14} style={{ color: kindStyle(sel).color }} />
                  <code style={{ wordBreak: "break-all" }}>{sel.kind === "file" ? sel.id : sel.label}</code>
                  {sel.kind === "symbol" ? (
                    <>
                      <Chip>{kindStyle(sel).word}</Chip>
                      {sel.symbol?.via && <Chip tone="accent">implements {sel.symbol.via}</Chip>}
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
                      {!sel.symbol && (
                        <Button size="sm" icon="file-code-2" onClick={() => openAt(sel.id)}>
                          Open file
                        </Button>
                      )}
                    </>
                  )}
                </div>
                {sel.symbol?.signature && <code className="dg-sig dg-sig--block">{sel.symbol.signature}</code>}
                {selEdges.length > 0 && (
                  <ul className="dg-links">
                    {selEdges.map((e, i) => {
                      const out = e.from === sel.id;
                      const other = out ? e.to : e.from;
                      const verb = e.kind === "implements" ? (out ? "implements " : "implemented by ") : isSymbolView ? (out ? "calls or uses " : "referenced by ") : out ? "uses " : "used by ";
                      return (
                        <li key={i}>
                          <button type="button" className="dg-link" onClick={() => pick(other)}>
                            {verb}
                            {byId.get(other)?.label ?? other}
                          </button>
                          {e.names.length > 0 && `: ${e.names.join(", ")}`}
                          {e.names.length === 0 && e.weight > 1 && ` (${e.weight} ${isSymbolView ? "lines" : "links"})`}
                        </li>
                      );
                    })}
                  </ul>
                )}
                {sel.symbol && (
                  <DefList title="Methods and functions" items={sel.symbol.members} more={sel.symbol.more_members} active={peekKey} onPeek={(m) => peekMember(m, sel.symbol!.name)} onCenter={toMember} />
                )}
                <DefList title={sel.kind === "file" ? "In this file" : "Also in this file"} items={sel.file_defs} more={sel.more_file_defs} active={peekKey} onPeek={(m) => peekMember(m, null)} onCenter={toMember} />
                {sel.kind === "file" && <FileSymbols ws={ws} projectKey={projectKey} path={sel.id} onPick={go} />}
              </div>
            )}
            {!sel && focusNode && ((focusNode.symbol?.members.length ?? 0) > 0 || focusNode.file_defs.length > 0) && (
              <div className="dg-details">
                {focusNode.symbol && (
                  <DefList title="Methods and functions" items={focusNode.symbol.members} more={focusNode.symbol.more_members} active={peekKey} onPeek={(m) => peekMember(m, focusNode.symbol!.name)} onCenter={toMember} />
                )}
                <DefList title={focusNode.kind === "file" ? "In this file" : "Also in this file"} items={focusNode.file_defs} more={focusNode.more_file_defs} active={peekKey} onPeek={(m) => peekMember(m, null)} onCenter={toMember} />
              </div>
            )}
            {previewed && (
              <DefinitionPreview
                key={previewed.path}
                ws={ws}
                projectKey={projectKey}
                symbol={previewed}
                onPick={go}
                onOpen={() => openAt(previewed.path, previewed.line)}
              />
            )}
          </div>
        </>
      )}
    </div>
  );
}

function edgeTitle(g: CodeGraph, e: CodeGraphEdge, byId: Map<string, Placed>): string {
  const a = byId.get(e.from)?.label ?? e.from;
  const b = byId.get(e.to)?.label ?? e.to;
  if (e.kind === "implements") return `${a} implements ${b}`;
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
    <span style={{ display: "inline-flex", gap: 12, alignItems: "center", flexWrap: "wrap" }}>
      {items.map((i) => (
        <span key={i.label} style={sw}>
          <Icon name={i.icon} size={12} style={{ color: i.color }} />
          {i.label}
        </span>
      ))}
      {view === "symbol" ? (
        <span style={sw}>
          <svg width={26} height={10} aria-hidden>
            <line x1={0} y1={5} x2={18} y2={5} stroke="var(--syntax-keyword)" strokeWidth={1.5} strokeDasharray="7 3" />
            <path d="M17,1 L25,5 L17,9 z" fill="var(--surface-sunken)" stroke="var(--syntax-keyword)" strokeWidth={1.2} />
          </svg>
          implements
        </span>
      ) : (
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
