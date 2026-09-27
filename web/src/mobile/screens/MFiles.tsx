import { Chip, fileIcon, Icon, parentDir, Spinner } from "@ostra/design";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import type { ProjectTreeEntry } from "../../api/types";
import { runsUnsandboxed } from "../../content/agents";
import { ARTIFACTS_ROOT, baseName } from "../../features/context/tags";
import { humanize } from "../../lib/format";
import { useFileIndex, useProjectChanges, useProjectFsChanges } from "../../lib/live";
import { useNav, useWorkspace } from "../../lib/nav";
import { fileId } from "../../lib/resource";
import { throttle } from "../../lib/store";
import { GIT_MARK, sortEntries } from "../../screens/project/files";
import { moveTarget } from "../../shell/FilesPanel";
import { cleanName, moveDestinations } from "./projectNav";

const MAX_MATCHES = 200;

type Listing = { entries: ProjectTreeEntry[] | null; error: string | null };

type MFilesProps = {
  ws: string;
  projectKey: string;
  dir: string;
  /** Open a folder of this project or of the artifacts. */
  openDir: (dir: string) => void;
  /** The project root without a folder open: shows "Changed by sessions". */
  root: boolean;
};

function Mark({ git }: { git: ProjectTreeEntry["git"] }) {
  if (!git) return null;
  const m = GIT_MARK[git];
  return (
    <span className="mp-mark" style={{ color: m.color }} title={m.word}>
      {m.letter}
    </span>
  );
}

