import type {
  ActivityItem,
  AnswerGate,
  AskQuestion,
  AskStarted,
  CostReport,
  CreateSession,
  CreateWorkspace,
  DecisionView,
  ExecutionView,
  FsListing,
  GateView,
  CloneProject,
  GitBranch,
  GitCheckoutRequest,
  GitCredentialEdit,
  GitCredentialView,
  GitOpResult,
  GitPullResult,
  GitRepoStatus,
  ImportProject,
  Lesson,
  LessonEdit,
  ProjectSkills,
  SkillAdopt,
  SkillDoc,
  SkillSave,
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
import type { CodeDeps } from "./gen/CodeDeps";
import type { CodeFile } from "./gen/CodeFile";
import type { CodeSymbols } from "./gen/CodeSymbols";
import type { CodeUsages } from "./gen/CodeUsages";
import type { DiffFile } from "./gen/DiffFile";
import type { EnvironmentStatus } from "./gen/EnvironmentStatus";
import type { HarnessSetupAction } from "./gen/HarnessSetupAction";
import type { HarnessSetupTerminal } from "./gen/HarnessSetupTerminal";
import type { ProviderCredentialsEdit } from "./gen/ProviderCredentialsEdit";
import type { ProviderStatus } from "./gen/ProviderStatus";
import type { FileDiff } from "./gen/FileDiff";
import type { FileIndex } from "./gen/FileIndex";
import type { FsBrowse } from "./gen/FsBrowse";
import type { OnboardingState } from "./gen/OnboardingState";
import type { ProjectChange } from "./gen/ProjectChange";
import type { ProjectFile } from "./gen/ProjectFile";
import type { SaveProjectFile } from "./gen/SaveProjectFile";
import type { ProjectTree } from "./gen/ProjectTree";
import type { WorkspaceUiState } from "./gen/WorkspaceUiState";
import type { ArtifactWithHeadings, SearchResults, WorkspaceActivity, WorkspaceTree } from "./nav";

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

async function request<T>(method: string, path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
  const res = await fetch(path, {
    method,
    signal,
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
  deleteWorkspace: (ws: string) => request<void>("DELETE", `/api/workspaces/${enc(ws)}`),
  validateSettings: (ws: string, settings: WorkspaceSettings) =>
    request<ValidationIssue[]>("POST", `/api/workspaces/${enc(ws)}/validate`, settings),
  importProject: (ws: string, body: ImportProject) =>
    request<WorkspaceDetail>("POST", `/api/workspaces/${enc(ws)}/projects`, body),
  cloneProject: (ws: string, body: CloneProject) => request<WorkspaceDetail>("POST", `/api/workspaces/${enc(ws)}/clone`, body),
  pullProject: (ws: string, key: string) => request<GitPullResult>("POST", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/pull`),
  gitStatus: (ws: string, key: string) => request<GitRepoStatus>("GET", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/git`),
  gitBranches: (ws: string, key: string) => request<GitBranch[]>("GET", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/git/branches`),
  /** An empty list stages every change in the project. */
  gitStage: (ws: string, key: string, paths: string[]) => request<GitOpResult>("POST", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/git/stage`, { paths }),
  gitUnstage: (ws: string, key: string, paths: string[]) => request<GitOpResult>("POST", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/git/unstage`, { paths }),
  gitCommit: (ws: string, key: string, message: string) => request<GitOpResult>("POST", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/git/commit`, { message }),
  gitFetch: (ws: string, key: string) => request<GitOpResult>("POST", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/git/fetch`),
  gitPush: (ws: string, key: string) => request<GitOpResult>("POST", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/git/push`),
  gitCheckout: (ws: string, key: string, body: GitCheckoutRequest) => request<GitOpResult>("POST", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/git/checkout`, body),
  gitCredentials: () => request<GitCredentialView[]>("GET", "/api/git/credentials"),
  createGitCredential: (edit: GitCredentialEdit) => request<GitCredentialView[]>("POST", "/api/git/credentials", edit),
  updateGitCredential: (id: string, edit: GitCredentialEdit) => request<GitCredentialView[]>("PATCH", `/api/git/credentials/${enc(id)}`, edit),
  deleteGitCredential: (id: string) => request<GitCredentialView[]>("DELETE", `/api/git/credentials/${enc(id)}`),
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
  stopSession: (id: string) => request<SessionSummary>("POST", `/api/sessions/${enc(id)}/stop`),

  answerGate: (id: string, body: AnswerGate) => request<GateView>("POST", `/api/gates/${enc(id)}/answer`, body),
  overrideDecision: (id: string, body: OverrideDecision) =>
    request<DecisionView>("POST", `/api/decisions/${enc(id)}/override`, body),

  execution: (id: string) => request<ExecutionView>("GET", `/api/executions/${enc(id)}`),
  activity: (id: string, after?: number) =>
    request<ActivityItem[]>("GET", `/api/executions/${enc(id)}/activity${q({ after })}`),
  cancelExecution: (id: string) => request<ExecutionView>("POST", `/api/executions/${enc(id)}/cancel`),
  resumeExecution: (id: string) => request<ExecutionView>("POST", `/api/executions/${enc(id)}/resume`),

  artifact: (path: string) => request<ArtifactWithHeadings>("GET", `/api/artifacts${q({ path })}`),
  diff: (session: string, project: string, phase: string) =>
    request<DiffFile[]>("GET", `/api/sessions/${enc(session)}/diff${q({ project, phase })}`),

  lessons: (ws: string, key: string, query?: string) =>
    request<Lesson[]>("GET", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/memory${q({ q: query })}`),
  saveLesson: (ws: string, key: string, body: LessonEdit) =>
    request<Lesson>("PATCH", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/memory`, body),
  deleteLesson: (ws: string, key: string, id: number) =>
    request<void>("DELETE", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/memory${q({ id })}`),

  skills: (ws: string) => request<ProjectSkills[]>("GET", `/api/workspaces/${enc(ws)}/skills`),
  skill: (ws: string, key: string, name: string) =>
    request<SkillDoc>("GET", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/skills/${enc(name)}`),
  harnessSkill: (ws: string, key: string, path: string) =>
    request<SkillDoc>("GET", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/harness-skill${q({ path })}`),
  saveSkill: (ws: string, key: string, name: string, body: SkillSave) =>
    request<SkillDoc>("PUT", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/skills/${enc(name)}`, body),
  deleteSkill: (ws: string, key: string, name: string) =>
    request<void>("DELETE", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/skills/${enc(name)}`),
  adoptSkill: (ws: string, key: string, name: string, body: SkillAdopt) =>
    request<SkillDoc>("POST", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/skills/${enc(name)}/adopt`, body),

  /** `since` (RFC 3339) limits the report to executions that started at or after it. */
  cost: (ws: string, since?: string | null) => request<CostReport>("GET", `/api/workspaces/${enc(ws)}/cost${q({ since })}`),
  ask: (ws: string, body: AskQuestion) => request<AskStarted>("POST", `/api/workspaces/${enc(ws)}/ask`, body),
  pushSubscribe: (body: PushSubscription) => request<void>("POST", "/api/push/subscribe", body),
  listDir: (path?: string) => request<FsListing>("GET", `/api/fs/list${q({ path })}`),
  fsBrowse: (opts: { path?: string; prefix?: string; limit?: number } = {}, init: { signal?: AbortSignal } = {}) =>
    request<FsBrowse>("GET", `/api/fs${q(opts)}`, undefined, init.signal),
  fsMkdir: (path: string) => request<FsBrowse>("POST", "/api/fs/mkdir", { path }),
  harnessSetup: (harness: string, action: HarnessSetupAction) =>
    request<HarnessSetupTerminal>("POST", `/api/harnesses/${enc(harness)}/setup`, { action }),

  environment: () => request<EnvironmentStatus>("GET", "/api/environment"),
  saveProvider: (name: string, edit: ProviderCredentialsEdit) => request<ProviderStatus>("PATCH", `/api/providers/${enc(name)}`, edit),
  validateNewWorkspace: (body: CreateWorkspace) => request<ValidationIssue[]>("POST", "/api/workspaces/validate", body),
  onboarding: () => request<OnboardingState>("GET", "/api/onboarding"),
  completeOnboarding: () => request<OnboardingState>("POST", "/api/onboarding/complete"),
  uiState: (ws: string) => request<WorkspaceUiState>("GET", `/api/workspaces/${enc(ws)}/ui`),
  patchUiState: (ws: string, patch: Partial<WorkspaceUiState>) => request<WorkspaceUiState>("PATCH", `/api/workspaces/${enc(ws)}/ui`, patch),

  projectTree: (ws: string, key: string, opts: { path?: string; depth?: number; hidden?: boolean } = {}) =>
    request<ProjectTree>("GET", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/tree${q({ path: opts.path, depth: opts.depth, hidden: opts.hidden ? "true" : undefined })}`),
  projectFile: (ws: string, key: string, path: string) =>
    request<ProjectFile>("GET", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/file${q({ path })}`),
  createProjectFolder: (ws: string, key: string, path: string) =>
    request<ProjectTree>("POST", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/mkdir`, { path }),
  saveProjectFile: (ws: string, key: string, body: SaveProjectFile) =>
    request<ProjectFile>("PUT", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/file`, body),
  projectFiles: (ws: string, key: string) => request<FileIndex>("GET", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/files`),
  projectDiff: (ws: string, key: string, path: string, base?: string) =>
    request<FileDiff>("GET", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/diff${q({ path, base })}`),
  projectChanges: (ws: string, key: string) => request<ProjectChange[]>("GET", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/changes`),
  codeFile: (ws: string, key: string, path: string) => request<CodeFile>("GET", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/code/file${q({ path })}`),
  codeUsages: (ws: string, key: string, symbol: string, at: { path?: string; line?: number; col?: number; limit?: number } = {}) =>
    request<CodeUsages>("GET", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/code/usages${q({ symbol, ...at })}`),
  codeDeps: (ws: string, key: string, path: string) => request<CodeDeps>("GET", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/code/deps${q({ path })}`),
  codeSymbols: (ws: string, key: string, query: string, limit = 50) =>
    request<CodeSymbols>("GET", `/api/workspaces/${enc(ws)}/projects/${enc(key)}/code/symbols${q({ q: query, limit })}`),

  tree: (ws: string) => request<WorkspaceTree>("GET", `/api/workspaces/${enc(ws)}/tree`),
  search: (ws: string, query: string, limit = 30) => request<SearchResults>("GET", `/api/workspaces/${enc(ws)}/search${q({ q: query, limit })}`),
  workspaceActivity: (ws: string) => request<WorkspaceActivity>("GET", `/api/workspaces/${enc(ws)}/activity`),
};

export type Api = typeof httpApi;
