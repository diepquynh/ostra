import { Banner, Button, Icon, type IconName, Input, Spinner, type TabItem, Tabs } from "@ostra/design";
import { type ReactNode, useEffect, useMemo, useState } from "react";
import { api } from "../../../api";
import type { CodeFile, CodeLocation, CodeSymbol, SymbolKind } from "../../../api/types";
import { useAsync } from "../../../lib/hooks";
import { useNav, useShell } from "../../../lib/nav";
import { depId, fileId } from "../../../lib/resource";
import type { SymbolRef } from "./SourceView";

type PaneTab = "outline" | "usages" | "deps";

export interface CodePaneProps {
  ws: string;
  projectKey: string;
  path: string;
  /** The provider's display data for this file. Null while loading. */
  file: CodeFile | null;
  fileError: Error | null;
  /** The symbol whose usages the Usages tab shows. */
  selected: SymbolRef | null;
  onSelect: (s: SymbolRef | null) => void;
  /** Scroll this file to a line. */
  onGoto: (line: number) => void;
}

const KIND_ICON: Record<SymbolKind, IconName> = {
  function: "braces",
  method: "braces",
  class: "box",
  enum: "list",
  interface: "box",
  type: "box",
  module: "folder",
  constant: "minus",
  variable: "minus",
  field: "minus",
  macro: "sparkles",
};

const TABS: TabItem[] = [
  { id: "outline", label: "Outline", icon: "list-tree" },
  { id: "usages", label: "Usages", icon: "text-search" },
  { id: "deps", label: "Dependencies", icon: "git-fork" },
];

const rowStyle = {
  display: "flex",
  gap: 6,
  alignItems: "baseline",
  width: "100%",
  padding: "3px 10px",
  background: "none",
  border: 0,
  font: "inherit",
  color: "var(--text-primary)",
  textAlign: "left",
  cursor: "pointer",
} as const;

const previewStyle = {
  font: "var(--text-xs)/1.4 var(--font-mono)",
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
  minWidth: 0,
  flex: 1,
} as const;
const mutedStyle = {
  color: "var(--text-muted)",
  fontSize: "var(--text-xs)",
} as const;
const groupStyle = {
  padding: "8px 10px 2px",
  font: "var(--text-2xs)/1.3 var(--font-mono)",
  color: "var(--text-secondary)",
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
} as const;

function Loading({ children }: { children: ReactNode }) {
  return (
    <div
      style={{
        padding: 12,
        display: "flex",
        gap: 8,
        alignItems: "center",
        color: "var(--text-muted)",
      }}
    >
      <Spinner size={11} /> {children}
    </div>
  );
}

const Empty = ({ children }: { children: ReactNode }) => <div style={{ padding: 12, ...mutedStyle }}>{children}</div>;

/**
 * Right-side pane of the file view: the file's outline, the usages of the selected symbol across the project, and
 * the file's imports and importers. Everything comes from the project's code provider, built in or external.
 */
export function CodePane({ ws, projectKey, path, file, fileError, selected, onSelect, onGoto }: CodePaneProps) {
  const [tab, setTab] = useState<PaneTab>("outline");
  useEffect(() => {
    if (selected) setTab("usages");
  }, [selected]);

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        minHeight: 0,
        height: "100%",
      }}
    >
      <div
        style={{
          padding: "4px 8px 0",
          borderBottom: "1px solid var(--border-subtle)",
          flex: "none",
        }}
      >
        <Tabs label="Code navigation" value={tab} onChange={(v) => setTab(v as PaneTab)} tabs={TABS} />
      </div>
      <div
        style={{
          flex: 1,
          overflow: "auto",
          minHeight: 0,
          fontSize: "var(--text-sm)",
        }}
      >
        {tab === "outline" && <Outline file={file} error={fileError} onGoto={onGoto} onSelect={onSelect} />}
        {tab === "usages" && (
          <Usages ws={ws} projectKey={projectKey} path={path} selected={selected} onSelect={onSelect} onGoto={onGoto} />
        )}
        {tab === "deps" && <Deps ws={ws} projectKey={projectKey} path={path} onGoto={onGoto} />}
      </div>
      <div
        style={{
          flex: "none",
          padding: "0 10px",
          height: 24,
          display: "flex",
          alignItems: "center",
          borderTop: "1px solid var(--border-subtle)",
          font: "var(--text-2xs)/1 var(--font-mono)",
          color: "var(--text-muted)",
        }}
      >
        {file ? `provider ${file.provider}${file.language ? `, ${file.language}` : ""}` : "provider"}
      </div>
    </div>
  );
}

