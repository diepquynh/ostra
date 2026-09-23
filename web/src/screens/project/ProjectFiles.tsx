import { Fragment, useEffect, useMemo, useState, type ReactNode } from "react";
import { api } from "../../api";
import type { ProjectTreeEntry, ProjectView } from "../../api/types";
import { Banner, Breadcrumbs, Button, CodeView, Icon, IconButton, Input, Spinner, Table, TreeItem } from "../../design";
import { useAsync } from "../../lib/hooks";
import { useFileIndex, useProjectFsChanges } from "../../lib/live";
import { useNav } from "../../lib/nav";
import { fileId } from "../../lib/resource";
import { entryIcon, extOf, fileIcon, fmtSize, GIT_MARK, isHiddenPath, kindOf, modifiedLabel, parentOf, parentsOf, sortEntries, toggleSort, type Sort, type SortKey } from "./files";
import { useFolders, type Folder } from "./useFolders";

const MAX_MATCHES = 200;

type Selection = { path: string; dir: boolean };

const muted = { color: "var(--text-muted)", fontSize: "var(--text-sm)" };

function Note({ children, spin }: { children: ReactNode; spin?: boolean }) {
  return (
    <div style={{ padding: "12px 8px", display: "flex", gap: 6, alignItems: "center", ...muted }}>
      {spin && <Spinner size={10} />}
      {children}
    </div>
  );
}

function GitLetter({ e }: { e: Pick<ProjectTreeEntry, "git"> }) {
  if (!e.git) return null;
  const m = GIT_MARK[e.git];
  return <span style={{ color: m.color, fontWeight: 600, fontFamily: "var(--font-mono)" }}>{m.letter}</span>;
}

