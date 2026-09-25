import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { useLocation } from "react-router";
import { api } from "../api";
import type { ChangedBy } from "../api/types";
import { Markdown } from "../components/Markdown";
import { Banner, Breadcrumbs, Button, Icon, IconButton, Spinner, type TabItem, Tabs } from "../design";
import { humanize } from "../lib/format";
import { useAsync } from "../lib/hooks";
import { useProjectFsChanges, useWorkspaceTree } from "../lib/live";
import { useNav, useShell } from "../lib/nav";
import { fileId } from "../lib/resource";
import { CodePane, takeCarried } from "./project/code/CodePane";
import type { EditorStart } from "./project/code/FileEditor";
import type { SymbolRef } from "./project/code/SourceView";
import { lineFromHash } from "./project/code/tokens";
import { useFileEdit } from "./project/code/useFileEdit";
import { DiffPane } from "./project/DiffPane";
import { changeBlocks } from "./project/diff";
import { extOf, fmtSize, GIT_MARK, modifiedLabel } from "./project/files";

export type FileScreenProps = {
  ws: string;
  projectKey: string;
  /** Project-relative, `/`-separated. */
  path: string;
};

type View = "file" | "rendered" | "diff";

const FileEditor = lazy(() => import("./project/code/FileEditor"));

/** "Implementer · Phase 2" or "Write-test · Phase 3 tests". */
export function changedByLabel(c: ChangedBy): string {
  const phase = c.phase === null ? "" : ` · Phase ${c.phase}${c.tests ? " tests" : ""}`;
  return `${humanize(c.agent)}${phase}`;
}

const barStyle = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  padding: "6px 12px",
  fontSize: "var(--text-sm)",
  color: "var(--text-secondary)",
  background: "var(--surface-panel)",
  borderBottom: "1px solid var(--border-subtle)",
  flex: "none",
} as const;

const linkStyle = {
  background: "none",
  border: 0,
  padding: 0,
  font: "inherit",
  fontWeight: 500,
  color: "var(--text-primary)",
  cursor: "pointer",
} as const;

/**
 * Resource `file:<key>:<path>`: a project file using the whole center pane, rendered markdown for `.md`,
 * or its Changes view (unified diff against HEAD) when a session touched it. Refreshes when the file
 * changes on disk. The File view is one Monaco editor, read-only until the pencil starts an edit; while viewing, a
 * clicked name drives the code pane beside it (outline, usages, dependencies), and changed lines carry gutter marks.
 * A `#L<n>` hash marks a line. Edit mode saves through the hash the edit started from and shows a conflict when the
 * file changed on disk.
 */