/** A folder of a project or of the workspace artifacts: find, create, and each entry; artifacts add upload, hide, move and delete. */
export function MFiles({ ws, projectKey, dir, openDir, root }: MFilesProps) {
  const nav = useNav();
  const sandbox = useWorkspace().detail?.sandbox;
  const art = projectKey === ARTIFACTS_ROOT;
  const [showHidden, setShowHidden] = useState(false);
  const [find, setFind] = useState("");
  const [creating, setCreating] = useState<"file" | "folder" | null>(null);
  const [name, setName] = useState("");
  const [createErr, setCreateErr] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [act, setAct] = useState<{ path: string; moving: boolean; confirmDelete: boolean } | null>(null);
  const uploadRef = useRef<HTMLInputElement>(null);

  // Listings by folder for one dotfiles setting; ancestors stay cached so a hidden folder's children know it.
  const cacheId = `${projectKey}\n${showHidden}`;
  const [cache, setCache] = useState<{ id: string; map: Record<string, Listing> }>({ id: cacheId, map: {} });
  const listings = cache.id === cacheId ? cache.map : {};
  const load = useCallback(
    (d: string) => {
      const id = cacheId;
      api.projectTree(ws, projectKey, { path: d, depth: 1, hidden: showHidden }).then(
        (t) =>
          setCache((c) => ({
            id,
            map: {
              ...(c.id === id ? c.map : {}),
              [d]: { entries: t.entries.filter((e) => parentDir(e.path) === d), error: null },
            },
          })),
        (e: Error) =>
          setCache((c) => ({
            id,
            map: { ...(c.id === id ? c.map : {}), [d]: { entries: c.map[d]?.entries ?? null, error: e.message } },
          })),
      );
    },
    [ws, projectKey, showHidden, cacheId],
  );
  useEffect(() => {
    if (!listings[dir]) load(dir);
  }, [dir, listings, load]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: another folder starts without the last one's forms.
  useEffect(() => {
    setFind("");
    setCreating(null);
    setAct(null);
    setError(null);
  }, [dir]);

  const loadedRef = useRef<() => void>(() => {});
  loadedRef.current = () => Object.keys(listings).forEach((d) => load(d));
  const refresh = useMemo(() => throttle(() => loadedRef.current(), 500), []);
  useEffect(() => refresh.cancel, [refresh]);
  useProjectFsChanges(ws, projectKey, refresh);

  const index = useFileIndex(ws, find.trim() || (art && act?.moving) ? projectKey : null);
  const { changes } = useProjectChanges(ws, art || !root ? null : projectKey);
  const q = find.trim().toLowerCase();
  const matches = useMemo(() => {
    if (!q) return [];
    const ok = (p: string) => showHidden || !p.split("/").some((s) => s.startsWith("."));
    const out: string[] = [];
    for (const p of index.paths) {
      if (baseName(p).toLowerCase().includes(q) && ok(p)) out.push(p);
      if (out.length >= MAX_MATCHES) break;
    }
    return out;
  }, [q, index.paths, showHidden]);

  const run = async (fn: () => Promise<unknown>) => {
    setError(null);
    setAct(null);
    try {
      await fn();
    } catch (e) {
      setError((e as Error).message);
    }
    loadedRef.current();
  };

  const create = async () => {
    const n = cleanName(name);
    if (!n) return setCreateErr("Enter a name.");
    const path = dir ? `${dir}/${n}` : n;
    try {
      if (creating === "file") {
        await api.saveProjectFile(ws, projectKey, { path, content: "", base_hash: null });
        nav.open(fileId(projectKey, path), { anchor: "edit" });
      } else {
        await api.createProjectFolder(ws, projectKey, path);
        openDir(path);
      }
      setCreating(null);
      setName("");
      load(dir);
    } catch (e) {
      setCreateErr((e as Error).message);
    }
  };

  /** Rule W2: a hidden folder hides all it holds, so what lies inside one is shown with the folder. */
  const hiddenUnit = (path: string): string => {
    let unit = path;
    for (let d = parentDir(path); d; d = parentDir(d)) {
      const row = listings[parentDir(d)]?.entries?.find((x) => x.path === d);
      if (!row?.hidden_from_agents) break;
      unit = d;
    }
    return unit;
  };

  const listing = listings[dir];
  const entries = listing?.entries ? sortEntries(listing.entries, { key: "name", dir: 1 }) : null;
  const knownDirs = Object.values(listings).flatMap((l) =>
    (l.entries ?? []).filter((e) => e.is_dir).map((e) => e.path),
  );

  const actions = (e: ProjectTreeEntry) => {
    const a = act?.path === e.path ? act : null;
    if (!a) return null;
    if (a.moving) {
      const dests = moveDestinations(e.path, index.paths, knownDirs);
      return (
        <div className="mp-acts">
          <span style={{ fontSize: 12, color: "var(--text-muted)" }}>Move {e.name} to</span>
          <div className="mp-list">
            {index.loading && !index.paths.length && (
              <span className="m-muted" style={{ display: "flex", gap: 8, padding: 12 }}>
                <Spinner size={11} /> Reading the folders…
              </span>
            )}
            {dests.map((d) => (
              <button
                type="button"
                key={d || "."}
                className="mp-dest"
                onClick={() => {
                  const to = moveTarget(e.path, d);
                  if (to) void run(() => api.moveArtifact(ws, e.path, to));
                }}
              >
                <Icon name={d ? "folder" : "package"} size={15} style={{ color: "var(--text-muted)" }} />
                {d || "Artifacts (top level)"}
              </button>
            ))}
          </div>
          <button type="button" className="m-btn m-btn-quiet" onClick={() => setAct({ ...a, moving: false })}>
            Cancel
          </button>
        </div>
      );
    }
    const unit = e.hidden_from_agents ? hiddenUnit(e.path) : null;
    return (
      <div className="mp-acts">
        <div className="mp-acts-grid">
          {!e.is_dir && (
            <button
              type="button"
              className="mp-act"
              onClick={() => {
                setAct(null);
                window.open(api.artifactDownloadUrl(ws, e.path), "_self");
              }}
            >
              <Icon name="download" size={15} />
              Download
            </button>
          )}
          <button
            type="button"
            className="mp-act"
            onClick={() => void run(() => api.setArtifactHidden(ws, unit ?? e.path, !unit))}
          >
            <Icon name={unit ? "eye" : "eye-off"} size={15} />
            {unit ? (unit === e.path ? "Show" : `Show ${baseName(unit)}`) : "Hide"}
          </button>
          <button type="button" className="mp-act" onClick={() => setAct({ ...a, moving: true })}>
            <Icon name="folder-input" size={15} />
            Move
          </button>
          {!e.is_dir && (
            <button
              type="button"
              className="mp-act mp-danger"
              onClick={() =>
                a.confirmDelete ? void run(() => api.deleteArtifact(ws, e.path)) : setAct({ ...a, confirmDelete: true })
              }
            >
              <Icon name="trash-2" size={15} />
              {a.confirmDelete ? "Tap to confirm" : "Delete"}
            </button>
          )}
        </div>
        {!e.is_dir && a.confirmDelete && (
          <span className="m-help">
            Ostra deletes it only while no session is running, waiting, stalled, or paused, because an agent may be
            reading it.
          </span>
        )}
      </div>
    );
  };

  const node = (e: ProjectTreeEntry) => (
    <div key={e.path} className={`mp-node${e.ignored || e.hidden_from_agents ? " mp-node-muted" : ""}`}>
      <div className="mp-node-line">
        <button
          type="button"
          className="mp-node-btn"
          onClick={() => (e.is_dir ? openDir(e.path) : nav.open(fileId(projectKey, e.path)))}
          title={e.changed_by ? `Changed by ${humanize(e.changed_by.agent)}` : undefined}
        >
          <Icon
            name={e.is_dir ? "folder" : fileIcon(e.name)}
            size={16}
            style={{ color: e.is_dir ? "var(--accent)" : "var(--text-muted)", flex: "none" }}
          />
          <span className="mp-item-body" style={{ gap: 1 }}>
            <span className="mp-node-name" style={e.git ? { color: GIT_MARK[e.git].color } : undefined}>
              {e.name}
            </span>
          </span>
          {e.is_dir && e.has_changes && !e.git && (
            <span className="os-dot os-dot--warn" style={{ width: 5, height: 5, flex: "none" }} />
          )}
          <Mark git={e.git} />
          {art && e.hidden_from_agents && <Chip icon="eye-off">Hidden</Chip>}
          {e.is_dir && <Icon name="chevron-right" size={15} style={{ color: "var(--text-muted)", flex: "none" }} />}
        </button>
        {art && (
          <button
            type="button"
            className="mp-more"
            aria-label={`Actions for ${e.name}`}
            aria-expanded={act?.path === e.path}
            onClick={() => setAct(act?.path === e.path ? null : { path: e.path, moving: false, confirmDelete: false })}
          >
            <Icon name="ellipsis-vertical" size={17} />
          </button>
        )}
      </div>
      {actions(e)}
    </div>
  );

  const where = `${art ? "Artifacts" : projectKey}/${dir ? `${dir}/` : ""}`;
  return (
    <>
      <div className="mp-pad" style={{ gap: 8 }}>
        <div className="mp-tools">
          <input
            className="m-input mp-mono-input"
            type="search"
            aria-label={art ? "Find an artifact" : "Find a file"}
            placeholder={art ? "Find an artifact" : "Find a file"}
            value={find}
            onChange={(e) => setFind(e.target.value)}
          />
          {art && (
            <>
              <button type="button" className="mp-tool" aria-label="Upload" onClick={() => uploadRef.current?.click()}>
                <Icon name="upload" size={17} />
              </button>
              <input
                ref={uploadRef}
                type="file"
                multiple
                hidden
                onChange={(e) => {
                  const files = Array.from(e.target.files ?? []);
                  e.target.value = "";
                  void run(async () => {
                    for (const f of files) await api.uploadArtifact(ws, dir ? `${dir}/${f.name}` : f.name, f);
                  });
                }}
              />
            </>
          )}
          <button
            type="button"
            className="mp-tool"
            aria-pressed={showHidden}
            aria-label={showHidden ? "Hide dotfiles" : "Show dotfiles"}
            onClick={() => setShowHidden((v) => !v)}
          >
            <Icon name={showHidden ? "eye" : "eye-off"} size={17} />
          </button>
          {!art && (
            <button
              type="button"
              className="mp-tool"
              aria-label="New file"
              onClick={() => {
                setCreating("file");
                setName("");
                setCreateErr(null);
              }}
            >
              <Icon name="file-plus" size={17} />
            </button>
          )}
          <button
            type="button"
            className="mp-tool"
            aria-label="New folder"
            onClick={() => {
              setCreating("folder");
              setName("");
              setCreateErr(null);
            }}
          >
            <Icon name="folder-plus" size={17} />
          </button>
        </div>
        {art && (
          <span className="m-help">
            Files you give Ostra for this workspace: uploads, samples, exports. Sessions can read them. Hidden ones stay
            on disk but agents do not see them.
          </span>
        )}
        {art && sandbox && runsUnsandboxed(sandbox) && (
          <span style={{ display: "flex", gap: 6, fontSize: 12, lineHeight: 1.5, color: "var(--warn)" }}>
            <Icon name="circle-alert" size={13} style={{ flex: "none", marginTop: 2 }} />
            <span>
              Shell commands can read hidden artifacts here, because agent commands run without a sandbox.{" "}
              <button
                type="button"
                className="mp-link"
                style={{ display: "inline", height: "auto", padding: 0, fontSize: 12 }}
                onClick={() => nav.open("ws:settings", { anchor: "setting:sandbox_mode" })}
              >
                Turn the sandbox on
              </button>
            </span>
          </span>
        )}
        {error && (
          <span role="alert" className="mp-error">
            {error}
          </span>
        )}
        {creating && (
          <form
            className="mp-form"
            onSubmit={(e) => {
              e.preventDefault();
              void create();
            }}
          >
            <span style={{ fontSize: 12, color: "var(--text-secondary)" }}>
              {creating === "file" ? "New file in " : "New folder in "}
              <span style={{ fontFamily: "var(--font-mono)" }}>{where}</span>
            </span>
            <div className="mp-tools">
              <input
                autoFocus
                className="m-input mp-mono-input"
                aria-label={creating === "file" ? "New file name" : "New folder name"}
                placeholder={creating === "file" ? "notes.md" : "docs/drafts"}
                value={name}
                onChange={(e) => {
                  setName(e.target.value);
                  setCreateErr(null);
                }}
              />
              <button type="submit" className="m-btn m-btn-primary">
                Create
              </button>
            </div>
            {createErr && <span className="mp-error">{createErr}</span>}
            <span style={{ fontSize: 11.5, color: "var(--text-muted)" }}>
              A name with / creates the folders on the way.
            </span>
            <button type="button" className="m-btn m-btn-quiet m-btn-sm" onClick={() => setCreating(null)}>
              Cancel
            </button>
          </form>
        )}
      </div>
      {root && !art && !q && changes.length > 0 && (
        <div style={{ display: "flex", flexDirection: "column" }}>
          <span className="m-label mp-group-label">
            Changed by sessions<span style={{ fontFamily: "var(--font-mono)" }}>{changes.length}</span>
          </span>
          {changes.map((c) => (
            <div key={c.path} className="mp-node">
              <button type="button" className="mp-node-btn" onClick={() => nav.open(fileId(projectKey, c.path))}>
                <Icon name="file-diff" size={16} style={{ color: "var(--text-muted)", flex: "none" }} />
                <span className="mp-item-body" style={{ gap: 1 }}>
                  <span className="mp-node-name">{baseName(c.path)}</span>
                  <span className="mp-node-sub">
                    {c.path} · {humanize(c.changed_by.agent)} · +{c.added} −{c.removed}
                  </span>
                </span>
                <Mark git={c.git} />
              </button>
            </div>
          ))}
        </div>
      )}
      <div style={{ display: "flex", flexDirection: "column" }}>
        <span className="m-label mp-group-label">{q ? "Matches" : dir || (art ? "Artifacts" : "Files")}</span>
        {q ? (
          index.loading && !index.paths.length ? (
            <span className="m-muted" style={{ display: "flex", gap: 8, padding: "12px 16px" }}>
              <Spinner size={11} /> Reading the file list…
            </span>
          ) : matches.length ? (
            matches.map((p) => (
              <div key={p} className="mp-node">
                <button type="button" className="mp-node-btn" onClick={() => nav.open(fileId(projectKey, p))}>
                  <Icon name={fileIcon(p)} size={16} style={{ color: "var(--text-muted)", flex: "none" }} />
                  <span className="mp-item-body" style={{ gap: 1 }}>
                    <span className="mp-node-name">{baseName(p)}</span>
                    <span className="mp-node-sub">{p}</span>
                  </span>
                </button>
              </div>
            ))
          ) : (
            <span className="m-muted" style={{ padding: "12px 16px" }}>
              No files match.
            </span>
          )
        ) : !entries ? (
          listing?.error ? (
            <span className="mp-error" style={{ padding: "12px 16px" }}>
              {listing.error}
            </span>
          ) : (
            <span className="m-muted" style={{ display: "flex", gap: 8, padding: "12px 16px" }}>
              <Spinner size={11} /> Reading the folder…
            </span>
          )
        ) : entries.length ? (
          entries.map(node)
        ) : (
          <span className="m-muted" style={{ padding: "12px 16px" }}>
            {art && !dir
              ? "No artifacts yet. Upload files or create a folder with the buttons above."
              : "This folder is empty."}
          </span>
        )}
      </div>
    </>
  );
}
