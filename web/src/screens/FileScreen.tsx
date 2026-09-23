import { useState } from "react";
import { api } from "../api";
import type { ChangedBy } from "../api/types";
import { Markdown } from "../components/Markdown";
import { Banner, Breadcrumbs, Button, CodeView, Icon, IconButton, Spinner, Tabs, type TabItem } from "../design";
import { humanize } from "../lib/format";
import { useAsync } from "../lib/hooks";
import { useProjectFsChanges, useWorkspaceTree } from "../lib/live";
import { useNav, useShell } from "../lib/nav";
import { DiffPane } from "./project/DiffPane";
import { extOf, fmtSize, GIT_MARK, modifiedLabel } from "./project/files";

export type FileScreenProps = {
  ws: string;
  projectKey: string;
  /** Project-relative, `/`-separated. */
  path: string;
};

type View = "file" | "rendered" | "diff";

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

const linkStyle = { background: "none", border: 0, padding: 0, font: "inherit", fontWeight: 500, color: "var(--text-primary)", cursor: "pointer" } as const;

/**
 * Resource `file:<key>:<path>`: a read-only file using the whole center pane, rendered markdown for `.md`,
 * or its Changes view (unified diff against HEAD) when a session touched it. Refreshes when the file
 * changes on disk. Scrolls its own content.
 */
export function FileScreen({ ws, projectKey, path }: FileScreenProps) {
  const nav = useNav();
  const shell = useShell();
  const tree = useWorkspaceTree(ws);
  const file = useAsync(() => api.projectFile(ws, projectKey, path), [ws, projectKey, path]);
  const changed = !!file.data?.git;
  const diff = useAsync(() => (changed ? api.projectDiff(ws, projectKey, path) : Promise.resolve(null)), [ws, projectKey, path, changed]);
  const [mode, setMode] = useState<View | null>(null);
  const [copied, setCopied] = useState(false);
  useProjectFsChanges(ws, projectKey, (paths) => {
    if (paths.includes(path)) {
      file.reload();
      diff.reload();
    }
  });

  const f = file.data;
  const d = diff.data && diff.data.hunks.length > 0 ? diff.data : null;
  const ext = extOf(path);
  const md = ext === "md" || ext === "markdown";
  const views: TabItem[] = [{ id: "file", label: "File", icon: "file-text" }];
  if (md && f?.content) views.push({ id: "rendered", label: "Rendered", icon: "book-open" });
  if (d) views.push({ id: "diff", label: "Changes", icon: "file-diff" });
  const view: View = mode && views.some((v) => v.id === mode) ? mode : d ? "diff" : "file";
  const lines = f?.content ? f.content.replace(/\n$/, "").split("\n").length : null;
  const by = f?.changed_by ?? d?.changed_by ?? null;
  const session = by ? tree.sessions.find((s) => s.id === by.session) : null;
  const segs = path.split("/");

  const copy = () => {
    void navigator.clipboard?.writeText(path).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    });
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", minHeight: 0 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 8, height: 36, padding: "0 8px 0 12px", borderBottom: "1px solid var(--border-subtle)", flex: "none" }}>
        <div style={{ flex: 1, minWidth: 0, overflow: "hidden" }}>
          <Breadcrumbs
            onNavigate={(_, i) => (i === 0 ? nav.open(`project:${projectKey}`) : shell.browseFiles(projectKey))}
            items={[{ label: projectKey, icon: "folder-git-2" }, ...segs.map((label) => ({ label }))]}
          />
        </div>
        {d && (
          <span style={{ font: "var(--text-sm)/1 var(--font-mono)", whiteSpace: "nowrap" }} title={`${d.added} added, ${d.removed} removed against ${d.base}`}>
            <span style={{ color: "var(--diff-add-fg)" }}>+{d.added}</span> <span style={{ color: "var(--diff-del-fg)" }}>−{d.removed}</span>
          </span>
        )}
        {views.length > 1 && <Tabs variant="segmented" label="View" value={view} onChange={(v) => setMode(v as View)} tabs={views} />}
        <IconButton size="sm" icon="message-square" label="Ask about this file" onClick={() => shell.openDock(`About ${projectKey}/${path}: `)} />
        <IconButton size="sm" icon={copied ? "check" : "copy"} label={copied ? "Copied" : "Copy path"} onClick={copy} />
      </div>
      {by && view === "diff" && (
        <div style={barStyle}>
          <Icon name="git-commit-horizontal" size={13} style={{ color: "var(--text-muted)" }} />
          <span style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {f?.git === "A" || f?.git === "?" ? "Added" : "Changed"} by{" "}
            <button type="button" style={linkStyle} onClick={() => nav.open(`exec:${by.execution}`)} title="Open the execution">
              {changedByLabel(by)}
            </button>
            {by.running ? " (running)" : by.staged ? " (staged)" : ""} in{" "}
            <button type="button" style={linkStyle} onClick={() => nav.open(`session:${by.session}`)} title="Open the session">
              {session?.title ?? session?.request ?? by.session}
            </button>
          </span>
          <span style={{ flex: 1 }} />
          <span style={{ color: "var(--text-muted)", whiteSpace: "nowrap" }}>against {d?.base ?? "HEAD"}</span>
        </div>
      )}
      <div className="os-rise" key={view} style={{ flex: 1, overflow: "auto", minHeight: 0 }}>
        {file.error ? (
          <div style={{ padding: 16 }}>
            <Banner tone="bad" actions={<Button size="sm" onClick={file.reload}>Try again</Button>}>
              {file.error.message}
            </Banner>
          </div>
        ) : !f ? (
          <div style={{ padding: 20, display: "flex", gap: 8, alignItems: "center", color: "var(--text-muted)" }}>
            <Spinner size={11} /> Reading the file…
          </div>
        ) : f.binary ? (
          <div style={{ padding: 20, color: "var(--text-muted)" }}>Binary file, {fmtSize(f.size)}. No preview.</div>
        ) : (
          <>
            {f.truncated && view !== "diff" && (
              <div style={{ padding: "8px 12px 0" }}>
                <Banner tone="info">Showing the start of the file. At {fmtSize(f.size)} it is larger than the size Ostra reads for the browser.</Banner>
              </div>
            )}
            {view === "diff" && d ? (
              <>
                {d.truncated && (
                  <div style={{ padding: "8px 12px 0" }}>
                    <Banner tone="info">The diff is cut at the size cap. The first changes are shown.</Banner>
                  </div>
                )}
                <DiffPane hunks={d.hunks} />
              </>
            ) : view === "rendered" ? (
              <div style={{ maxWidth: 760, padding: "20px 28px 40px" }}>
                <Markdown text={f.content ?? ""} className="os-prose" />
              </div>
            ) : (
              <CodeView flush language={ext} code={f.content ?? ""} />
            )}
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
          {f.modified && <span title={f.modified}>modified {modifiedLabel(f.modified)}</span>}
          {f.git && <span style={{ color: GIT_MARK[f.git].color }}>{GIT_MARK[f.git].word}{f.staged ? ", staged" : ""}</span>}
          {f.truncated && <span>cut at the size cap</span>}
          <span style={{ flex: 1 }} />
          <span>read-only</span>
        </div>
      )}
    </div>
  );
}