function Outline({
  file,
  error,
  onGoto,
  onSelect,
}: {
  file: CodeFile | null;
  error: Error | null;
  onGoto: (line: number) => void;
  onSelect: (s: SymbolRef) => void;
}) {
  if (error)
    return (
      <Banner tone="bad" style={{ margin: 10 }}>
        {error.message}
      </Banner>
    );
  if (!file) return <Loading>Reading symbols…</Loading>;
  return (
    <>
      {file.warning && (
        <Banner tone="warn" style={{ margin: 10 }}>
          {file.warning}
        </Banner>
      )}
      {file.symbols.length === 0 ? (
        <Empty>
          {file.language
            ? "This file defines no symbols the provider found."
            : "The provider does not know this file type."}
        </Empty>
      ) : (
        <div style={{ padding: "4px 0" }}>
          {file.symbols.map((s, i) => (
            <SymbolRow key={i} s={s} onGoto={onGoto} onSelect={onSelect} />
          ))}
        </div>
      )}
    </>
  );
}

function SymbolRow({
  s,
  onGoto,
  onSelect,
}: {
  s: CodeSymbol;
  onGoto: (line: number) => void;
  onSelect: (s: SymbolRef) => void;
}) {
  return (
    <div style={{ display: "flex", alignItems: "center" }}>
      <button
        type="button"
        style={{ ...rowStyle, paddingLeft: s.container ? 26 : 10 }}
        onClick={() => onGoto(s.line)}
        title={`${s.kind}, line ${s.line}`}
      >
        <Icon
          name={KIND_ICON[s.kind]}
          size={12}
          style={{
            color: "var(--text-muted)",
            flex: "none",
            alignSelf: "center",
          }}
        />
        <span style={{ ...previewStyle, fontSize: "var(--text-sm)" }}>{s.name}</span>
        <span style={mutedStyle}>{s.line}</span>
      </button>
      <button
        type="button"
        style={{
          ...rowStyle,
          width: "auto",
          padding: "3px 8px",
          color: "var(--text-muted)",
        }}
        title="Find usages"
        aria-label={`Find usages of ${s.name}`}
        onClick={() => onSelect({ name: s.name, line: s.line, col: s.col })}
      >
        <Icon name="text-search" size={12} />
      </button>
    </div>
  );
}

/** Group locations by file, keeping the order the provider sent. */
function byFile(locs: CodeLocation[]): [string, CodeLocation[]][] {
  const groups = new Map<string, CodeLocation[]>();
  for (const l of locs) {
    const g = groups.get(l.path);
    if (g) g.push(l);
    else groups.set(l.path, [l]);
  }
  return [...groups];
}

function useOpenLocation(projectKey: string, path: string, onGoto: (line: number) => void, symbol?: SymbolRef | null) {
  const nav = useNav();
  return (p: string, line: number, carry = symbol, uri?: string | null) => {
    if (uri) return nav.open(depId(projectKey, uri), { anchor: `L${line}`, beside: true });
    if (p === path) return onGoto(line);
    if (carry) carrySymbol(projectKey, p, carry);
    nav.open(fileId(projectKey, p), { anchor: `L${line}`, beside: true });
  };
}

function LocationList({
  title,
  locs,
  open,
}: {
  title: string;
  locs: CodeLocation[];
  open: (path: string, line: number, carry?: SymbolRef | null, uri?: string | null) => void;
}) {
  if (locs.length === 0) return null;
  return (
    <div style={{ paddingBottom: 6 }}>
      <div style={{ padding: "8px 10px 0", fontWeight: 500 }}>
        {title} <span style={mutedStyle}>{locs.length}</span>
      </div>
      {byFile(locs).map(([p, ls]) => (
        <div key={p}>
          <div style={groupStyle} title={p}>
            {p}
          </div>
          {ls.map((l, i) => (
            <button
              key={i}
              type="button"
              style={rowStyle}
              onClick={() => open(l.path, l.line, undefined, l.uri)}
              title={`${l.path}:${l.line}`}
            >
              <span
                style={{
                  ...mutedStyle,
                  flex: "none",
                  minWidth: 28,
                  textAlign: "right",
                }}
              >
                {l.line}
              </span>
              <span style={previewStyle}>{l.preview}</span>
            </button>
          ))}
        </div>
      ))}
    </div>
  );
}

