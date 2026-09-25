import { Fragment, type MouseEvent, type ReactNode, useState } from "react";
import type { TreeGroup, TreeSession } from "../api/nav";
import type { ExecutionStatus, ProjectView, SessionStatus } from "../api/types";
import { Chip, IconButton, Input, Spinner, StatusDot, Tabs, type Tone, TreeItem, TreeSection } from "../design";
import { formatCost, humanize } from "../lib/format";
import type { OpenOptions } from "../lib/nav";
import { parseResource } from "../lib/resource";
import { FilesPanel } from "./FilesPanel";
import { GitPanel } from "./GitPanel";
import { sessionLabel } from "./meta";
import type { LeftTab } from "./uiState";

const SESSION_DOT: Record<SessionStatus, [Tone, boolean?]> = {
  running: ["accent", true],
  waiting: ["warn"],
  completed: ["ok"],
  failed: ["bad"],
  stalled: ["bad"],
};

export function SessionDot({ status }: { status: SessionStatus }) {
  const [tone, pulse] = SESSION_DOT[status] ?? ["neutral"];
  return <StatusDot tone={tone} pulse={pulse} />;
}

const EXEC_TONE: Record<ExecutionStatus, Tone> = {
  running: "accent",
  ok: "ok",
  stuck: "warn",
  handoff: "info",
  error: "bad",
  denied: "bad",
  interrupted: "warn",
  cancelled: "neutral",
};

function RunIcon({ status }: { status: ExecutionStatus }) {
  if (status === "running") return <Spinner size={10} style={{ color: "var(--accent)", margin: "0 2px" }} />;
  return <StatusDot tone={EXEC_TONE[status]} size={6} />;
}

/** The session a resource belongs to, so the tree can expand it. */
function sessionOf(activeId: string | null, sessions: TreeSession[]): string | null {
  const r = activeId ? parseResource(activeId) : null;
  if (!r) return null;
  if (r.type === "session") return r.id;
  if (r.type === "exec")
    return sessions.find((s) => s.groups.some((g) => g.runs.some((x) => x.id === r.id)))?.id ?? null;
  if (r.type === "artifact") return sessions.find((s) => s.artifacts.some((a) => a.path === r.path))?.id ?? null;
  return null;
}

type OpenFn = (id: string, opts?: OpenOptions) => void;

function GroupRows({
  g,
  activeId,
  open,
  expanded,
  toggle,
}: {
  g: TreeGroup;
  activeId: string | null;
  open: OpenFn;
  expanded: (k: string, d: boolean) => boolean;
  toggle: (k: string, d: boolean) => void;
}) {
  if (g.runs.length === 1) {
    const x = g.runs[0];
    return (
      <TreeItem
        depth={2}
        label={humanize(g.agent)}
        meta={g.project}
        icon={<RunIcon status={x.status} />}
        selected={activeId === `exec:${x.id}`}
        onClick={() => open(`exec:${x.id}`, { preview: true })}
        title={[`${humanize(g.agent)} · ${x.run_label}`, x.summary].filter(Boolean).join("\n")}
      />
    );
  }
  const key = `g:${g.group}`;
  const def = g.status === "running" || g.runs.some((x) => activeId === `exec:${x.id}`);
  const isOpen = expanded(key, def);
  return (
    <>
      <TreeItem
        depth={2}
        label={
          <span>
            {humanize(g.agent)} <span style={{ color: "var(--text-muted)" }}>×{g.runs.length}</span>
          </span>
        }
        meta={g.project}
        icon={<RunIcon status={g.status} />}
        expanded={isOpen}
        onToggle={() => toggle(key, def)}
        title={`${humanize(g.agent)} in ${g.project}`}
      />
      {isOpen &&
        g.runs.map((x) => (
          <TreeItem
            key={x.id}
            depth={3}
            label={x.run_label}
            icon={<RunIcon status={x.status} />}
            meta={x.stream === "terminal" ? "terminal" : null}
            selected={activeId === `exec:${x.id}`}
            onClick={() => open(`exec:${x.id}`, { preview: true })}
            title={x.summary ?? x.run_label}
          />
        ))}
    </>
  );
}

type SessionsPanelProps = {
  sessions: TreeSession[];
  loading: boolean;
  error: Error | null;
  projects: ProjectView[];
  activeId: string | null;
  open: OpenFn;
  onAddProject: () => void;
};

