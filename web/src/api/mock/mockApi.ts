import { type Api, HttpError } from "../client";
import type { SearchHit, TreeSession, WorkspaceActivity } from "../nav";
import type {
  DecisionView,
  ExecutionView,
  GateView,
  GitCredentialView,
  PendingGate,
  ProjectSkills,
  SessionDetail,
  SessionSummary,
  SignInSession,
  SkillDoc,
  WorkspaceArtifacts,
  WorkspaceUiState,
} from "../types";
import * as f from "./fixtures";
import { MOCK_BOOK, summaryOf } from "./fixtures.books";
import * as fx from "./fixtures.execution";
import { mockCreateWorkspace, mockValidateCreate, mockValidateImport } from "./fixtures.projects";
import { eventsFor, gateSessions } from "./fixtures.session";
import * as wf from "./fixtures.workspace";
import { mockCodeDeps, mockCodeFile, mockCodeGraph, mockCodeSymbols, mockCodeUsages } from "./mockCode";
import { mockGit } from "./mockGit";
import {
  mockArtifacts,
  mockBrowse,
  mockChanges,
  mockDeleteArtifact,
  mockDiff,
  mockFile,
  mockFileIndex,
  mockMkdir,
  mockMoveArtifact,
  mockSaveFile,
  mockSetArtifactHidden,
  mockTree,
  mockUploadArtifact,
} from "./projectFiles";

let books = [MOCK_BOOK];

const delay = <T>(value: T, ms = 80): Promise<T> =>
  new Promise((resolve) => setTimeout(() => resolve(structuredClone(value)), ms));

/** Run `fn` after the mock delay; a thrown error becomes a 404, like the server's missing-path answers. */
const attempt = <T>(fn: () => T): Promise<T> => {
  try {
    return delay(fn());
  } catch (e) {
    return new Promise((_, reject) => setTimeout(() => reject(new HttpError(404, (e as Error).message)), 80));
  }
};

let settings = structuredClone(f.settings);
const mcpSignedIn = new Set<string>();
const mcpServer = (name: string) => {
  const m = settings.mcp_servers.find((s) => s.name === name);
  if (!m) throw new Error(`This workspace has no MCP server \`${name}\`.`);
  return m;
};
let gitCreds: GitCredentialView[] = [];
let gates: GateView[] = structuredClone(f.gates);
let lessons = structuredClone(wf.lessonsByProject);
let skillDocs: Record<string, Record<string, SkillDoc>> = Object.fromEntries(
  f.workspaceDetail.projects.map((p) => [
    p.key,
    Object.fromEntries(
      (p.profile?.skills ?? []).map((e) => [
        e.name,
        {
          skill: {
            name: e.name,
            description: `Use when a task touches a ${e.component_type ?? e.kind}.`,
            path: e.path,
            origin: "ostra",
            entry: e,
            exists: true,
          },
          content: `---\nname: ${e.name}\ndescription: Use when a task touches a ${e.component_type ?? e.kind}.\n---\n\n# ${e.name}\n`,
        } satisfies SkillDoc,
      ]),
    ),
  ]),
);
const mockSkills = (): ProjectSkills[] =>
  f.workspaceDetail.projects.map((p) => ({
    project: p.key,
    path: p.path,
    blocked: null,
    skills: Object.values(skillDocs[p.key] ?? {}).map((d) => d.skill),
  }));
const skillDoc = (key: string, name: string) => {
  const d = skillDocs[key]?.[name];
  if (!d) throw new HttpError(404, `No skill \`${name}\` in ${key}.`);
  return d;
};
let yolo = false;
let ui: WorkspaceUiState = {
  tabs: [],
  active: null,
  left_tab: null,
  files_project: null,
  sidebar_open: null,
  dock_open: false,
  theme: null,
};

/** `?first-run` in the page URL shows the first-run setup in mock mode. */
const firstRun = () => typeof location !== "undefined" && /[?&]first-run\b/.test(location.search);

