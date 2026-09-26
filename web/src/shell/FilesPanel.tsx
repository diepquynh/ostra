import { Button, Icon, IconButton, type IconName, Input, Select, Spinner, TreeItem } from "@ostra/design";
import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { GitMark, ProjectTreeEntry, ProjectView } from "../api/types";
import { setContextDrag } from "../features/context/tags";
import { humanize } from "../lib/format";
import { useFileIndex, useProjectChanges, useProjectFsChanges } from "../lib/live";
import { throttle } from "../lib/store";

const FILE_ICON: Record<string, IconName> = {
  rs: "file-code-2",
  ts: "file-code-2",
  tsx: "file-code-2",
  js: "file-code-2",
  jsx: "file-code-2",
  py: "file-code-2",
  go: "file-code-2",
  java: "file-code-2",
  kt: "file-code-2",
  sql: "database",
  toml: "file-cog",
  yaml: "file-cog",
  yml: "file-cog",
  json: "file-json",
  md: "file-text",
  txt: "file-text",
};

export const fileIcon = (path: string): IconName => FILE_ICON[path.split(".").pop()?.toLowerCase() ?? ""] ?? "file";

const MARK: Record<GitMark, { letter: string; color: string; word: string }> = {
  M: { letter: "M", color: "var(--warn)", word: "modified" },
  A: { letter: "A", color: "var(--ok)", word: "added" },
  "?": { letter: "U", color: "var(--ok)", word: "untracked" },
  D: { letter: "D", color: "var(--bad)", word: "deleted" },
  R: { letter: "R", color: "var(--info)", word: "renamed" },
};

const MAX_MATCHES = 200;

type Folder = { entries: ProjectTreeEntry[] | null; error: string | null };

type Creating = { kind: "file" | "folder"; dir: string };

const parentOf = (path: string) => path.split("/").slice(0, -1).join("/");

