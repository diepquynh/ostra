// Live workspace data for the shell and the screens. Every hook here shares one fetch and one socket
// subscription per workspace (see SharedStore), so any number of components may call them.

import { useEffect, useRef, useState } from "react";
import { api, socket } from "../api";
import type { SocketState } from "../api/extra";
import type { FileIndex } from "../api/gen/FileIndex";
import type { ProjectChange } from "../api/gen/ProjectChange";
import type { SearchHit, TreeSession, WorkspaceActivity } from "../api/nav";
import type { WireMsg } from "../api/socket";
import type { SessionSummary, WorkspaceSummary } from "../api/types";
import { SharedStore, throttle, useShared } from "./store";

// ---------------------------------------------------------------------------------------------
// Sessions tree
// ---------------------------------------------------------------------------------------------

export type TreeData = { sessions: TreeSession[] };

const byUpdated = (a: TreeSession, b: TreeSession) => b.updated_at.localeCompare(a.updated_at);

/** A tree row from a list summary, keeping the executions and artifacts the tree already has. */
export function summaryToTree(s: SessionSummary, prev?: TreeSession): TreeSession {
  return {
    id: s.id,
    title: s.title,
    request: s.request,
    kind: s.kind,
    status: s.status,
    open_gates: s.open_gates,
    cost_usd: s.cost_usd,
    updated_at: s.updated_at,
    groups: prev?.groups ?? [],
    artifacts: prev?.artifacts ?? [],
  };
}

function upsert(list: TreeSession[], s: TreeSession): TreeSession[] {
  const i = list.findIndex((x) => x.id === s.id);
  const next = i >= 0 ? list.map((x, j) => (j === i ? s : x)) : [s, ...list];
  return next.sort(byUpdated);
}

export const treeStore = new SharedStore<TreeData>({
  load: async (ws) => ({ sessions: [...(await api.tree(ws)).sessions].sort(byUpdated) }),
  live: (ws, feed) =>
    socket().subscribe(`workspace:${ws}`, (m: WireMsg) => {
      if (m.type === "tree_patch") {
        feed.update((prev) => (prev ? { sessions: upsert(prev.sessions, m.session) } : prev));
      } else if (m.type === "session_updated") {
        feed.update((prev) => {
          if (!prev) return prev;
          const old = prev.sessions.find((s) => s.id === m.summary.id);
          return { sessions: upsert(prev.sessions, summaryToTree(m.summary, old)) };
        });
      }
    }),
});

/** The Sessions tree of a workspace: sessions, their execution groups and runs, and artifacts. */
export function useWorkspaceTree(ws: string | null) {
  const s = useShared(treeStore, ws);
  return { sessions: s.data?.sessions ?? [], data: s.data, loading: s.loading, error: s.error, reload: s.reload };
}

// ---------------------------------------------------------------------------------------------
// Session summaries (the list endpoint; it carries `yolo`, which the tree does not)
// ---------------------------------------------------------------------------------------------

export const summariesStore = new SharedStore<SessionSummary[]>({
  load: (ws) => api.sessions(ws),
  live: (ws, feed) =>
    socket().subscribe(`workspace:${ws}`, (m) => {
      if (m.type !== "session_updated") return;
      feed.update((prev) => {
        const list = prev ?? [];
        const i = list.findIndex((s) => s.id === m.summary.id);
        return i >= 0 ? list.map((s, j) => (j === i ? m.summary : s)) : [m.summary, ...list];
      });
    }),
});

export function useSessionSummaries(ws: string | null) {
  const s = useShared(summariesStore, ws);
  return { sessions: s.data ?? [], loading: s.loading, error: s.error, reload: s.reload };
}

// ---------------------------------------------------------------------------------------------
// Workspace activity (status bar)
// ---------------------------------------------------------------------------------------------

export type LiveActivity = WorkspaceActivity;

export const activityStore = new SharedStore<LiveActivity>({
  load: (ws) => api.workspaceActivity(ws),
  live: (ws, feed) =>
    socket().subscribe(`workspace:${ws}`, (m) => {
      if (m.type === "activity") feed.update(() => m.activity);
    }),
});