function SessionsPanel({ sessions, loading, error, projects, activeId, open, onAddProject }: SessionsPanelProps) {
  const [overrides, setOverrides] = useState<Record<string, boolean>>({});
  const [filter, setFilter] = useState("");
  const current = sessionOf(activeId, sessions);
  const expanded = (k: string, def: boolean) => overrides[k] ?? def;
  const toggle = (k: string, def: boolean) => setOverrides((o) => ({ ...o, [k]: !(o[k] ?? def) }));
  const f = filter.trim().toLowerCase();
  const shown = f ? sessions.filter((s) => `${s.title ?? ""} ${s.request}`.toLowerCase().includes(f)) : sessions;

  let body: ReactNode;
  if (error && sessions.length === 0)
    body = <div style={{ padding: "6px 8px", color: "var(--bad)", fontSize: "var(--text-sm)" }}>{error.message}</div>;
  else if (loading && sessions.length === 0)
    body = (
      <div
        style={{
          padding: "6px 8px",
          display: "flex",
          gap: 8,
          alignItems: "center",
          color: "var(--text-muted)",
          fontSize: "var(--text-sm)",
        }}
      >
        <Spinner size={10} /> Reading the sessions…
      </div>
    );
  else if (sessions.length === 0)
    body = (
      <div style={{ padding: "6px 8px", color: "var(--text-muted)", fontSize: "var(--text-sm)" }}>
        No sessions yet. Start one with the New task form.
      </div>
    );
  else if (shown.length === 0)
    body = (
      <div style={{ padding: "6px 8px", color: "var(--text-muted)", fontSize: "var(--text-sm)" }}>
        No session matches.
      </div>
    );
  else
    body = shown.map((s) => {
      const isOpen = expanded(s.id, s.id === current);
      const runs = s.groups.reduce((n, g) => n + g.runs.length, 0);
      const xKey = `${s.id}:x`;
      const aKey = `${s.id}:a`;
      const aDefault = !!activeId?.startsWith("artifact:") && s.id === current;
      return (
        <Fragment key={s.id}>
          <TreeItem
            label={sessionLabel(s)}
            icon={<SessionDot status={s.status} />}
            expanded={isOpen}
            meta={s.open_gates ? null : formatCost(s.cost_usd)}
            trailing={s.open_gates ? <Chip tone="warn">{s.open_gates}</Chip> : null}
            selected={activeId === `session:${s.id}`}
            onToggle={() => toggle(s.id, s.id === current)}
            onClick={() => open(`session:${s.id}`, { preview: true })}
            title={s.request}
          />
          {isOpen && runs === 0 && s.artifacts.length === 0 && (
            <TreeItem depth={1} label="Board" icon="kanban" onClick={() => open(`session:${s.id}`)} />
          )}
          {isOpen && runs > 0 && (
            <>
              <TreeItem
                depth={1}
                label="Executions"
                icon="square-terminal"
                expanded={expanded(xKey, true)}
                meta={runs}
                onToggle={() => toggle(xKey, true)}
              />
              {expanded(xKey, true) &&
                s.groups.map((g) => (
                  <GroupRows key={g.group} g={g} activeId={activeId} open={open} expanded={expanded} toggle={toggle} />
                ))}
            </>
          )}
          {isOpen && s.artifacts.length > 0 && (
            <>
              <TreeItem
                depth={1}
                label="Artifacts"
                icon="files"
                expanded={expanded(aKey, aDefault)}
                meta={s.artifacts.length}
                onToggle={() => toggle(aKey, aDefault)}
              />
              {expanded(aKey, aDefault) &&
                s.artifacts.map((a) => (
                  <TreeItem
                    key={a.path}
                    depth={2}
                    label={a.label}
                    icon="file-text"
                    selected={activeId === `artifact:${a.path}`}
                    onClick={() => open(`artifact:${a.path}`, { preview: true })}
                    title={a.path}
                  />
                ))}
            </>
          )}
        </Fragment>
      );
    });

  return (
    <div
      role="tree"
      aria-label="Sessions"
      style={{ padding: "8px 8px 4px", flex: 1, overflowY: "auto", overflowX: "hidden" }}
    >
      <TreeSection
        label="Sessions"
        actions={<IconButton size="sm" icon="plus" label="New task" onClick={() => open("ws:overview")} />}
      >
        <div style={{ padding: "2px 0 6px" }}>
          <Input
            size="sm"
            icon="list-filter"
            placeholder="Filter"
            aria-label="Filter sessions"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
        </div>
        {body}
      </TreeSection>
      <div style={{ height: 8 }} />
      <TreeSection
        label="Projects"
        actions={<IconButton size="sm" icon="plus" label="Add project" onClick={onAddProject} />}
      >
        {projects.length === 0 && (
          <div style={{ padding: "6px 8px", color: "var(--text-muted)", fontSize: "var(--text-sm)" }}>
            No projects yet. Add one to start work.
          </div>
        )}
        {projects.map((p) => (
          <TreeItem
            key={p.key}
            label={p.key}
            icon="folder-git-2"
            meta={p.init_status === "initialized" ? null : "not init"}
            trailing={p.init_status !== "initialized" ? <StatusDot tone="warn" size={6} /> : null}
            selected={activeId === `project:${p.key}`}
            onClick={() => open(`project:${p.key}`, { preview: true })}
            title={p.path}
          />
        ))}
      </TreeSection>
    </div>
  );
}