// Sessions from fixtures.session.ts, one per gate kind; mutable so answers and stops stick.
let boards: SessionDetail[] = structuredClone(gateSessions);
const boardFor = (id: string) => boards.find((d) => d.summary.id === id);
const updateBoard = (id: string, fn: (d: SessionDetail) => SessionDetail) => {
  boards = boards.map((d) => (d.summary.id === id ? fn(d) : d));
};
const allSessions = (): SessionSummary[] => [
  ...f.sessions.filter((s) => !boardFor(s.id)).map((s) => summaryFor(s.id)),
  ...boards.map((d) => d.summary),
];
const allGates = () => [...gates, ...boards.flatMap((d) => d.gates)];
/** The latest open gate whose payload names the execution, as the server's fold finds it. */
function pendingGate(id: string): PendingGate | null {
  const open = allGates().filter((g) => g.answer === null && "execution" in g.payload && g.payload.execution === id);
  const g = open.sort((a, b) => a.opened_at.localeCompare(b.opened_at)).pop();
  return g ? { id: g.id, kind: g.payload.kind, title: g.title } : null;
}
const execView = (id: string): ExecutionView => {
  const e = f.executions.some((x) => x.id === id)
    ? fx.executionView(id)
    : (boards.flatMap((d) => d.executions).find((x) => x.id === id) ?? fx.executionView(id));
  return { ...e, pending_gate: pendingGate(id) };
};

const summaryFor = (id: string): SessionSummary => {
  const b = boardFor(id);
  if (b) return b.summary;
  const s = f.sessions.find((x) => x.id === id) ?? f.sessionSummary;
  return id === f.SESSION ? { ...s, yolo } : s;
};

const detailFor = (id: string): SessionDetail => {
  const b = boardFor(id);
  if (b) return b;
  if (id === f.SESSION) return { ...f.sessionDetail, summary: summaryFor(id), gates };
  const executions = f.sessionExecutions(id);
  const base =
    id === "s_research"
      ? f.completedDetail
      : { ...f.sessionDetail, completion: null, stages: [], phases: [], decisions: [], artifacts: [] };
  return { ...base, summary: summaryFor(id), gates: [], executions, execution_groups: f.groupsFor(executions) };
};

function treeSession(id: string): TreeSession {
  const d = detailFor(id);
  const byId = new Map(d.executions.map((x) => [x.id, x]));
  const s = d.summary;
  return {
    id: s.id,
    title: s.title,
    request: s.request,
    kind: s.kind,
    status: s.status,
    open_gates: id === f.SESSION ? gates.filter((g) => g.answer === null).length : s.open_gates,
    cost_usd: s.cost_usd,
    updated_at: s.updated_at,
    artifacts: d.artifacts,
    groups: d.execution_groups.map((g) => ({
      group: g.group,
      agent: g.agent,
      project: g.project,
      status: g.status,
      cost_usd: g.cost_usd,
      runs: g.executions.map((xid) => {
        const x = byId.get(xid)!;
        return { id: x.id, run_label: x.run_label, status: x.status, stream: x.stream, summary: x.summary };
      }),
    })),
  };
}

const tree = () =>
  allSessions()
    .map((s) => treeSession(s.id))
    .sort((a, b) => b.updated_at.localeCompare(a.updated_at));

function activity(): WorkspaceActivity {
  const t = tree();
  return {
    running: t.flatMap((s) =>
      s.groups.flatMap((g) =>
        g.runs
          .filter((r) => r.status === "running")
          .map((r) => ({
            id: r.id,
            session: s.id,
            agent: g.agent,
            project: g.project,
            run_label: r.run_label,
            stream: r.stream,
            summary: r.summary,
          })),
      ),
    ),
    open_gates: allGates()
      .filter((g) => g.answer === null)
      .map((g) => ({
        id: g.id,
        session: g.session,
        session_title: summaryFor(g.session).title,
        title: g.title,
        kind: g.payload.kind,
        opened_at: g.opened_at,
      })),
    spend_today_usd: 1.84,
    spend_week_usd: 4.18,
    today_since: "2026-09-22T00:00:00Z",
    week_since: "2026-09-16T00:00:00Z",
  };
}