/** Running executions, open gates, and spend today and this week. */
export function useActivity(ws: string | null) {
  const s = useShared(activityStore, ws);
  return { activity: s.data, loading: s.loading, error: s.error, reload: s.reload };
}

// ---------------------------------------------------------------------------------------------
// Project files
// ---------------------------------------------------------------------------------------------

const projectKey = (ws: string, key: string) => `${ws}\n${key}`;
const splitKey = (k: string) => k.split("\n") as [string, string];

/** Reload an entry whenever `project_fs_changed` names its project, at most once a second. */
function reloadOnFsChange(k: string, reload: () => void) {
  const [ws, key] = splitKey(k);
  const later = throttle(reload, 1000);
  const off = socket().subscribe(`workspace:${ws}`, (m) => {
    if (m.type === "project_fs_changed" && m.key === key) later();
  });
  return () => {
    off();
    later.cancel();
  };
}

export const fileIndexStore = new SharedStore<FileIndex>({
  load: (k) => api.projectFiles(...splitKey(k)),
  live: (k, feed) => reloadOnFsChange(k, feed.reload),
});

export const changesStore = new SharedStore<ProjectChange[]>({
  load: (k) => api.projectChanges(...splitKey(k)),
  live: (k, feed) => reloadOnFsChange(k, feed.reload),
});

/** Every non-ignored file path of a project, kept current on `project_fs_changed`. */
export function useFileIndex(ws: string | null, key: string | null) {
  const s = useShared(fileIndexStore, ws && key ? projectKey(ws, key) : null);
  return { paths: s.data?.paths ?? [], truncated: s.data?.truncated ?? false, loading: s.loading, error: s.error, reload: s.reload };
}

/** Files that sessions changed in a project and that still differ from HEAD. */
export function useProjectChanges(ws: string | null, key: string | null) {
  const s = useShared(changesStore, ws && key ? projectKey(ws, key) : null);
  return { changes: s.data ?? [], loading: s.loading, error: s.error, reload: s.reload };
}

/** Call `onChange(paths)` whenever files of project `key` change on disk. */
export function useProjectFsChanges(ws: string | null, key: string | null, onChange: (paths: string[]) => void): void {
  const ref = useRef(onChange);
  ref.current = onChange;
  useEffect(() => {
    if (!ws || !key) return;
    return socket().subscribe(`workspace:${ws}`, (m) => {
      if (m.type === "project_fs_changed" && m.key === key) ref.current(m.paths);
    });
  }, [ws, key]);
}

// ---------------------------------------------------------------------------------------------
// Workspaces, connection, search
// ---------------------------------------------------------------------------------------------

export const workspacesStore = new SharedStore<WorkspaceSummary[]>({
  load: () => api.workspaces(),
  live: (_k, feed) => {
    const later = throttle(feed.reload, 1000);
    const off = socket().subscribe("home", (m) => {
      if (m.type === "workspace_updated" || m.type === "session_updated") later();
    });
    return () => {
      off();
      later.cancel();
    };
  },
});

export function useWorkspaces() {
  const s = useShared(workspacesStore, "all");
  return { workspaces: s.data ?? [], loading: s.loading, error: s.error, reload: s.reload };
}

/** The socket's connection state. */
export function useSocketState(): SocketState {
  const [state, setState] = useState<SocketState>(() => socket().state);
  useEffect(() => socket().onState(setState), []);
  return state;
}

export type SearchState = {
  items: SearchHit[];
  loading: boolean;
  /** The last request failed, so callers search the rows they hold instead. */
  unavailable: boolean;
};

/** Server search, debounced. An empty query returns no items without a request. */
export function useSearch(ws: string | null, query: string, delayMs = 100): SearchState {
  const [state, setState] = useState<SearchState>({ items: [], loading: false, unavailable: false });
  useEffect(() => {
    const q = query.trim();
    if (!ws || !q) {
      setState({ items: [], loading: false, unavailable: false });
      return;
    }
    let alive = true;
    setState((s) => ({ ...s, loading: true }));
    const timer = setTimeout(() => {
      api.search(ws, q).then(
        (r) => alive && setState({ items: r.items, loading: false, unavailable: false }),
        () => alive && setState({ items: [], loading: false, unavailable: true }),
      );
    }, delayMs);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [ws, query, delayMs]);
  return state;
}
