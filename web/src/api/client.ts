import type {
  ActivityItem,
  AnswerGate,
  Artifact,
  AskQuestion,
  AskStarted,
  CostReport,
  CreateSession,
  CreateWorkspace,
  DecisionView,
  ExecutionView,
  FsListing,
  GateView,
  ImportProject,
  Lesson,
  LessonEdit,
  OverrideDecision,
  PushSubscription,
  ServerInfo,
  SessionDetail,
  SessionSummary,
  StoredEvent,
  ValidationIssue,
  WorkspaceDetail,
  WorkspaceSettings,
  WorkspaceSummary,
} from "./types";
import type { DiffFile } from "./extra";

/** An HTTP error from the server, with validation issues when the server sent them. */
export class HttpError extends Error {
  status: number;
  issues: ValidationIssue[];
  constructor(status: number, message: string, issues: ValidationIssue[] = []) {
    super(message);
    this.status = status;
    this.issues = issues;
  }
}

type Listener = () => void;
const unauthorizedListeners = new Set<Listener>();

/** Called whenever a request comes back 401, so the app can show the sign-in screen. */
export function onUnauthorized(fn: Listener): () => void {
  unauthorizedListeners.add(fn);
  return () => unauthorizedListeners.delete(fn);
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(path, {
    method,
    credentials: "same-origin",
    headers: body === undefined ? {} : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (res.status === 401) {
    unauthorizedListeners.forEach((fn) => fn());
  }
  if (!res.ok) {
    let message = `${res.status} ${res.statusText}`;
    let issues: ValidationIssue[] = [];
    try {
      const data = await res.json();
      if (data && typeof data.error === "string") message = data.error;
      if (data && Array.isArray(data.issues)) issues = data.issues;
    } catch {
      // Body was not JSON; keep the status line.
    }
    throw new HttpError(res.status, message, issues);
  }
  if (res.status === 204) return undefined as T;
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

const q = (params: Record<string, string | number | undefined | null>): string => {
  const parts = Object.entries(params)
    .filter(([, v]) => v !== undefined && v !== null && v !== "")
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(String(v))}`);
  return parts.length ? `?${parts.join("&")}` : "";
};

const enc = encodeURIComponent;

export const httpApi = {
  info: () => request<ServerInfo>("GET", "/api/info"),
  exchange: (token: string) => request<void>("POST", "/api/auth/exchange", { token }),

  workspaces: () => request<WorkspaceSummary[]>("GET", "/api/workspaces"),
  createWorkspace: (body: CreateWorkspace) => request<WorkspaceDetail>("POST", "/api/workspaces", body),
  workspace: (ws: string) => request<WorkspaceDetail>("GET", `/api/workspaces/${enc(ws)}`),
  saveSettings: (ws: string, settings: WorkspaceSettings) =>
    request<WorkspaceDetail>("PATCH", `/api/workspaces/${enc(ws)}`, settings),
  validateSettings: (ws: string, settings: WorkspaceSettings) =>
    request<ValidationIssue[]>("POST", `/api/workspaces/${enc(ws)}/validate`, settings),
  importProject: (ws: string, body: ImportProject) =>
    request<WorkspaceDetail>("POST", `/api/workspaces/${enc(ws)}/projects`, body),
  removeProject: (ws: string, key: string) =>
    request<WorkspaceDetail>("DELETE", `/api/workspaces/${enc(ws)}/projects/${enc(key)}`),
  initProject: (ws: string, key: string) =>
    request<SessionSummary>("POST", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/init`),

  sessions: (ws: string) => request<SessionSummary[]>("GET", `/api/workspaces/${enc(ws)}/sessions`),
  createSession: (ws: string, body: CreateSession) =>
    request<SessionSummary>("POST", `/api/workspaces/${enc(ws)}/sessions`, body),
  session: (id: string) => request<SessionDetail>("GET", `/api/sessions/${enc(id)}`),
  events: (id: string, after?: number) =>
    request<StoredEvent[]>("GET", `/api/sessions/${enc(id)}/events${q({ after })}`),
  setYolo: (id: string, enabled: boolean) =>
    request<SessionSummary>("POST", `/api/sessions/${enc(id)}/yolo`, { enabled }),
  amend: (id: string, text: string) => request<SessionSummary>("POST", `/api/sessions/${enc(id)}/amend`, { text }),

  answerGate: (id: string, body: AnswerGate) => request<GateView>("POST", `/api/gates/${enc(id)}/answer`, body),
  overrideDecision: (id: string, body: OverrideDecision) =>
    request<DecisionView>("POST", `/api/decisions/${enc(id)}/override`, body),

  execution: (id: string) => request<ExecutionView>("GET", `/api/executions/${enc(id)}`),
  activity: (id: string, after?: number) =>
    request<ActivityItem[]>("GET", `/api/executions/${enc(id)}/activity${q({ after })}`),
  cancelExecution: (id: string) => request<ExecutionView>("POST", `/api/executions/${enc(id)}/cancel`),
  resumeExecution: (id: string) => request<ExecutionView>("POST", `/api/executions/${enc(id)}/resume`),

  artifact: (path: string) => request<Artifact>("GET", `/api/artifacts${q({ path })}`),
  diff: (session: string, project: string, phase: string) =>
    request<DiffFile[]>("GET", `/api/sessions/${enc(session)}/diff${q({ project, phase })}`),

  lessons: (ws: string, key: string, query?: string) =>
    request<Lesson[]>("GET", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/memory${q({ q: query })}`),
  saveLesson: (ws: string, key: string, body: LessonEdit) =>
    request<Lesson>("PATCH", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/memory`, body),
  deleteLesson: (ws: string, key: string, id: number) =>
    request<void>("DELETE", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/memory${q({ id })}`),

  cost: (ws: string) => request<CostReport>("GET", `/api/workspaces/${enc(ws)}/cost`),
  ask: (ws: string, body: AskQuestion) => request<AskStarted>("POST", `/api/workspaces/${enc(ws)}/ask`, body),
  pushSubscribe: (body: PushSubscription) => request<void>("POST", "/api/push/subscribe", body),
  listDir: (path?: string) => request<FsListing>("GET", `/api/fs/list${q({ path })}`),
};

export type Api = typeof httpApi;