function Usages({
  ws,
  projectKey,
  path,
  selected,
  onSelect,
  onGoto,
}: {
  ws: string;
  projectKey: string;
  path: string;
  selected: SymbolRef | null;
  onSelect: (s: SymbolRef | null) => void;
  onGoto: (line: number) => void;
}) {
  const [query, setQuery] = useState("");
  const [debounced, setDebounced] = useState("");
  useEffect(() => {
    const t = setTimeout(() => setDebounced(query.trim()), 200);
    return () => clearTimeout(t);
  }, [query]);
  const open = useOpenLocation(projectKey, path, onGoto, selected);
  const found = useAsync(
    () => (debounced ? api.codeSymbols(ws, projectKey, debounced, 30) : Promise.resolve(null)),
    [ws, projectKey, debounced],
  );
  const usages = useAsync(
    () =>
      selected
        ? api.codeUsages(ws, projectKey, selected.name, {
            path,
            line: selected.line,
            col: selected.col,
          })
        : Promise.resolve(null),
    [ws, projectKey, selected?.name, selected?.line, selected?.col],
  );
  const u = usages.data;

  return (
    <>
      <div style={{ padding: "8px 10px 4px" }}>
        <Input
          size="sm"
          icon="search"
          placeholder="Find a symbol"
          aria-label="Find a symbol"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>
      {debounced ? (
        found.error ? (
          <Banner tone="bad" style={{ margin: 10 }}>
            {found.error.message}
          </Banner>
        ) : !found.data ? (
          <Loading>Searching symbols…</Loading>
        ) : found.data.items.length === 0 ? (
          <Empty>No symbol matches.</Empty>
        ) : (
          <div style={{ paddingBottom: 6 }}>
            {found.data.items.map((l, i) => (
              <button
                key={i}
                type="button"
                style={rowStyle}
                title={`${l.path}:${l.line}`}
                onClick={() => {
                  const sym = {
                    name: l.name,
                    line: l.line,
                    col: l.col,
                  };
                  setQuery("");
                  if (l.path === path && !l.uri) onSelect(sym);
                  open(l.path, l.line, sym, l.uri);
                }}
              >
                {l.kind && (
                  <Icon
                    name={KIND_ICON[l.kind]}
                    size={12}
                    style={{
                      color: "var(--text-muted)",
                      flex: "none",
                      alignSelf: "center",
                    }}
                  />
                )}
                <span style={{ ...previewStyle, flex: "none", maxWidth: "50%" }}>
                  {l.container ? `${l.container}::` : ""}
                  {l.name}
                </span>
                <span style={{ ...previewStyle, ...mutedStyle }}>{l.path}</span>
              </button>
            ))}
            {found.data.truncated && <Empty>More symbols match. Type more of the name.</Empty>}
          </div>
        )
      ) : !selected ? (
        <Empty>Click a name in the file, or find a symbol above, to list where it is defined and used.</Empty>
      ) : usages.error ? (
        <Banner
          tone="bad"
          style={{ margin: 10 }}
          actions={
            <Button size="sm" onClick={usages.reload}>
              Try again
            </Button>
          }
        >
          {usages.error.message}
        </Banner>
      ) : !u || u.symbol !== selected.name ? (
        <Loading>Finding usages of {selected.name}…</Loading>
      ) : (
        <>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 6,
              padding: "4px 10px",
            }}
          >
            <span
              style={{
                font: "var(--text-sm)/1.3 var(--font-mono)",
                fontWeight: 600,
                overflow: "hidden",
                textOverflow: "ellipsis",
              }}
            >
              {u.symbol}
            </span>
            <span style={mutedStyle}>
              {u.definitions.length} {u.definitions.length === 1 ? "definition" : "definitions"}, {u.references.length}{" "}
              {u.references.length === 1 ? "use" : "uses"}
            </span>
            <span style={{ flex: 1 }} />
            <button
              type="button"
              style={{
                ...rowStyle,
                width: "auto",
                padding: 2,
                color: "var(--text-muted)",
              }}
              aria-label="Clear the symbol"
              title="Clear the symbol"
              onClick={() => onSelect(null)}
            >
              <Icon name="x" size={12} />
            </button>
          </div>
          {u.warning && (
            <Banner tone="warn" style={{ margin: "4px 10px" }}>
              {u.warning}
            </Banner>
          )}
          {u.definitions.length === 0 && u.references.length === 0 && (
            <Empty>The provider found no definition or use of this name.</Empty>
          )}
          <LocationList title="Definitions" locs={u.definitions} open={open} />
          <LocationList title="Uses" locs={u.references} open={open} />
          {u.truncated && <Empty>The list stops at the result cap.</Empty>}
        </>
      )}
    </>
  );
}