/** The Files tab: folder tree with find and dotfiles, and a right pane with the selected folder or file. */
export function ProjectFiles({ ws, project }: { ws: string; project: ProjectView }) {
  const key = project.key;
  const nav = useNav();
  const [hidden, setHidden] = useState(false);
  const [filter, setFilter] = useState("");
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const [sel, setSel] = useState<Selection>({ path: "", dir: true });
  const { folders, ensure } = useFolders(ws, key, hidden);

  const listed = sel.dir ? sel.path : parentOf(sel.path);
  const openDirs = Object.keys(open).filter((d) => open[d]);
  useEffect(() => ensure(["", listed, ...openDirs]), [ensure, listed, openDirs.join("\n")]); // eslint-disable-line react-hooks/exhaustive-deps

  const reveal = (path: string, dir: boolean) => {
    const up = dir ? [...parentsOf(path), path].filter(Boolean) : parentsOf(path);
    setOpen((o) => ({ ...o, ...Object.fromEntries(up.map((d) => [d, true])) }));
    setSel({ path, dir });
  };

  const index = useFileIndex(ws, filter.trim() ? key : null);
  const q = filter.trim().toLowerCase();
  const matches = useMemo(() => {
    if (!q) return [];
    const out: string[] = [];
    for (const p of index.paths) {
      if (p.toLowerCase().includes(q) && (hidden || !isHiddenPath(p))) out.push(p);
      if (out.length >= MAX_MATCHES) break;
    }
    return out;
  }, [q, index.paths, hidden]);

  const renderDir = (dir: string, depth: number): ReactNode => {
    const f = folders[dir];
    if (!f || (!f.entries && !f.error))
      return (
        <div style={{ paddingLeft: 26 + depth * 14, height: "var(--row-h)", display: "flex", alignItems: "center", gap: 6, ...muted }}>
          <Spinner size={10} /> Reading…
        </div>
      );
    if (f.error && !f.entries) return <div style={{ paddingLeft: 26 + depth * 14, color: "var(--bad)", fontSize: "var(--text-sm)" }}>{f.error}</div>;
    const entries = sortEntries(f.entries ?? [], { key: "name", dir: 1 });
    if (entries.length === 0 && depth === 0) return <Note>This project is empty.</Note>;
    return entries.map((e) => {
      const dim = e.ignored ? { color: "var(--text-muted)" } : undefined;
      if (e.is_dir) {
        const isOpen = !!open[e.path];
        return (
          <Fragment key={e.path}>
            <TreeItem
              depth={depth}
              label={<span style={dim}>{e.name}</span>}
              icon={entryIcon(e, isOpen)}
              expanded={isOpen}
              selected={sel.dir && sel.path === e.path}
              trailing={!isOpen && e.has_changes ? <span className="os-dot os-dot--warn" style={{ width: 5, height: 5 }} /> : null}
              onToggle={() => setOpen((o) => ({ ...o, [e.path]: !o[e.path] }))}
              onClick={() => {
                setOpen((o) => ({ ...o, [e.path]: true }));
                setSel({ path: e.path, dir: true });
              }}
              title={e.path + (e.ignored ? " · ignored" : "")}
            />
            {isOpen && renderDir(e.path, depth + 1)}
          </Fragment>
        );
      }
      const mark = e.git ? GIT_MARK[e.git] : null;
      return (
        <TreeItem
          key={e.path}
          depth={depth}
          label={<span style={mark ? { color: mark.color } : dim}>{e.name}</span>}
          icon={fileIcon(e.name)}
          selected={!sel.dir && sel.path === e.path}
          onClick={() => setSel({ path: e.path, dir: false })}
          meta={<GitLetter e={e} />}
          title={e.path + (mark ? ` · ${mark.word}` : "")}
        />
      );
    });
  };

  return (
    <div style={{ display: "flex", flex: 1, minHeight: 0, borderTop: "1px solid var(--border-subtle)" }}>
      <div style={{ width: 280, flex: "none", display: "flex", flexDirection: "column", minHeight: 0, borderRight: "1px solid var(--border-subtle)", background: "var(--surface-panel)" }}>
        <div style={{ display: "flex", gap: 4, padding: 8 }}>
          <div style={{ flex: 1, minWidth: 0 }}>
            <Input size="sm" icon="search" placeholder="Find a file" aria-label="Find a file" value={filter} onChange={(e) => setFilter(e.target.value)} />
          </div>
          <IconButton size="sm" icon={hidden ? "eye" : "eye-off"} label={hidden ? "Hide dotfiles" : "Show dotfiles"} active={hidden} onClick={() => setHidden(!hidden)} />
          <IconButton size="sm" icon="chevrons-down-up" label="Collapse all" onClick={() => setOpen({})} />
        </div>
        <div role="tree" aria-label={`Files in ${key}`} style={{ flex: 1, overflowY: "auto", overflowX: "hidden", padding: "0 8px 8px" }}>
          {q ? (
            index.loading && index.paths.length === 0 ? (
              <Note spin>Reading the file list…</Note>
            ) : matches.length ? (
              matches.map((p) => (
                <TreeItem
                  key={p}
                  label={p.split("/").pop()}
                  meta={parentOf(p)}
                  icon={fileIcon(p)}
                  selected={!sel.dir && sel.path === p}
                  onClick={() => reveal(p, false)}
                  title={p}
                />
              ))
            ) : (
              <Note>No file matches.</Note>
            )
          ) : (
            renderDir("", 0)
          )}
        </div>
      </div>
      <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column", minHeight: 0 }}>
        <PathBar project={key} path={sel.path} onNavigate={(p) => reveal(p, true)}>
          {!sel.dir && (
            <Button size="sm" variant="ghost" iconRight="arrow-right" onClick={() => nav.open(fileId(key, sel.path))}>
              Open in a tab
            </Button>
          )}
        </PathBar>
        {sel.dir ? (
          <FolderListing folder={folders[sel.path]} folders={folders} hidden={hidden} onOpen={(e) => (e.is_dir ? reveal(e.path, true) : setSel({ path: e.path, dir: false }))} />
        ) : (
          <FilePreview ws={ws} project={key} path={sel.path} onOpenTab={() => nav.open(fileId(key, sel.path))} />
        )}
      </div>
    </div>
  );
}

export function PathBar({ project, path, onNavigate, children }: { project: string; path: string; onNavigate: (path: string) => void; children?: ReactNode }) {
  const parts = path ? path.split("/") : [];
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 6, height: 36, padding: "0 8px", borderBottom: "1px solid var(--border-subtle)", flex: "none" }}>
      <IconButton size="sm" icon="arrow-up" label="Parent folder" disabled={!path} onClick={() => onNavigate(parts.slice(0, -1).join("/"))} />
      <div style={{ flex: 1, minWidth: 0, overflow: "hidden" }}>
        <Breadcrumbs
          onNavigate={(_, i) => onNavigate(i === 0 ? "" : parts.slice(0, i).join("/"))}
          items={[{ label: project, icon: "folder-git-2" }, ...parts.map((label) => ({ label }))]}
        />
      </div>
      {children}
    </div>
  );
}

type Row = ProjectTreeEntry & { items: number | null };