/** The inline name field for a new file or folder inside `dir`. Enter creates, Escape or an empty blur cancels. */
function NewEntry({
  creating,
  depth,
  onCreate,
  onCancel,
}: {
  creating: Creating;
  depth: number;
  onCreate: (name: string) => Promise<void>;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const submit = () => {
    const n = name.trim().replace(/^\/+|\/+$/g, "");
    if (!n) return onCancel();
    setBusy(true);
    onCreate(n).catch((e: Error) => {
      setError(e.message);
      setBusy(false);
    });
  };
  return (
    <div style={{ paddingLeft: 8 + depth * 14, paddingBlock: 2 }}>
      <Input
        size="sm"
        mono
        autoFocus
        icon={creating.kind === "file" ? "file-plus" : "folder-plus"}
        aria-label={creating.kind === "file" ? "New file name" : "New folder name"}
        placeholder={creating.kind === "file" ? "name.ext or folder/name.ext" : "folder or folder/sub"}
        value={name}
        disabled={busy}
        error={error ?? undefined}
        onChange={(e) => {
          setName(e.target.value);
          setError(null);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") submit();
          if (e.key === "Escape") onCancel();
        }}
        onBlur={() => !name.trim() && !busy && onCancel()}
      />
    </div>
  );
}

type FilesPanelProps = {
  ws: string;
  projects: ProjectView[];
  project: string | null;
  setProject: (key: string) => void;
  selected: { key: string; path: string } | null;
  /** `edit` opens the file in edit mode in a normal tab, for a file just created. */
  onOpenFile: (key: string, path: string, opts?: { edit?: boolean }) => void;
  onOpenProject: (key: string) => void;
  onAddProject: () => void;
};

const parents = (path: string) => {
  const segs = path.split("/");
  return segs.slice(0, -1).map((_, i) => segs.slice(0, i + 1).join("/"));
};

/** Left-dock Files tab: project picker, find, dotfiles, collapse all, the lazy folder tree, and "Changed by sessions". */
export function FilesPanel({
  ws,
  projects,
  project,
  setProject,
  selected,
  onOpenFile,
  onOpenProject,
  onAddProject,
}: FilesPanelProps) {
  const key = project && projects.some((p) => p.key === project) ? project : (projects[0]?.key ?? null);
  const [showHidden, setShowHidden] = useState(false);
  const [filter, setFilter] = useState("");
  const [creating, setCreating] = useState<Creating | null>(null);
  /** The folder new entries go into: the last folder toggled, else the open file's folder. */
  const [lastDir, setLastDir] = useState<string | null>(null);
  const [openDirs, setOpenDirs] = useState<Record<string, Record<string, boolean>>>({});
  const loading = useRef(new Set<string>());
  const open = (key && openDirs[key]) || {};
  const setOpen = useCallback(
    (fn: (o: Record<string, boolean>) => Record<string, boolean>) =>
      key && setOpenDirs((m) => ({ ...m, [key]: fn(m[key] ?? {}) })),
    [key],
  );

  // Cached listings belong to one project and one dotfiles setting.
  const cacheId = `${key}\n${showHidden}`;
  const [cache, setCache] = useState<{ id: string; map: Record<string, Folder> }>({ id: cacheId, map: {} });
  const folders = cache.id === cacheId ? cache.map : {};
  const cacheRef = useRef(cacheId);
  if (cacheRef.current !== cacheId) {
    cacheRef.current = cacheId;
    loading.current.clear();
  }
  const setFolder = useCallback((id: string, dir: string, fn: (prev: Folder | undefined) => Folder) => {
    setCache((c) =>
      c.id === id ? { id, map: { ...c.map, [dir]: fn(c.map[dir]) } } : { id, map: { [dir]: fn(undefined) } },
    );
  }, []);

  const load = useCallback(
    (dir: string) => {
      if (!key) return;
      const id = cacheId;
      loading.current.add(dir);
      api.projectTree(ws, key, { path: dir, depth: 1, hidden: showHidden }).then(
        (t) => {
          if (cacheRef.current !== id) return;
          loading.current.delete(dir);
          setFolder(id, dir, () => ({ entries: t.entries, error: null }));
        },
        (e: Error) => {
          if (cacheRef.current !== id) return;
          loading.current.delete(dir);
          setFolder(id, dir, (prev) => ({ entries: prev?.entries ?? null, error: e.message }));
        },
      );
    },
    [ws, key, showHidden, cacheId, setFolder],
  );

  // Load the root and every open folder that has no listing yet.
  useEffect(() => {
    if (!key) return;
    for (const dir of ["", ...Object.keys(open).filter((d) => open[d])])
      if (!folders[dir] && !loading.current.has(dir)) load(dir);
  }, [key, open, folders, load]);

  // Reveal the file the active tab shows: switch to its project, open its folders, and scroll to it once it renders.
  const treeRef = useRef<HTMLDivElement>(null);
  const [scrollTo, setScrollTo] = useState<string | null>(null);
  const reveal = (clearFilter: boolean) => {
    if (!selected) return;
    if (selected.key !== key) setProject(selected.key);
    if (clearFilter) setFilter("");
    const dirs = parents(selected.path);
    setOpenDirs((m) => {
      const o = m[selected.key] ?? {};
      return dirs.every((d) => o[d])
        ? m
        : { ...m, [selected.key]: { ...o, ...Object.fromEntries(dirs.map((d) => [d, true])) } };
    });
    setScrollTo(`${selected.key}:${selected.path}`);
  };
  useEffect(() => {
    reveal(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected?.key, selected?.path]);
  useEffect(() => {
    if (!scrollTo || !selected || scrollTo !== `${key}:${selected.path}`) return;
    const el = treeRef.current?.querySelector('[aria-selected="true"]');
    if (!el) return;
    el.scrollIntoView?.({ block: "nearest" });
    setScrollTo(null);
  });

  // Files changed on disk: reload the loaded listings, at most twice a second.
  const loadedRef = useRef<() => void>(() => {});
  loadedRef.current = () => Object.keys(folders).forEach((d) => load(d));
  const refresh = useMemo(() => throttle(() => loadedRef.current(), 500), []);
  useEffect(() => refresh.cancel, [refresh]);
  useProjectFsChanges(ws, key, refresh);

  const startCreate = (kind: Creating["kind"]) => {
    const dir = lastDir ?? (selected?.key === key ? parentOf(selected.path) : "");
    setFilter("");
    if (dir) setOpen((o) => ({ ...o, ...Object.fromEntries([...parents(`${dir}/x`), dir].map((d) => [d, true])) }));
    setCreating({ kind, dir });
  };

  const create = async (name: string) => {
    if (!key || !creating) return;
    const path = creating.dir ? `${creating.dir}/${name}` : name;
    if (creating.kind === "file") {
      await api.saveProjectFile(ws, key, { path, content: "", base_hash: null });
      onOpenFile(key, path, { edit: true });
    } else {
      await api.createProjectFolder(ws, key, path);
      setOpen((o) => ({ ...o, ...Object.fromEntries([...parents(path), path].map((d) => [d, true])) }));
      setLastDir(path);
    }
    setCreating(null);
    for (const d of new Set([creating.dir, ...parents(path)])) load(d);
  };

  const index = useFileIndex(ws, filter.trim() ? key : null);
  const { changes } = useProjectChanges(ws, key);
  const q = filter.trim().toLowerCase();
  const matches = useMemo(() => {
    if (!q) return [];
    const hiddenOk = (p: string) => showHidden || !p.split("/").some((s) => s.startsWith("."));
    const out: string[] = [];
    for (const p of index.paths) {
      if (p.toLowerCase().includes(q) && hiddenOk(p)) out.push(p);
      if (out.length >= MAX_MATCHES) break;
    }
    return out;
  }, [q, index.paths, showHidden]);

  if (!key)
    return (
      <div
        style={{
          padding: 12,
          display: "flex",
          flexDirection: "column",
          gap: 10,
          color: "var(--text-muted)",
          fontSize: "var(--text-sm)",
        }}
      >
        This workspace has no projects yet. Add one to browse its files.
        <div>
          <Button size="sm" icon="folder-plus" onClick={onAddProject}>
            Add project
          </Button>
        </div>
      </div>
    );

  const proj = projects.find((p) => p.key === key);
  const isSelected = (path: string) => selected?.key === key && selected.path === path;

  const renderDir = (dir: string, depth: number): React.ReactNode => {
    const f = folders[dir];
    if (!f || (!f.entries && !f.error))
      return (
        <div
          style={{
            paddingLeft: 26 + depth * 14,
            height: "var(--row-h)",
            display: "flex",
            alignItems: "center",
            gap: 6,
            color: "var(--text-muted)",
            fontSize: "var(--text-sm)",
          }}
        >
          <Spinner size={10} /> Reading…
        </div>
      );
    if (f.error && !f.entries)
      return (
        <div style={{ paddingLeft: 26 + depth * 14, color: "var(--bad)", fontSize: "var(--text-sm)" }}>{f.error}</div>
      );
    const entries = [...(f.entries ?? [])].sort((a, b) =>
      a.is_dir === b.is_dir ? a.name.localeCompare(b.name) : a.is_dir ? -1 : 1,
    );
    const field = creating?.dir === dir && (
      <NewEntry
        key={`new-${creating.kind}`}
        creating={creating}
        depth={depth}
        onCreate={create}
        onCancel={() => setCreating(null)}
      />
    );
    if (entries.length === 0 && depth === 0)
      return (
        field || (
          <div style={{ padding: "12px 8px", color: "var(--text-muted)", fontSize: "var(--text-sm)" }}>
            This project is empty.
          </div>
        )
      );
    return [
      field,
      ...entries.map((e) => {
        const muted = e.ignored ? { color: "var(--text-muted)" } : undefined;
        if (e.is_dir) {
          const isOpen = !!open[e.path];
          return (
            <Fragment key={e.path}>
              <TreeItem
                depth={depth}
                label={<span style={muted}>{e.name}</span>}
                icon={isOpen ? "folder-open" : "folder"}
                expanded={isOpen}
                trailing={
                  !isOpen && e.has_changes ? (
                    <span className="os-dot os-dot--warn" style={{ width: 5, height: 5 }} />
                  ) : null
                }
                onToggle={() => {
                  setOpen((o) => ({ ...o, [e.path]: !o[e.path] }));
                  setLastDir(isOpen ? parentOf(e.path) : e.path);
                }}
                title={e.path + (e.ignored ? " · ignored" : "")}
                onDragStart={(ev) => setContextDrag(ev.dataTransfer, { project: key, path: `${e.path}/` })}
              />
              {isOpen && renderDir(e.path, depth + 1)}
            </Fragment>
          );
        }
        const mark = e.git ? MARK[e.git] : null;
        return (
          <TreeItem
            key={e.path}
            depth={depth}
            label={<span style={mark ? { color: mark.color } : muted}>{e.name}</span>}
            icon={fileIcon(e.name)}
            selected={isSelected(e.path)}
            onClick={() => onOpenFile(key, e.path)}
            onDragStart={(ev) => setContextDrag(ev.dataTransfer, { project: key, path: e.path })}
            meta={mark ? <span style={{ color: mark.color, fontWeight: 600 }}>{mark.letter}</span> : null}
            title={
              e.path + (mark ? ` · ${mark.word}` : "") + (e.changed_by ? ` by ${humanize(e.changed_by.agent)}` : "")
            }
          />
        );
      }),
    ];
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", minHeight: 0, flex: 1 }}>
      <div style={{ display: "flex", flexDirection: "column", gap: 6, padding: "8px 8px 6px" }}>
        <div style={{ display: "flex", gap: 4 }}>
          <div style={{ flex: 1, minWidth: 0 }}>
            <Select
              size="sm"
              mono
              aria-label="Project"
              value={key}
              onChange={(e) => setProject(e.target.value)}
              options={projects.map((p) => ({ value: p.key, label: p.key }))}
            />
          </div>
          <IconButton
            size="sm"
            icon="locate-fixed"
            label="Reveal the open file"
            disabled={!selected}
            onClick={() => reveal(true)}
          />
          <IconButton size="sm" icon="info" label="Project overview" onClick={() => onOpenProject(key)} />
          <IconButton
            size="sm"
            icon={showHidden ? "eye" : "eye-off"}
            label={showHidden ? "Hide dotfiles" : "Show dotfiles"}
            active={showHidden}
            onClick={() => setShowHidden(!showHidden)}
          />
          <IconButton size="sm" icon="file-plus" label="New file" onClick={() => startCreate("file")} />
          <IconButton size="sm" icon="folder-plus" label="New folder" onClick={() => startCreate("folder")} />
          <IconButton
            size="sm"
            icon="chevrons-down-up"
            label="Collapse all"
            onClick={() => {
              setOpen(() => ({}));
              setLastDir(null);
            }}
          />
        </div>
        <Input
          size="sm"
          icon="search"
          placeholder="Find a file"
          aria-label="Find a file"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        {proj && proj.init_status !== "initialized" && (
          <div
            style={{ fontSize: "var(--text-xs)", color: "var(--warn)", display: "flex", gap: 5, alignItems: "center" }}
          >
            <Icon name="circle-alert" size={12} /> Not initialized
          </div>
        )}
      </div>
      <div
        ref={treeRef}
        role="tree"
        aria-label={`Files in ${key}`}
        style={{ flex: 1, overflowY: "auto", overflowX: "hidden", padding: "0 8px 8px" }}
      >
        {q ? (
          index.loading && index.paths.length === 0 ? (
            <div
              style={{
                padding: "12px 8px",
                color: "var(--text-muted)",
                fontSize: "var(--text-sm)",
                display: "flex",
                gap: 6,
                alignItems: "center",
              }}
            >
              <Spinner size={10} /> Reading the file list…
            </div>
          ) : matches.length ? (
            matches.map((p) => (
              <TreeItem
                key={p}
                label={p.split("/").pop()}
                meta={p.split("/").slice(0, -1).join("/")}
                icon={fileIcon(p)}
                selected={isSelected(p)}
                onClick={() => onOpenFile(key, p)}
                onDragStart={(ev) => setContextDrag(ev.dataTransfer, { project: key, path: p })}
                title={p}
              />
            ))
          ) : (
            <div style={{ padding: "12px 8px", color: "var(--text-muted)", fontSize: "var(--text-sm)" }}>
              No file matches.
            </div>
          )
        ) : (
          renderDir("", 0)
        )}
      </div>
      {changes.length > 0 && !q && (
        <div
          style={{
            borderTop: "1px solid var(--border-subtle)",
            padding: "4px 8px 8px",
            flex: "none",
            maxHeight: 160,
            overflowY: "auto",
          }}
        >
          <div className="os-tree-section">
            <span>Changed by sessions</span>
            <span style={{ fontFamily: "var(--font-mono)" }}>{changes.length}</span>
          </div>
          {changes.map((c) => {
            const mark = c.git ? MARK[c.git] : null;
            return (
              <TreeItem
                key={c.path}
                label={c.path.split("/").pop()}
                meta={mark ? <span style={{ color: mark.color, fontWeight: 600 }}>{mark.letter}</span> : null}
                icon="file-diff"
                title={`${c.path} · ${humanize(c.changed_by.agent)}${c.changed_by.phase !== null ? `, phase ${c.changed_by.phase}` : ""} · +${c.added} −${c.removed}`}
                selected={isSelected(c.path)}
                onClick={() => onOpenFile(key, c.path)}
                onDragStart={(ev) => setContextDrag(ev.dataTransfer, { project: key, path: c.path })}
              />
            );
          })}
        </div>
      )}
    </div>
  );
}