function search(query: string, limit: number): SearchHit[] {
  const q = query.toLowerCase();
  const hits: SearchHit[] = [];
  const add = (kind: SearchHit["kind"], id: string, label: string, hint: string | null, text = label) => {
    const i = text.toLowerCase().indexOf(q);
    if (i >= 0) hits.push({ kind, id, label, hint, score: 1 / (1 + i) });
  };
  for (const s of tree()) {
    add("session", `session:${s.id}`, s.title ?? s.request, s.status, `${s.title ?? ""} ${s.request}`);
    for (const g of s.groups)
      for (const r of g.runs) add("execution", `exec:${r.id}`, `${g.agent} · ${r.run_label} in ${g.project}`, r.status);
    for (const a of s.artifacts)
      add("artifact", `artifact:${a.path}`, a.label, a.path.split("/").pop() ?? null, `${a.label} ${a.path}`);
  }
  for (const p of f.workspaceDetail.projects) add("project", `project:${p.key}`, p.key, p.path);
  for (const key of ["backend", "web"])
    for (const p of mockFileIndex(key).paths) add("file", `file:${key}:${p}`, p, key);
  for (const [key, list] of Object.entries(lessons))
    for (const l of list)
      add("lesson", `lesson:${key}:${l.id}`, l.lesson, `${key} · ${l.area}`, `${l.area} ${l.lesson}`);
  for (const [k, label] of wf.SETTING_KEYS) add("setting", `setting:${k}`, k, label, `${k} ${label}`);
  return hits.sort((a, b) => b.score - a.score).slice(0, limit);
}

let mockSignIns: SignInSession[] = [
  {
    id: "si_3f2a9c1d7e5b6a40",
    created: "2026-09-20T08:00:00Z",
    last_seen: "2026-09-25T09:30:00Z",
    expires: "2026-10-20T08:00:00Z",
    user_agent: "Mozilla/5.0 (X11; Linux x86_64) Chrome/140.0",
    ip: "127.0.0.1",
    current: true,
  },
  {
    id: "si_91be07c4d2a38f15",
    created: "2026-09-22T19:12:00Z",
    last_seen: "2026-09-24T21:40:00Z",
    expires: "2026-10-22T19:12:00Z",
    user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_2) Safari/19.0",
    ip: "192.168.0.2",
    current: false,
  },
];

/** In-memory stand-in for the server, used with `VITE_MOCK=1`. */
const artifactList = (): WorkspaceArtifacts => ({
  dir: "/home/me/code/shop/.ostra/artifacts",
  artifacts: mockArtifacts(),
  delete_blocked: null,
});

