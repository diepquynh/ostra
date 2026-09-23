import type { Api } from "../client";
import type { ExecutionView, GateView, SessionSummary } from "../types";
import * as f from "./fixtures";

const delay = <T>(value: T, ms = 80): Promise<T> =>
  new Promise((resolve) => setTimeout(() => resolve(structuredClone(value)), ms));

let settings = structuredClone(f.settings);
let gates: GateView[] = structuredClone(f.gates);
let lessons = structuredClone(f.lessons);
let yolo = false;

const detailFor = (id: string) => {
  if (id === "s_research") return f.completedDetail;
  return { ...f.sessionDetail, summary: { ...f.sessionSummary, yolo }, gates };
};

/** In-memory stand-in for the server, used with `VITE_MOCK=1`. */
export const mockApi: Api = {
  info: () => delay({ version: "0.1.0-mock", vapid_public_key: "" }),
  exchange: () => delay(undefined),
  workspaces: () => delay(f.workspaces),
  createWorkspace: (body) => delay({ ...f.workspaceDetail, id: "ws_new", root: body.root, settings: { ...settings, name: body.name, projects: [] }, projects: [] }),
  workspace: () => delay({ ...f.workspaceDetail, settings }),
  saveSettings: (_ws, next) => {
    settings = next;
    return delay({ ...f.workspaceDetail, settings });
  },
  validateSettings: (_ws, next) =>
    delay(
      next.name.trim()
        ? Object.keys(next.routing.model.byAgent).includes("plan")
          ? []
          : [{ path: "routing.model.byAgent.plan", message: "`plan` has no model route. Add `plan` under `[routing.model.byAgent]`." }]
        : [{ path: "name", message: "The workspace needs a name." }],
    ),
  importProject: (_ws, body) =>
    delay({
      ...f.workspaceDetail,
      projects: [
        ...f.workspaceDetail.projects,
        { key: body.key, path: body.path, init_status: "not_initialized", ultracode_bootstrap: false, is_git: true, stack: body.stack, profile: null },
      ],
    }),
  removeProject: (_ws, key) => delay({ ...f.workspaceDetail, projects: f.workspaceDetail.projects.filter((p) => p.key !== key) }),
  initProject: () => delay(f.sessions[2]),
  sessions: () => delay(f.sessions),
  createSession: (_ws, body): Promise<SessionSummary> =>
    delay({ ...f.sessionSummary, id: "s_new", request: body.request, status: "running", lane: "research", stage_label: "Classifying", yolo: body.options.yolo }),
  session: (id) => delay(detailFor(id)),
  events: () => delay([]),
  setYolo: (_id, enabled) => {
    yolo = enabled;
    return delay({ ...f.sessionSummary, yolo });
  },
  amend: () => delay(f.sessionSummary),
  answerGate: (id, body) => {
    gates = gates.map((g) => (g.id === id ? { ...g, answer: body.answer, source: "user", answered_at: new Date().toISOString() } : g));
    return delay(gates.find((g) => g.id === id)!);
  },
  overrideDecision: (id, body) => delay({ ...f.decisions.find((d) => d.id === id)!, output: body.output, reason: body.reason, overridden: true }),
  execution: (id) => delay(f.executions.find((e) => e.id === id) ?? f.executions[0]),
  activity: () => delay(f.activity),
  cancelExecution: (id) => delay({ ...(f.executions.find((e) => e.id === id) as ExecutionView), status: "cancelled" }),
  resumeExecution: (id) => delay(f.executions.find((e) => e.id === id)!),
  artifact: (path) => delay({ path, content: path.includes("ledger") ? f.markdownLedger : f.markdownSpec }),
  diff: () =>
    delay([
      {
        path: "src/main/java/shop/order/OrderService.java",
        original: "class OrderService {\n  void cancel(Order order) {\n    order.setStatus(CANCELLED);\n  }\n}\n",
        modified:
          "class OrderService {\n  void cancel(Order order) {\n    order.requireNotShipped();\n    order.setStatus(CANCELLED);\n    events.publish(new OrderCancelled(order.id()));\n  }\n}\n",
      },
    ]),
  lessons: (_ws, _key, query) =>
    delay(query ? lessons.filter((l) => `${l.area} ${l.lesson}`.toLowerCase().includes(query.toLowerCase())) : lessons),
  saveLesson: (_ws, _key, body) => {
    const id = body.id ?? Math.max(0, ...lessons.map((l) => l.id)) + 1;
    const lesson = { id, area: body.area, lesson: body.lesson, source: "user", created_at: new Date().toISOString() };
    lessons = [...lessons.filter((l) => l.id !== id), lesson];
    return delay(lesson);
  },
  deleteLesson: (_ws, _key, id) => {
    lessons = lessons.filter((l) => l.id !== id);
    return delay(undefined);
  },
  cost: () => delay(f.cost),
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
};