function FolderListing({ folder, folders, hidden, onOpen }: { folder: Folder | undefined; folders: Record<string, Folder>; hidden: boolean; onOpen: (e: ProjectTreeEntry) => void }) {
  const [sort, setSort] = useState<Sort>({ key: "name", dir: 1 });
  if (!folder || (!folder.entries && !folder.error)) return <Note spin>Reading the folder…</Note>;
  if (folder.error && !folder.entries)
    return (
      <div style={{ padding: 16 }}>
        <Banner tone="bad">{folder.error}</Banner>
      </div>
    );
  const rows: Row[] = sortEntries(folder.entries ?? [], sort).map((e) => ({ ...e, items: e.is_dir ? (folders[e.path]?.entries?.length ?? null) : null }));
  const head = (key: SortKey, label: string) => (
    <span
      role="button"
      tabIndex={0}
      aria-label={`Sort by ${label.toLowerCase()}`}
      style={{ display: "inline-flex", alignItems: "center", gap: 4, cursor: "pointer" }}
      onClick={() => setSort((s) => toggleSort(s, key))}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          setSort((s) => toggleSort(s, key));
        }
      }}
    >
      {label}
      {sort.key === key && <Icon name={sort.dir > 0 ? "arrow-up" : "arrow-down"} size={11} />}
    </span>
  );
  return (
    <div className="os-rise" style={{ flex: 1, overflow: "auto", minHeight: 0 }}>
      <Table<Row>
        dense
        rowKey="path"
        onRowClick={onOpen}
        empty={hidden ? "This folder is empty." : "This folder is empty, or holds only dotfiles."}
        rows={rows}
        columns={[
          {
            key: "name",
            label: head("name", "Name"),
            render: (r) => {
              const mark = r.git ? GIT_MARK[r.git] : null;
              return (
                <span style={{ display: "inline-flex", alignItems: "center", gap: 8 }} title={r.path + (mark ? ` · ${mark.word}` : "")}>
                  <Icon name={entryIcon(r)} size={14} style={{ color: r.is_dir ? "var(--accent-fg)" : "var(--text-muted)" }} />
                  <span style={{ color: mark ? mark.color : r.ignored ? "var(--text-muted)" : undefined }}>{r.name}</span>
                  <GitLetter e={r} />
                  {r.is_dir && r.has_changes && <span className="os-dot os-dot--warn" style={{ width: 5, height: 5 }} title="Holds changed files" />}
                </span>
              );
            },
          },
          { key: "kind", label: head("kind", "Kind"), render: (r) => <span style={muted}>{kindOf(r)}</span> },
          {
            key: "size",
            label: head("size", "Size"),
            num: true,
            render: (r) => (r.is_dir ? <span style={{ color: "var(--text-muted)" }}>{r.items === null ? "" : `${r.items} items`}</span> : fmtSize(r.size)),
          },
          {
            key: "modified",
            label: head("modified", "Modified"),
            render: (r) => (
              <span style={{ ...muted, whiteSpace: "nowrap" }} title={r.modified ?? undefined}>
                {modifiedLabel(r.modified)}
              </span>
            ),
          },
        ]}
      />
    </div>
  );
}

function FilePreview({ ws, project, path, onOpenTab }: { ws: string; project: string; path: string; onOpenTab: () => void }) {
  const file = useAsync(() => api.projectFile(ws, project, path), [ws, project, path]);
  useProjectFsChanges(ws, project, (paths) => paths.includes(path) && file.reload());
  const f = file.data?.path === path ? file.data : null;
  const lines = f?.content ? f.content.replace(/\n$/, "").split("\n").length : null;
  const ext = extOf(path);
  return (
    <div className="os-rise" key={path} style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
      <div style={{ flex: 1, overflow: "auto", minHeight: 0 }}>
        {file.error ? (
          <div style={{ padding: 16 }}>
            <Banner tone="bad" actions={<Button size="sm" onClick={file.reload}>Try again</Button>}>
              {file.error.message}
            </Banner>
          </div>
        ) : !f ? (
          <Note spin>Reading the file…</Note>
        ) : f.binary ? (
          <div style={{ padding: 20, color: "var(--text-muted)" }}>Binary file, {fmtSize(f.size)}. No preview.</div>
        ) : (
          <>
            {f.truncated && (
              <div style={{ padding: "8px 12px 0" }}>
                <Banner tone="info" actions={<Button size="sm" onClick={onOpenTab}>Open in a tab</Button>}>
                  Showing the start of the file. It is larger than the preview limit.
                </Banner>
              </div>
            )}
            <CodeView flush language={ext} code={f.content ?? ""} />
          </>
        )}
      </div>
      {f && (
        <div
          style={{
            display: "flex",
            gap: 14,
            height: 24,
            alignItems: "center",
            padding: "0 12px",
            borderTop: "1px solid var(--border-subtle)",
            font: "var(--text-2xs)/1 var(--font-mono)",
            color: "var(--text-muted)",
            flex: "none",
            whiteSpace: "nowrap",
            overflow: "hidden",
          }}
        >
          <span>{f.binary ? "binary" : ext || "text"}</span>
          {lines !== null && <span>{lines} lines</span>}
          <span>{fmtSize(f.size)}</span>
          {f.modified && <span>modified {modifiedLabel(f.modified)}</span>}
          {f.git && <span style={{ color: GIT_MARK[f.git].color }}>{GIT_MARK[f.git].word}</span>}
          <span style={{ flex: 1 }} />
          <span>read-only</span>
        </div>
      )}
    </div>
  );
}