export type SidebarProps = {
  ws: string;
  /** The drag handle on the right edge. */
  resizer?: ReactNode;
  tab: LeftTab;
  setTab: (t: LeftTab) => void;
  sessions: TreeSession[];
  loading: boolean;
  error: Error | null;
  projects: ProjectView[];
  activeId: string | null;
  open: OpenFn;
  /** Double-click on the selected row pins the tab that the first click opened as a preview. */
  pinActive: () => void;
  filesProject: string | null;
  setFilesProject: (key: string) => void;
  onAddProject: () => void;
};

export function Sidebar({
  ws,
  resizer,
  tab,
  setTab,
  sessions,
  loading,
  error,
  projects,
  activeId,
  open,
  pinActive,
  filesProject,
  setFilesProject,
  onAddProject,
}: SidebarProps) {
  const onDoubleClick = (e: MouseEvent) => {
    const row = (e.target as HTMLElement).closest('[role="treeitem"]');
    if (row?.getAttribute("aria-selected") === "true") pinActive();
  };
  const selectedFile = (() => {
    const r = activeId ? parseResource(activeId) : null;
    return r?.type === "file" ? { key: r.key, path: r.path } : null;
  })();
  return (
    <aside
      onDoubleClick={onDoubleClick}
      style={{
        position: "relative",
        width: "var(--sidebar-w)",
        flex: "none",
        background: "var(--surface-panel)",
        borderRight: "1px solid var(--border-default)",
        display: "flex",
        flexDirection: "column",
        minHeight: 0,
      }}
    >
      <div style={{ padding: "0 8px", flex: "none" }}>
        <Tabs
          label="Left dock"
          value={tab}
          onChange={(t) => setTab(t as LeftTab)}
          tabs={[
            { id: "sessions", label: "Sessions", icon: "git-pull-request", count: sessions.length },
            { id: "files", label: "Files", icon: "folder-tree" },
            { id: "git", label: "Git", icon: "git-branch" },
          ]}
        />
      </div>
      {tab === "files" ? (
        <FilesPanel
          ws={ws}
          projects={projects}
          project={filesProject}
          setProject={setFilesProject}
          selected={selectedFile}
          onOpenFile={(key, path, opts) =>
            open(`file:${key}:${path}`, opts?.edit ? { anchor: "edit" } : { preview: true })
          }
          onOpenProject={(key) => open(`project:${key}`)}
          onAddProject={onAddProject}
        />
      ) : tab === "git" ? (
        <GitPanel
          ws={ws}
          projects={projects}
          project={filesProject}
          setProject={setFilesProject}
          selected={selectedFile}
          onOpenFile={(key, path) => open(`file:${key}:${path}`, { preview: true })}
          onAddProject={onAddProject}
        />
      ) : (
        <SessionsPanel
          sessions={sessions}
          loading={loading}
          error={error}
          projects={projects}
          activeId={activeId}
          open={open}
          onAddProject={onAddProject}
        />
      )}
      {resizer}
    </aside>
  );
}
