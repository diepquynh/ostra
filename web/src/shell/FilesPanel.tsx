import {
  Button,
  ContextMenu,
  Dialog,
  dragHasFiles,
  FileTree,
  type FileTreeCreating,
  type FileTreeFolder,
  fileIcon,
  Icon,
  IconButton,
  Input,
  type MenuItem,
  parentDir,
  Select,
  Spinner,
  TreeItem,
} from "@ostra/design";
import { type DragEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { ProjectTreeEntry, ProjectView } from "../api/types";
import { runsUnsandboxed } from "../content/agents";
import {
  ARTIFACTS_ROOT,
  baseName,
  CONTEXT_DRAG_TYPE,
  readContextDrag,
  setContextDrag,
  tagOf,
} from "../features/context/tags";
import { humanize } from "../lib/format";
import { useFileIndex, useProjectChanges, useProjectFsChanges } from "../lib/live";
import { useNav, useWorkspace } from "../lib/nav";
import { throttle } from "../lib/store";
import { GIT_MARK } from "../screens/project/files";

const MAX_MATCHES = 200;

type Folder = FileTreeFolder<ProjectTreeEntry>;
type Menu = { at: { x: number; y: number }; entry: ProjectTreeEntry | null };

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
  /** `artifacts` shows the workspace artifacts (HANDOVER 6.5) instead of a project's files. */
  root?: "project" | "artifacts";
};

const parents = (path: string) => {
  const segs = path.split("/");
  return segs.slice(0, -1).map((_, i) => segs.slice(0, i + 1).join("/"));
};

/** Where a dragged artifact lands when dropped into `dir`, or null when the drop would not move it. */
export function moveTarget(from: string, dir: string): string | null {
  const path = from.replace(/\/$/, "");
  const to = dir ? `${dir}/${baseName(path)}` : baseName(path);
  if (to === path || dir === path || dir.startsWith(`${path}/`)) return null;
  return to;
}

/**
 * Left-dock Files and Artifacts tabs: the root picker, find, dotfiles, collapse all, the lazy folder tree, a right-click
 * menu, and "Changed by sessions". Artifacts add upload, download, hide, and delete.
 */