export const mockApi: Api = {
  info: () => delay({ version: "0.1.0-mock", vapid_public_key: "" }),
  exchange: () => delay(undefined),
  signIns: () => delay(mockSignIns),
  revokeSignIn: (id) => {
    mockSignIns = mockSignIns.filter((s) => s.id !== id);
    return delay(undefined);
  },
  revokeOtherSignIns: () => {
    const revoked = mockSignIns.filter((s) => !s.current).length;
    mockSignIns = mockSignIns.filter((s) => s.current);
    return delay({ revoked });
  },
  signOut: () => delay(undefined),
  workspaces: () => delay(firstRun() ? [] : f.workspaces),
  createWorkspace: (body) => {
    const issues = mockValidateCreate(body, firstRun() ? [] : f.workspaces.map((w) => w.root));
    if (issues.length)
      return new Promise((_, reject) =>
        setTimeout(() => reject(new HttpError(422, "The workspace settings have problems.", issues)), 400),
      );
    return delay(mockCreateWorkspace(body, { ...f.workspaceDetail, settings }), 1500);
  },
  workspace: () => delay({ ...f.workspaceDetail, settings }),
  deleteWorkspace: () => delay(undefined),
  saveSettings: (_ws, next) => {
    const issues = wf.validate(next);
    if (issues.length)
      return new Promise((_, reject) =>
        setTimeout(() => reject(new HttpError(422, "The settings have problems.", issues)), 80),
      );
    settings = next;
    return delay({ ...f.workspaceDetail, settings });
  },
  validateSettings: (_ws, next) => delay(wf.validate(next)),
  approveCommands: () => delay({ ...f.workspaceDetail, settings }),
  fixSettings: () => delay({ ...f.workspaceDetail, settings }),
  mcpStatus: () =>
    delay(
      settings.mcp_servers.map((m) => wf.mockMcpStatus(m, mcpSignedIn)),
      600,
    ),
  mcpRefresh: (_ws, name) => attempt(() => wf.mockMcpStatus(mcpServer(name), mcpSignedIn)),
  mcpLogin: (_ws, name) => {
    mcpServer(name);
    // The mock authorization server signs in at once.
    setTimeout(() => mcpSignedIn.add(name), 1500);
    return delay({ authorization_url: "https://auth.example.test/authorize" });
  },
  mcpLogout: (_ws, name) => {
    mcpSignedIn.delete(name);
    return attempt(() => wf.mockMcpStatus(mcpServer(name), mcpSignedIn));
  },
  importProject: (_ws, body) => {
    const issues = mockValidateImport(body, f.workspaceDetail.projects);
    if (issues.length) {
      const error =
        issues.length === 1
          ? issues[0].message
          : `Fix these problems and import the project again: ${issues.map((i) => i.message).join(" ")}`;
      return new Promise((_, reject) => setTimeout(() => reject(new HttpError(422, error, issues)), 250));
    }
    return delay({
      ...f.workspaceDetail,
      projects: [
        ...f.workspaceDetail.projects,
        {
          key: body.key.trim(),
          path: body.path,
          init_status: "not_initialized",
          ultracode_bootstrap: false,
          is_git: true,
          git_branch: "main",
          stack: body.stack || null,
          profile: null,
        },
      ],
    });
  },
  cloneProject: (_ws, body) => {
    const key = body.key.trim();
    return delay(
      {
        ...f.workspaceDetail,
        projects: [
          ...f.workspaceDetail.projects,
          {
            key,
            path: body.path || `${f.workspaceDetail.root}/${key}`,
            init_status: "not_initialized",
            ultracode_bootstrap: false,
            is_git: true,
            git_branch: body.branch || "main",
            stack: body.stack || null,
            profile: null,
          },
        ],
      },
      1500,
    );
  },
  pullProject: () =>
    delay({ branch: "main", updated: false, before: "1a2b3c4", after: "1a2b3c4", output: "Already up to date." }),
  gitStatus: (_ws, key) => delay(mockGit.status(key)),
  gitBranches: (_ws, key) => delay(mockGit.branches(key)),
  gitStage: (_ws, key, paths) => delay(mockGit.stage(key, paths, true)),
  gitUnstage: (_ws, key, paths) => delay(mockGit.stage(key, paths, false)),
  gitCommit: (_ws, key, message) => attempt(() => mockGit.commit(key, message)),
  gitFetch: (_ws, key) => delay(mockGit.op(key, "Fetched origin."), 400),
  gitPush: (_ws, key) => delay(mockGit.push(key), 600),
  gitCheckout: (_ws, key, body) => attempt(() => mockGit.checkout(key, body)),
  gitCredentials: () => delay(gitCreds),
  createGitCredential: (edit) => {
    const host = (edit.host ?? "").trim();
    gitCreds = [
      ...gitCreds,
      {
        id: `gc_${gitCreds.length + 1}`,
        label: edit.label || host,
        host,
        kind: edit.kind === "ssh" ? "ssh" : "https",
        username: edit.username || null,
        has_secret: true,
      },
    ];
    return delay(gitCreds);
  },
  updateGitCredential: (id, edit) => {
    gitCreds = gitCreds.map((c) =>
      c.id === id
        ? { ...c, label: edit.label ?? c.label, host: edit.host ?? c.host, username: edit.username ?? c.username }
        : c,
    );
    return delay(gitCreds);
  },
  deleteGitCredential: (id) => {
    gitCreds = gitCreds.filter((c) => c.id !== id);
    return delay(gitCreds);
  },
  removeProject: (_ws, key) =>
    delay({ ...f.workspaceDetail, projects: f.workspaceDetail.projects.filter((p) => p.key !== key) }),
  initProject: () => delay(f.sessions[2]),
  sessions: () => delay(allSessions()),
  createSession: (_ws, body): Promise<SessionSummary> =>
    delay({
      ...f.sessionSummary,
      id: "s_new",
      request: body.request,
      status: "running",
      lane: "research",
      stage_label: "Classifying",
      yolo: body.options.yolo,
      title: null,
    }),
  session: (id) => delay(detailFor(id)),
  events: (id) => delay(eventsFor(detailFor(id))),
  setYolo: (id, enabled) => {
    if (boardFor(id)) {
      updateBoard(id, (d) => ({ ...d, summary: { ...d.summary, yolo: enabled } }));
      return delay(summaryFor(id));
    }
    yolo = enabled;
    return delay({ ...f.sessionSummary, yolo });
  },
  amend: (id, body) => {
    updateBoard(id, (d) => ({
      ...d,
      additions: [
        ...d.additions,
        { text: body.text, files: body.files, uploads: [], delivery: body.delivery, at: new Date().toISOString() },
      ],
    }));
    return delay(summaryFor(id));
  },
  uploadFile: (_ws, file) =>
    delay({ id: Math.random().toString(16).slice(2).padEnd(32, "0"), name: file.name, size: file.size }),
  pauseSession: (id) => {
    updateBoard(id, (d) => ({ ...d, summary: { ...d.summary, status: "paused" } }));
    return delay(summaryFor(id));
  },
  resumeSession: (id) => {
    updateBoard(id, (d) => ({ ...d, summary: { ...d.summary, status: "running" } }));
    return delay(summaryFor(id));
  },
  stopSession: (id) => {
    updateBoard(id, (d) => ({
      ...d,
      summary: { ...d.summary, status: "failed", lane: "done", stage_label: "Stopped", open_gates: 0 },
    }));
    return delay(summaryFor(id));
  },
  answerGate: (id, body) => {
    const answer = (g: GateView): GateView =>
      g.id === id ? { ...g, answer: body.answer, source: "user", answered_at: new Date().toISOString() } : g;
    gates = gates.map(answer);
    boards = boards.map((d) => {
      if (!d.gates.some((g) => g.id === id)) return d;
      const next = d.gates.map(answer);
      const open = next.filter((g) => g.answer === null).length;
      return { ...d, gates: next, summary: { ...d.summary, open_gates: open, status: open ? "waiting" : "running" } };
    });
    return delay(allGates().find((g) => g.id === id)!);
  },
  overrideDecision: (id, body) => {
    const next = (d: DecisionView): DecisionView =>
      d.id === id ? { ...d, output: body.output, reason: body.reason, overridden: true } : d;
    boards = boards.map((b) => ({ ...b, decisions: b.decisions.map(next) }));
    return delay(next([...f.decisions, ...boards.flatMap((b) => b.decisions)].find((d) => d.id === id)!));
  },
  execution: (id) => delay(execView(id)),
  activity: (id, after) => delay(fx.activityFor(id).filter((a) => a.seq > (after ?? 0))),
  cancelExecution: (id) => delay({ ...execView(id), status: "cancelled" }),
  skipExecution: (id) => delay({ ...execView(id), status: "interrupted", can_skip: false }),
  resumeExecution: (id) => delay(fx.resume(id)),
  inspectExecution: (id) => delay(fx.resume(id)),
  artifact: (path) => delay(fx.artifactFor(path)),
  diff: () => delay(fx.ledgerDiff),
  lessons: (_ws, key, query) => {
    const list = lessons[key] ?? [];
    return delay(
      query ? list.filter((l) => `${l.area} ${l.lesson}`.toLowerCase().includes(query.toLowerCase())) : list,
    );
  },
  saveLesson: (_ws, key, body) => {
    const id =
      body.id ??
      Math.max(
        0,
        ...Object.values(lessons)
          .flat()
          .map((l) => l.id),
      ) + 1;
    const list = lessons[key] ?? [];
    const old = list.find((l) => l.id === id);
    const lesson = {
      id,
      area: body.area,
      lesson: body.lesson,
      source: old?.source ?? "user",
      created_at: old?.created_at ?? new Date().toISOString(),
    };
    lessons = { ...lessons, [key]: old ? list.map((l) => (l.id === id ? lesson : l)) : [...list, lesson] };
    return delay(lesson);
  },
  deleteLesson: (_ws, key, id) => {
    lessons = { ...lessons, [key]: (lessons[key] ?? []).filter((l) => l.id !== id) };
    return delay(undefined);
  },
  skills: () => delay(mockSkills()),
  books: () => delay(books.map(summaryOf)),
  book: (_ws, id) =>
    attempt(() => {
      const b = books.find((x) => x.id === id);
      if (!b) throw new Error("No such book in this workspace.");
      return b;
    }),
  deleteBook: (_ws, id) => {
    books = books.filter((b) => b.id !== id);
    return delay(books.map(summaryOf));
  },
  skill: async (_ws, key, name) => delay(skillDoc(key, name)),
  harnessSkill: async (_ws, key, path) => delay(skillDoc(key, path)),
  saveSkill: (_ws, key, name, body) => {
    const old = skillDocs[key]?.[name];
    const entry = {
      name,
      kind: body.kind,
      path: `.ostra/skills/${name}/SKILL.md`,
      component_type: body.component_type,
      source: old?.skill.entry?.source ?? "user",
    };
    const doc: SkillDoc = {
      skill: {
        name,
        description: old?.skill.description ?? null,
        path: entry.path,
        origin: "ostra",
        entry,
        exists: true,
      },
      content: body.content,
    };
    skillDocs = { ...skillDocs, [key]: { ...skillDocs[key], [name]: doc } };
    return delay(doc);
  },
  saveProjectCommands: (_ws, key, commands) => {
    const p = f.workspaceDetail.projects.find((x) => x.key === key);
    if (!p?.profile)
      return Promise.reject(new HttpError(409, `Initialize ${key} first: its commands live in .ostra/project.toml.`));
    const bad = Object.entries(commands).filter(([, v]) => v && /[\r\n]/.test(v));
    if (bad.length)
      return Promise.reject(
        new HttpError(
          422,
          "Fix the commands and save again.",
          bad.map(([k]) => ({
            path: `commands.${k}`,
            message: "Write the command on one line; chain steps with `&&`.",
          })),
        ),
      );
    const clean = Object.fromEntries(
      Object.entries(commands).map(([k, v]) => [k, v?.trim() ? v.trim() : null]),
    ) as typeof commands;
    p.profile.commands = clean;
    return delay(clean);
  },
  deleteSkill: (_ws, key, name) => {
    const rest = { ...skillDocs[key] };
    delete rest[name];
    skillDocs = { ...skillDocs, [key]: rest };
    return delay(undefined);
  },
  adoptSkill: async (_ws, key, name) => delay(skillDoc(key, name)),
  cost: (_ws, since) => delay(since ? wf.costSince(since) : wf.cost),
  ask: () => delay({ execution: "x_rev2" }),
  pushSubscribe: () => delay(undefined),
  listDir: (path) =>
    delay({
      path: path || "/home/me",
      parent: path && path !== "/" ? path.split("/").slice(0, -1).join("/") || "/" : null,
      entries: [
        { name: "code", is_dir: true, is_git: false, is_ostra_project: false },
        { name: "shop-backend", is_dir: true, is_git: true, is_ostra_project: true },
        { name: "notes.txt", is_dir: false, is_git: false, is_ostra_project: false },
      ],
    }),
  fsBrowse: (opts = {}) => delay(mockBrowse(opts.path, opts.prefix, opts.limit)),
  fsMkdir: (path) => delay(mockBrowse(path)),
  harnessSetup: (harness, action) => delay({ terminal: `setup_${harness}_${action}`, command: `${harness} ${action}` }),

  environment: () =>
    delay({
      providers: f.workspaceDetail.providers,
      harnesses: f.workspaceDetail.harnesses,
      stacks: f.workspaceDetail.stacks,
      sandbox: { ...f.workspaceDetail.sandbox, mode: "auto" },
      shell: { available: true, path: "/usr/bin/bash" },
    }),
  saveProvider: (name, edit) => {
    const p = f.workspaceDetail.providers.find((x) => x.name === name);
    if (!p) return Promise.reject(new HttpError(404, `No provider ${name} in the global config.`, []));
    const set = (v: string | undefined, cur: boolean) => (v === undefined ? cur : v.trim() !== "");
    if (edit.base_url !== undefined) p.saved.base_url = edit.base_url.trim() || null;
    p.saved.has_api_key = set(edit.api_key, p.saved.has_api_key);
    p.saved.has_auth_token = set(edit.auth_token, p.saved.has_auth_token);
    if (!p.source.startsWith("env:")) {
      p.has_key = p.saved.has_api_key || p.saved.has_auth_token;
      p.source = p.has_key ? "saved" : "none";
    }
    if (!p.base_url_source.startsWith("env:")) {
      p.base_url = p.saved.base_url;
      p.base_url_source = p.saved.base_url ? "saved" : "default";
    }
    return delay({ ...p, saved: { ...p.saved } });
  },
  validateNewWorkspace: (body) =>
    delay(mockValidateCreate(body, firstRun() ? [] : f.workspaces.map((w) => w.root)), 250),
  onboarding: () =>
    delay(
      firstRun()
        ? { onboarded_at: null, workspaces: 0 }
        : { onboarded_at: "2026-09-01T09:00:00Z", workspaces: f.workspaces.length },
    ),
  completeOnboarding: () => delay({ onboarded_at: new Date().toISOString(), workspaces: f.workspaces.length }),
  uiState: () => delay(ui),
  patchUiState: (_ws, patch) => {
    ui = { ...ui, ...patch };
    return delay(ui);
  },

  artifacts: () => delay(artifactList()),
  uploadArtifact: async (_ws, path, file) => delay(mockUploadArtifact(path, await file.text())),
  setArtifactHidden: (_ws, path, hidden) => {
    try {
      mockSetArtifactHidden(path, hidden);
    } catch (e) {
      const err = e as Error & { status?: number; issues?: { path: string; message: string }[] };
      return Promise.reject(new HttpError(err.status ?? 409, err.message, err.issues ?? []));
    }
    return delay(artifactList());
  },
  moveArtifact: (_ws, from, to) => {
    mockMoveArtifact(from, to);
    return delay(artifactList());
  },
  deleteArtifact: (_ws, path) => {
    mockDeleteArtifact(path);
    return delay(artifactList());
  },
  artifactDownloadUrl: () => "#",

  projectTree: (_ws, key, opts = {}) => attempt(() => mockTree(key, opts.path, opts.hidden)),
  projectFile: (_ws, key, path) => attempt(() => mockFile(key, path)),
  createProjectFolder: (_ws, key, path) => {
    try {
      return delay(mockMkdir(key, path));
    } catch (e) {
      const err = e as Error & { status?: number; issues?: { path: string; message: string }[] };
      return new Promise((_, reject) =>
        setTimeout(() => reject(new HttpError(err.status ?? 400, err.message, err.issues ?? [])), 80),
      );
    }
  },
  saveProjectFile: (_ws, key, body) => {
    try {
      return delay(mockSaveFile(key, body.path, body.content, body.base_hash));
    } catch (e) {
      const err = e as Error & { status?: number; issues?: { path: string; message: string }[] };
      return new Promise((_, reject) =>
        setTimeout(() => reject(new HttpError(err.status ?? 404, err.message, err.issues ?? [])), 80),
      );
    }
  },
  projectFiles: (_ws, key) => delay(mockFileIndex(key)),
  projectDiff: (_ws, key, path) => attempt(() => mockDiff(key, path)),
  codeFile: (_ws, key, path) => attempt(() => mockCodeFile(key, path)),
  codeUsages: (_ws, key, symbol, at = {}) => attempt(() => mockCodeUsages(key, symbol, at.path)),
  codeDeps: (_ws, key, path) => attempt(() => mockCodeDeps(key, path)),
  codeExternal: () =>
    attempt(() => {
      throw new Error("The mock project has no language server, so it has no dependency files.");
    }),
  codeSymbols: (_ws, key, query, limit = 50) => attempt(() => mockCodeSymbols(key, query, limit)),
  codeGraph: (_ws, key, at = {}) => attempt(() => mockCodeGraph(key, at)),
  codeReindex: (_ws, key) =>
    delay({ indexed_files: mockCodeGraph(key, {}).indexed_files, millis: 12, truncated: false }),
  projectChanges: (_ws, key) => delay(mockChanges(key)),

  tree: () => delay({ sessions: tree() }),
  search: (_ws, query, limit = 30) => delay({ items: search(query, limit) }),
  workspaceActivity: () => delay(activity()),
};