export function FileScreen({ ws, projectKey, path }: FileScreenProps) {
  const nav = useNav();
  const shell = useShell();
  const tree = useWorkspaceTree(ws);
  const file = useAsync(() => api.projectFile(ws, projectKey, path), [ws, projectKey, path]);
  const changed = !!file.data?.git;
  const diff = useAsync(
    () => (changed ? api.projectDiff(ws, projectKey, path) : Promise.resolve(null)),
    [ws, projectKey, path, changed],
  );
  const [mode, setMode] = useState<View | null>(null);
  const [copied, setCopied] = useState(false);
  const code = useAsync(() => api.codeFile(ws, projectKey, path), [ws, projectKey, path]);
  const [selected, setSelected] = useState<SymbolRef | null>(() => takeCarried(projectKey, path));
  const [goto, setGoto] = useState<{ line: number; n: number } | null>(null);
  const [paneOpen, setPaneOpen] = useState<boolean | null>(null);
  const hash = useLocation().hash;
  const hashLine = lineFromHash(hash);
  const edit = useFileEdit(ws, projectKey, path, file);
  const [editStart, setEditStart] = useState<EditorStart | null>(null);
  // `#edit`, from creating the file in the Files panel, opens it in edit mode once.
  const autoEdit = useRef(hash === "#edit");
  useEffect(() => {
    const f = file.data;
    if (!autoEdit.current || !f || f.hash === null || f.read_only) return;
    autoEdit.current = false;
    edit.start();
  }, [file.data, edit]);
  useProjectFsChanges(ws, projectKey, (paths) => {
    if (paths.includes(path)) {
      file.reload();
      diff.reload();
      code.reload();
    }
  });

  const f = file.data;
  const d = diff.data && diff.data.hunks.length > 0 ? diff.data : null;
  const changes = useMemo(() => (d ? changeBlocks(d.hunks) : null), [d]);
  const ext = extOf(path);
  const md = ext === "md" || ext === "markdown";
  const views: TabItem[] = [{ id: "file", label: "File", icon: "file-text" }];
  if (md && f?.content) views.push({ id: "rendered", label: "Rendered", icon: "book-open" });
  if (d) views.push({ id: "diff", label: "Changes", icon: "file-diff" });
  const view: View = edit.editing ? "file" : mode && views.some((v) => v.id === mode) ? mode : d ? "diff" : "file";
  const lines = f?.content ? f.content.replace(/\n$/, "").split("\n").length : null;
  const by = f?.changed_by ?? d?.changed_by ?? null;
  const session = by ? tree.sessions.find((s) => s.id === by.session) : null;
  const segs = path.split("/");
  const c = code.data?.path === path ? code.data : null;
  const canPane = view === "file" && !!f && !f.binary && !edit.editing;
  const canEdit = view === "file" && !!f && f.hash !== null && !edit.editing;
  const showPane = canPane && (paneOpen ?? !!c?.language);
  const gotoLine = (line: number) => {
    setMode("file");
    setGoto((g) => ({ line, n: (g?.n ?? 0) + 1 }));
  };
  const locked = !f
    ? null
    : f.hash === null
      ? f.truncated
        ? "This file is larger than the size Ostra reads for the browser, so it cannot be edited here."
        : "This file is not UTF-8 text, so it cannot be edited here."
      : (f.read_only ?? null);
  const reveal = useMemo(
    () => (goto ? { line: goto.line, n: goto.n } : hashLine ? { line: hashLine, n: 0 } : null),
    [goto, hashLine],
  );
  useEffect(() => {
    if (!edit.editing) setEditStart(null);
    // Done lands back on the File view, not on Changes, even when the save made the file differ from HEAD.
    else setMode("file");
  }, [edit.editing]);

  useEffect(() => {
    if (!edit.editing) return;
    // Cmd/Ctrl+S outside the editor saves too, instead of the browser's save dialog.
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        edit.save();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [edit]);

  const copy = () => {
    void navigator.clipboard?.writeText(path).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    });
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", minHeight: 0 }}>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          height: 36,
          padding: "0 8px 0 12px",
          borderBottom: "1px solid var(--border-subtle)",
          flex: "none",
        }}
      >
        <div style={{ flex: 1, minWidth: 0, overflow: "hidden" }}>
          <Breadcrumbs
            onNavigate={(_, i) =>
              i === 0 ? nav.open(`project:${projectKey}`, { beside: true }) : shell.browseFiles(projectKey)
            }
            items={[{ label: projectKey, icon: "folder-git-2" }, ...segs.map((label) => ({ label }))]}
          />
        </div>
        {d && (
          <span
            style={{ font: "var(--text-sm)/1 var(--font-mono)", whiteSpace: "nowrap" }}
            title={`${d.added} added, ${d.removed} removed against ${d.base}`}
          >
            <span style={{ color: "var(--diff-add-fg)" }}>+{d.added}</span>{" "}
            <span style={{ color: "var(--diff-del-fg)" }}>−{d.removed}</span>
          </span>
        )}
        {views.length > 1 && !edit.editing && (
          <Tabs variant="segmented" label="View" value={view} onChange={(v) => setMode(v as View)} tabs={views} />
        )}
        {edit.editing && (
          <>
            {edit.dirty && (
              <span
                style={{ color: "var(--warn)", fontSize: "var(--text-sm)", whiteSpace: "nowrap" }}
                title="Unsaved changes"
              >
                ● unsaved
              </span>
            )}
            <Button size="sm" variant={edit.confirmDiscard ? "danger" : "ghost"} onClick={edit.discard}>
              {!edit.dirty ? "Done" : edit.confirmDiscard ? "Discard changes?" : "Discard"}
            </Button>
            <Button
              size="sm"
              variant="primary"
              icon="check"
              kbd="⌘S"
              disabled={!edit.dirty || edit.saving || edit.conflict || !!f?.read_only}
              onClick={edit.save}
            >
              {edit.saving ? "Saving…" : "Save"}
            </Button>
          </>
        )}
        {canEdit && (
          <IconButton
            size="sm"
            icon="pencil"
            label={f?.read_only ?? "Edit this file"}
            disabled={!!f?.read_only}
            onClick={edit.start}
          />
        )}
        {canPane && (
          <IconButton
            size="sm"
            icon="panel-right"
            active={showPane}
            label={showPane ? "Hide the code pane" : "Show the code pane"}
            onClick={() => setPaneOpen(!showPane)}
          />
        )}
        <IconButton
          size="sm"
          icon="message-square"
          label="Ask about this file"
          onClick={() => shell.openDock(`About ${projectKey}/${path}: `)}
        />
        <IconButton size="sm" icon={copied ? "check" : "copy"} label={copied ? "Copied" : "Copy path"} onClick={copy} />
      </div>
      {by && view === "diff" && (
        <div style={barStyle}>
          <Icon name="git-commit-horizontal" size={13} style={{ color: "var(--text-muted)" }} />
          <span style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {f?.git === "A" || f?.git === "?" ? "Added" : "Changed"} by{" "}
            <button
              type="button"
              style={linkStyle}
              onClick={() => nav.open(`exec:${by.execution}`, { beside: true })}
              title="Open the execution"
            >
              {changedByLabel(by)}
            </button>
            {by.running ? " (running)" : by.staged ? " (staged)" : ""} in{" "}
            <button
              type="button"
              style={linkStyle}
              onClick={() => nav.open(`session:${by.session}`, { beside: true })}
              title="Open the session"
            >
              {session?.title ?? session?.request ?? by.session}
            </button>
          </span>
          <span style={{ flex: 1 }} />
          <span style={{ color: "var(--text-muted)", whiteSpace: "nowrap" }}>against {d?.base ?? "HEAD"}</span>
        </div>
      )}
      <div style={{ flex: 1, display: "flex", minHeight: 0 }}>
        <div className="os-rise" key={view} style={{ flex: 1, overflow: "auto", minHeight: 0, minWidth: 0 }}>
          {file.error ? (
            <div style={{ padding: 16 }}>
              <Banner
                tone="bad"
                actions={
                  <Button size="sm" onClick={file.reload}>
                    Try again
                  </Button>
                }
              >
                {file.error.message}
              </Banner>
            </div>
          ) : !f ? (
            <div style={{ padding: 20, display: "flex", gap: 8, alignItems: "center", color: "var(--text-muted)" }}>
              <Spinner size={11} /> Reading the file…
            </div>
          ) : f.binary ? (
            <div style={{ padding: 20, color: "var(--text-muted)" }}>Binary file, {fmtSize(f.size)}. No preview.</div>
          ) : view === "diff" && d ? (
            <>
              {d.truncated && (
                <div style={{ padding: "8px 12px 0" }}>
                  <Banner tone="info">The diff is cut at the size cap. The first changes are shown.</Banner>
                </div>
              )}
              <DiffPane hunks={d.hunks} />
            </>
          ) : view === "rendered" ? (
            <div style={{ padding: "20px 28px 40px" }}>
              <Markdown text={f.content ?? ""} className="os-prose" />
            </div>
          ) : (
            <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
              {edit.editing && edit.conflict && (
                <div style={{ padding: "8px 12px 0" }}>
                  <Banner
                    tone="warn"
                    actions={
                      <>
                        <Button size="sm" onClick={edit.reload}>
                          Reload
                        </Button>
                        <Button size="sm" variant="danger" onClick={edit.overwrite} disabled={edit.saving}>
                          Overwrite
                        </Button>
                      </>
                    }
                  >
                    This file changed on disk after you started editing. Reload to take the disk version and drop your
                    changes, or overwrite it with yours.
                  </Banner>
                </div>
              )}
              {edit.editing && f.read_only && (
                <div style={{ padding: "8px 12px 0" }}>
                  <Banner tone="info">{f.read_only}</Banner>
                </div>
              )}
              {edit.editing && edit.error && (
                <div style={{ padding: "8px 12px 0" }}>
                  <Banner tone="bad">{edit.error}</Banner>
                </div>
              )}
              {f.truncated && (
                <div style={{ padding: "8px 12px 0" }}>
                  <Banner tone="info">
                    Showing the start of the file. At {fmtSize(f.size)} it is larger than the size Ostra reads for the
                    browser.
                  </Banner>
                </div>
              )}
              <div style={{ flex: 1, minHeight: 0, paddingTop: 0 }}>
                <Suspense
                  fallback={
                    <div
                      style={{ padding: 20, display: "flex", gap: 8, alignItems: "center", color: "var(--text-muted)" }}
                    >
                      <Spinner size={11} /> Loading the editor…
                    </div>
                  }
                >
                  <FileEditor
                    ws={ws}
                    projectKey={projectKey}
                    path={path}
                    value={edit.editing ? edit.draft : (f.content ?? "")}
                    onChange={edit.change}
                    theme={shell.theme}
                    onSave={edit.save}
                    dirty={edit.dirty}
                    onOpen={(p, line) => nav.open(fileId(projectKey, p), { anchor: `L${line}`, beside: true })}
                    readOnly={!edit.editing}
                    locked={locked}
                    onEditAt={(start) => {
                      setEditStart(start);
                      edit.start();
                    }}
                    start={editStart}
                    selected={edit.editing ? null : (selected?.name ?? null)}
                    onSymbol={setSelected}
                    reveal={reveal}
                    changes={edit.editing ? null : changes}
                  />
                </Suspense>
              </div>
            </div>
          )}
        </div>
        {showPane && (
          <aside
            style={{
              width: 320,
              flex: "none",
              borderLeft: "1px solid var(--border-subtle)",
              minHeight: 0,
              background: "var(--surface-panel)",
            }}
            aria-label="Code navigation"
          >
            <CodePane
              ws={ws}
              projectKey={projectKey}
              path={path}
              file={c}
              fileError={code.error}
              selected={selected}
              onSelect={setSelected}
              onGoto={gotoLine}
            />
          </aside>
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
          {f.modified && <span title={f.modified}>modified {modifiedLabel(f.modified)}</span>}
          {f.git && (
            <span style={{ color: GIT_MARK[f.git].color }}>
              {GIT_MARK[f.git].word}
              {f.staged ? ", staged" : ""}
            </span>
          )}
          {f.truncated && <span>cut at the size cap</span>}
          <span style={{ flex: 1 }} />
          <span>{edit.editing ? (edit.dirty ? "unsaved changes" : "editing") : "read-only"}</span>
        </div>
      )}
    </div>
  );
}