export function FilesPanel({
  ws,
  projects,
  project,
  setProject,
  selected,
  onOpenFile,
  onOpenProject,
  onAddProject,
  root = "project",
}: FilesPanelProps) {
  const artifacts = root === "artifacts";
  const nav = useNav();
  const sandbox = useWorkspace().detail?.sandbox;
  const key = artifacts
    ? ARTIFACTS_ROOT
    : project && projects.some((p) => p.key === project)
      ? project
      : (projects[0]?.key ?? null);
  const [showHidden, setShowHidden] = useState(false);
  const [filter, setFilter] = useState("");
  const [creating, setCreating] = useState<FileTreeCreating | null>(null);
  const [menu, setMenu] = useState<Menu | null>(null);
  const [deleting, setDeleting] = useState<ProjectTreeEntry | null>(null);
  const [error, setError] = useState<string | null>(null);
  const upload = useRef<{ input: HTMLInputElement | null; dir: string }>({ input: null, dir: "" });
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

  // Cached listings belong to one root and one dotfiles setting.
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
  const mine = selected && (artifacts ? selected.key === ARTIFACTS_ROOT : selected.key !== ARTIFACTS_ROOT);
  const reveal = (clearFilter: boolean) => {
    if (!selected || !mine) return;
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
  // biome-ignore lint/correctness/useExhaustiveDependencies: reveal only when the open file changes.
  useEffect(() => {
    reveal(false);
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

  const startCreate = (kind: FileTreeCreating["kind"], at?: string) => {
    const dir = at ?? lastDir ?? (selected?.key === key ? parentDir(selected.path) : "");
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

  const run = async (fn: () => Promise<unknown>) => {
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError((e as Error).message);
    }
    loadedRef.current();
  };

  const uploadFiles = (dir: string, files: File[]) =>
    void run(async () => {
      for (const f of files) await api.uploadArtifact(ws, dir ? `${dir}/${f.name}` : f.name, f);
      if (dir) setOpen((o) => ({ ...o, ...Object.fromEntries([...parents(`${dir}/x`), dir].map((d) => [d, true])) }));
    });
  /** Rule W4 applies: the server refuses a move while a session is live, and the reason shows under the header. */
  const moveInto = (dir: string, from: string) => {
    const to = moveTarget(from, dir);
    if (!to) return;
    const was = from.replace(/\/$/, "");
    void run(async () => {
      await api.moveArtifact(ws, was, to);
      if (dir) setOpen((o) => ({ ...o, ...Object.fromEntries([...parents(`${dir}/x`), dir].map((d) => [d, true])) }));
      if (selected?.key === key && (selected.path === was || selected.path.startsWith(`${was}/`)))
        onOpenFile(key, to + selected.path.slice(was.length));
    });
  };
  const dropEffect = (ev: DragEvent<HTMLDivElement>): "copy" | "move" | null =>
    !artifacts ? null : dragHasFiles(ev) ? "copy" : ev.dataTransfer.types.includes(CONTEXT_DRAG_TYPE) ? "move" : null;
  const dropInto = (dir: string, ev: DragEvent<HTMLDivElement>) => {
    if (dragHasFiles(ev)) return uploadFiles(dir, Array.from(ev.dataTransfer.files));
    const f = readContextDrag(ev.dataTransfer);
    if (f?.project === ARTIFACTS_ROOT) moveInto(dir, f.path);
  };

  const pickUploads = (dir: string) => {
    upload.current.dir = dir;
    upload.current.input?.click();
  };

  /** The topmost hidden folder above a hidden path, as the loaded listings show it, else the path itself. */
  const hiddenUnit = (path: string): string => {
    let unit = path;
    for (let dir = parentDir(path); dir; dir = parentDir(dir)) {
      const row = folders[parentDir(dir)]?.entries?.find((x) => x.path === dir);
      if (!row?.hidden_from_agents) break;
      unit = dir;
    }
    return unit;
  };

  const menuItems = (e: ProjectTreeEntry | null): MenuItem[] => {
    const dir = e ? (e.is_dir ? e.path : parentDir(e.path)) : "";
    const items: MenuItem[] = [];
    if (e && !e.is_dir && key) items.push({ label: "Open", icon: "file", onSelect: () => onOpenFile(key, e.path) });
    items.push(
      { label: "New file", icon: "file-plus", onSelect: () => startCreate("file", dir) },
      { label: "New folder", icon: "folder-plus", onSelect: () => startCreate("folder", dir) },
    );
    if (artifacts) items.push({ label: "Upload files", icon: "paperclip", onSelect: () => pickUploads(dir) });
    if (e && key) {
      const target = e.is_dir ? { project: key, path: `${e.path}/` } : { project: key, path: e.path };
      items.push(
        { type: "divider" },
        { label: "Copy path", icon: "copy", onSelect: () => void navigator.clipboard?.writeText(e.path) },
        { label: "Copy tag", icon: "at-sign", onSelect: () => void navigator.clipboard?.writeText(tagOf(target)) },
      );
    }
    if (artifacts && e) {
      items.push({ type: "divider" });
      if (!e.is_dir)
        items.push({
          label: "Download",
          icon: "download",
          onSelect: () => window.open(api.artifactDownloadUrl(ws, e.path), "_self"),
        });
      // Rule W2: a hidden folder hides all it holds, so what lies inside one is shown with the folder.
      const unit = e.hidden_from_agents ? hiddenUnit(e.path) : null;
      items.push(
        unit
          ? {
              label: unit === e.path ? "Show to agents" : `Show folder ${baseName(unit)} to agents`,
              icon: "eye",
              onSelect: () => void run(() => api.setArtifactHidden(ws, unit, false)),
            }
          : {
              label: "Hide from agents",
              icon: "eye-off",
              onSelect: () => void run(() => api.setArtifactHidden(ws, e.path, true)),
            },
      );
      if (!e.is_dir) items.push({ label: "Delete", icon: "trash-2", danger: true, onSelect: () => setDeleting(e) });
    }
    return items;
  };

  const index = useFileIndex(ws, filter.trim() ? key : null);
  const { changes } = useProjectChanges(ws, artifacts ? null : key);
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
  const drag = (path: string) => (ev: DragEvent<HTMLDivElement>) =>
    setContextDrag(ev.dataTransfer, { project: key, path });

  return (
    <div style={{ display: "flex", flexDirection: "column", minHeight: 0, flex: 1 }}>
      <div style={{ display: "flex", flexDirection: "column", gap: 6, padding: "8px 8px 6px" }}>
        <div style={{ display: "flex", gap: 4, alignItems: "center" }}>
          <div style={{ flex: 1, minWidth: 0 }}>
            {artifacts ? null : (
              <Select
                size="sm"
                mono
                aria-label="Project"
                value={key}
                onChange={(e) => setProject(e.target.value)}
                options={projects.map((p) => ({ value: p.key, label: p.key }))}
              />
            )}
          </div>
          <IconButton
            size="sm"
            icon="locate-fixed"
            label="Reveal the open file"
            disabled={!mine}
            onClick={() => reveal(true)}
          />
          {!artifacts && (
            <IconButton size="sm" icon="info" label="Project overview" onClick={() => onOpenProject(key)} />
          )}
          <IconButton
            size="sm"
            icon={showHidden ? "eye" : "eye-off"}
            label={showHidden ? "Hide dotfiles" : "Show dotfiles"}
            active={showHidden}
            onClick={() => setShowHidden(!showHidden)}
          />
          <IconButton size="sm" icon="file-plus" label="New file" onClick={() => startCreate("file")} />
          <IconButton size="sm" icon="folder-plus" label="New folder" onClick={() => startCreate("folder")} />
          {artifacts && (
            <IconButton size="sm" icon="paperclip" label="Upload files" onClick={() => pickUploads(lastDir ?? "")} />
          )}
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
          placeholder={artifacts ? "Find an artifact" : "Find a file"}
          aria-label={artifacts ? "Find an artifact" : "Find a file"}
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
        {artifacts && sandbox && runsUnsandboxed(sandbox) && (
          <div style={{ fontSize: "var(--text-xs)", color: "var(--warn)", display: "flex", gap: 5 }}>
            <Icon name="circle-alert" size={12} style={{ flex: "none", marginTop: 1 }} />
            <span>
              Shell commands can read hidden artifacts here, because agent commands run without a sandbox.{" "}
              <button
                type="button"
                style={{
                  background: "none",
                  border: 0,
                  padding: 0,
                  color: "var(--accent-fg)",
                  font: "inherit",
                  textDecoration: "underline",
                  cursor: "pointer",
                }}
                onClick={() => nav.open("ws:settings", { anchor: "setting:sandbox_mode" })}
              >
                Turn the sandbox on
              </button>
            </span>
          </div>
        )}
        {error && (
          <div role="alert" style={{ fontSize: "var(--text-xs)", color: "var(--bad)" }}>
            {error}
          </div>
        )}
      </div>
      <div
        ref={treeRef}
        role="tree"
        aria-label={artifacts ? "Workspace artifacts" : `Files in ${key}`}
        style={{ flex: 1, overflowY: "auto", overflowX: "hidden", padding: "0 8px 8px" }}
        onContextMenu={(ev) => {
          ev.preventDefault();
          setMenu({ at: { x: ev.clientX, y: ev.clientY }, entry: null });
        }}
        onDragOver={(ev) => {
          const effect = dropEffect(ev);
          if (!effect) return;
          ev.preventDefault();
          ev.dataTransfer.dropEffect = effect;
        }}
        onDrop={(ev) => {
          if (!dropEffect(ev)) return;
          ev.preventDefault();
          dropInto("", ev);
        }}
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
                onDragStart={drag(p)}
                title={p}
              />
            ))
          ) : (
            <div style={{ padding: "12px 8px", color: "var(--text-muted)", fontSize: "var(--text-sm)" }}>
              No file matches.
            </div>
          )
        ) : (
          <FileTree<ProjectTreeEntry>
            folder={(dir) => folders[dir]}
            open={open}
            selected={selected?.key === key ? selected.path : null}
            onToggle={(e, isOpen) => {
              setOpen((o) => ({ ...o, [e.path]: !o[e.path] }));
              setLastDir(isOpen ? parentDir(e.path) : e.path);
            }}
            onOpen={(e) => onOpenFile(key, e.path)}
            onDragStart={(e, ev) => drag(e.is_dir ? `${e.path}/` : e.path)(ev)}
            dropEffect={dropEffect}
            onDrop={dropInto}
            onContextMenu={(e, ev) => setMenu({ at: { x: ev.clientX, y: ev.clientY }, entry: e })}
            row={(e) => {
              if (e.hidden_from_agents)
                return {
                  muted: true,
                  trailing: <Icon name="eye-off" size={12} style={{ color: "var(--text-muted)" }} />,
                  title: `${e.path} · hidden from agents`,
                };
              if (e.is_dir)
                return {
                  muted: e.ignored,
                  trailing:
                    !open[e.path] && e.has_changes ? (
                      <span className="os-dot os-dot--warn" style={{ width: 5, height: 5 }} />
                    ) : null,
                  title: e.path + (e.ignored ? " · ignored" : ""),
                };
              const mark = e.git ? GIT_MARK[e.git] : null;
              return {
                color: mark?.color,
                muted: e.ignored,
                meta: mark ? <span style={{ color: mark.color, fontWeight: 600 }}>{mark.letter}</span> : null,
                title:
                  e.path +
                  (mark ? ` · ${mark.word}` : "") +
                  (e.changed_by ? ` by ${humanize(e.changed_by.agent)}` : ""),
              };
            }}
            creating={creating}
            onCreate={create}
            onCancelCreate={() => setCreating(null)}
            empty={
              artifacts
                ? "No artifacts yet. Drop files here, or create a file with the buttons above."
                : "This project is empty."
            }
          />
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
            const mark = c.git ? GIT_MARK[c.git] : null;
            return (
              <TreeItem
                key={c.path}
                label={c.path.split("/").pop()}
                meta={mark ? <span style={{ color: mark.color, fontWeight: 600 }}>{mark.letter}</span> : null}
                icon="file-diff"
                title={`${c.path} · ${humanize(c.changed_by.agent)}${c.changed_by.phase !== null ? `, phase ${c.changed_by.phase}` : ""} · +${c.added} −${c.removed}`}
                selected={isSelected(c.path)}
                onClick={() => onOpenFile(key, c.path)}
                onDragStart={drag(c.path)}
              />
            );
          })}
        </div>
      )}
      <ContextMenu
        at={menu?.at ?? null}
        items={menu ? menuItems(menu.entry) : []}
        label={menu?.entry ? `Actions for ${menu.entry.path}` : "Actions"}
        onClose={() => setMenu(null)}
      />
      {artifacts && (
        <input
          ref={(el) => {
            upload.current.input = el;
          }}
          type="file"
          multiple
          hidden
          onChange={(e) => {
            uploadFiles(upload.current.dir, Array.from(e.target.files ?? []));
            e.target.value = "";
          }}
        />
      )}
      {deleting && (
        <Dialog
          title="Delete this artifact?"
          width={440}
          onClose={() => setDeleting(null)}
          footer={
            <>
              <span className="wp-spacer" />
              <Button onClick={() => setDeleting(null)}>Keep it</Button>
              <Button
                variant="danger"
                onClick={() => {
                  const path = deleting.path;
                  setDeleting(null);
                  void run(() => api.deleteArtifact(ws, path));
                }}
              >
                Delete
              </Button>
            </>
          }
        >
          <span style={{ fontFamily: "var(--font-mono)" }}>{deleting.path}</span>
          <p style={{ color: "var(--text-secondary)" }}>
            Ostra deletes it only while no session is running, waiting, stalled, or paused, because an agent may be
            reading it.
          </p>
        </Dialog>
      )}
    </div>
  );
}