let carried: { key: string; path: string; symbol: SymbolRef } | null = null;

/** Hand the selected symbol to the file screen that opens next, so its Usages tab keeps the list. */
export function carrySymbol(key: string, path: string, symbol: SymbolRef) {
  carried = { key, path, symbol };
}

/** The symbol carried to this file, once. */
export function takeCarried(key: string, path: string): SymbolRef | null {
  const c = carried;
  carried = null;
  return c && c.key === key && c.path === path ? c.symbol : null;
}

function Deps({
  ws,
  projectKey,
  path,
  onGoto,
}: {
  ws: string;
  projectKey: string;
  path: string;
  onGoto: (line: number) => void;
}) {
  const shell = useShell();
  const deps = useAsync(() => api.codeDeps(ws, projectKey, path), [ws, projectKey, path]);
  const open = useOpenLocation(projectKey, path, onGoto);
  const d = deps.data;
  const imports = useMemo(() => d?.imports ?? [], [d]);
  if (deps.error)
    return (
      <Banner tone="bad" style={{ margin: 10 }}>
        {deps.error.message}
      </Banner>
    );
  if (!d) return <Loading>Reading dependencies…</Loading>;
  return (
    <>
      {d.warning && (
        <Banner tone="warn" style={{ margin: 10 }}>
          {d.warning}
        </Banner>
      )}
      <div style={{ padding: "8px 10px 0", fontWeight: 500 }}>
        Imports <span style={mutedStyle}>{imports.length}</span>
      </div>
      {imports.length === 0 && <Empty>This file names no imports the provider found.</Empty>}
      {imports.map((im, i) => (
        <div key={i} style={{ display: "flex", alignItems: "center" }}>
          <button type="button" style={rowStyle} onClick={() => onGoto(im.line)} title={`Line ${im.line}`}>
            <span
              style={{
                ...mutedStyle,
                flex: "none",
                minWidth: 28,
                textAlign: "right",
              }}
            >
              {im.line}
            </span>
            <span style={previewStyle}>{im.spec}</span>
          </button>
          {im.target ? (
            <button
              type="button"
              style={{
                ...rowStyle,
                width: "auto",
                maxWidth: "50%",
                padding: "3px 10px 3px 0",
                color: "var(--accent-fg)",
              }}
              title={im.folder ? `Show the folder ${im.target}` : `Open ${im.target}`}
              onClick={() => (im.folder ? shell.browseFiles(projectKey) : open(im.target!, 1))}
            >
              <Icon name="corner-down-right" size={12} style={{ flex: "none", alignSelf: "center" }} />
              <span style={previewStyle}>{im.target}</span>
            </button>
          ) : (
            <span style={{ ...mutedStyle, paddingRight: 10, flex: "none" }}>external</span>
          )}
        </div>
      ))}
      <div style={{ padding: "12px 10px 0", fontWeight: 500 }}>
        Imported by <span style={mutedStyle}>{d.importers.length}</span>
      </div>
      {d.importers.length === 0 && <Empty>No project file imports this one.</Empty>}
      {byFile(
        d.importers.map((im) => ({
          name: "",
          path: im.path,
          line: im.line,
          col: 0,
          len: 0,
          preview: im.spec,
        })),
      ).map(([p, ls]) => (
        <div key={p}>
          <div style={groupStyle} title={p}>
            {p}
          </div>
          {ls.map((l, i) => (
            <button
              key={i}
              type="button"
              style={rowStyle}
              onClick={() => open(l.path, l.line, undefined, l.uri)}
              title={`${l.path}:${l.line}`}
            >
              <span
                style={{
                  ...mutedStyle,
                  flex: "none",
                  minWidth: 28,
                  textAlign: "right",
                }}
              >
                {l.line}
              </span>
              <span style={previewStyle}>{l.preview}</span>
            </button>
          ))}
        </div>
      ))}
      {d.truncated && <Empty>The list stops at the result cap.</Empty>}
    </>
  );
}
