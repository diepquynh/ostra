// The settings page edits a form model rather than `WorkspaceSettings` itself, so text fields keep what the
// user typed (a trailing newline in a rule list, a half-written model table) while the settings sent for
// validation and saving are derived from it.

import type { Effort } from "../../api/gen/Effort";
import type { Complexity, ModelChoice, PermissionMode, ValidationIssue, WorkspaceSettings } from "../../api/types";
import { COMPLEXITY_AGENTS, ROUTE_KEYS } from "../../content/agents";

export const COMPLEXITIES: Complexity[] = ["low", "medium", "high"];
export const TIER_NAMES = ["fast", "balanced", "advanced", "frontier"];
export const EFFORTS = ["low", "medium", "high", "xhigh", "max"];

export type ModelKind = "unset" | "default" | "tier" | "custom" | "per-executor";
/** One model route as the form holds it. `text` is the tier, the model, or `executor = model, ...`. */
export type ModelField = { kind: ModelKind; text: string };

export type ProjectRow = { key: string; path: string; stack: string };

export type SettingsForm = {
  name: string;
  projects: ProjectRow[];
  /** Route key to `native` or `harness:<name>`. `native` is written as an absent entry. */
  executor: Record<string, string>;
  model: Record<string, ModelField>;
  /** Agent to complexity to executor; `""` follows the agent's route. */
  complexityExecutor: Record<string, Record<Complexity, string>>;
  /** Agent to complexity to model; `unset` follows the agent's route. */
  complexityModel: Record<string, Record<Complexity, ModelField>>;
  /** Agent to effort; `""` keeps the agent definition's default. */
  effort: Record<string, string>;
  /** Agent to complexity to effort; `""` follows the agent's effort. */
  complexityEffort: Record<string, Record<Complexity, string>>;
  instructionsAll: string;
  instructionsAgents: Record<string, string>;
  yolo: boolean;
  mode: PermissionMode;
  allow: string;
  ask: string;
  deny: string;
  push: boolean;
  maxParallel: string;
  budget: string;
};

// ---------------------------------------------------------------------------------------------
// Model routes
// ---------------------------------------------------------------------------------------------

export function formatPerExecutor(table: Record<string, string>): string {
  return Object.entries(table)
    .map(([k, v]) => `${k} = ${v}`)
    .join(", ");
}

const PER_EXECUTOR_HELP = "Write each entry as executor = model, for example native = anthropic:claude-sonnet-5, codex = gpt-5.6-terra.";

export function parsePerExecutor(text: string): { value: Record<string, string> | null; error: string | null } {
  const parts = text
    .split(/[,\n]/)
    .map((p) => p.trim())
    .filter(Boolean);
  if (parts.length === 0) return { value: null, error: `Add at least one entry. ${PER_EXECUTOR_HELP}` };
  const value: Record<string, string> = {};
  for (const p of parts) {
    const m = /^([a-z][a-z0-9_-]*)\s*=\s*(\S.*)$/.exec(p);
    if (!m) return { value: null, error: PER_EXECUTOR_HELP };
    value[m[1]] = m[2].trim();
  }
  return { value, error: null };
}

export function modelToField(m: ModelChoice | undefined): ModelField {
  if (m === undefined) return { kind: "unset", text: "" };
  if (typeof m === "object") return { kind: "per-executor", text: formatPerExecutor(m) };
  if (m === "default") return { kind: "default", text: "" };
  if (TIER_NAMES.includes(m)) return { kind: "tier", text: m };
  return { kind: "custom", text: m };
}

export function fieldToModel(f: ModelField): { value: ModelChoice | undefined; error: string | null } {
  switch (f.kind) {
    case "unset":
      return { value: undefined, error: null };
    case "default":
      return { value: "default", error: null };
    case "tier":
      return { value: f.text, error: null };
    case "custom":
      return f.text.trim() ? { value: f.text.trim(), error: null } : { value: undefined, error: "Enter a model, such as anthropic:claude-sonnet-5 or gpt-5.6-terra." };
    case "per-executor": {
      const { value, error } = parsePerExecutor(f.text);
      return { value: value ?? undefined, error };
    }
  }
}

/** The select value for a model field: the tier name for tiers, else the kind. */
export const modelSelectValue = (f: ModelField) => (f.kind === "tier" ? f.text : f.kind);

/** The field after the user picks `value` in the model select. Switching kinds keeps a sensible text. */
export function pickModel(prev: ModelField, value: string): ModelField {
  if (TIER_NAMES.includes(value)) return { kind: "tier", text: value };
  if (value === "custom") return { kind: "custom", text: prev.kind === "custom" ? prev.text : "" };
  if (value === "per-executor") {
    if (prev.kind === "per-executor") return prev;
    const seed = prev.kind === "tier" || prev.kind === "custom" ? prev.text : "default";
    return { kind: "per-executor", text: `native = ${seed}` };
  }
  return { kind: value as ModelKind, text: "" };
}

// ---------------------------------------------------------------------------------------------
// Form <-> settings
// ---------------------------------------------------------------------------------------------

const lines = (list: string[]) => list.join("\n");
export const unlines = (text: string) =>
  text
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);

/** Route keys the form shows: the known ones, then any the file names that Ostra does not know. */
export function routeKeys(form: Pick<SettingsForm, "executor" | "model">): string[] {
  const known = ROUTE_KEYS.map((r) => r.key as string);
  const extra = [...new Set([...Object.keys(form.executor), ...Object.keys(form.model)])].filter((k) => !known.includes(k)).sort();
  return [...known, ...extra];
}

export function complexityAgents(s: WorkspaceSettings): string[] {
  const keys = new Set<string>(COMPLEXITY_AGENTS);
  Object.keys(s.routing.executor.byPhaseComplexity).forEach((k) => keys.add(k));
  Object.keys(s.routing.model.byPhaseComplexity).forEach((k) => keys.add(k));
  Object.keys(s.routing.effort.byPhaseComplexity).forEach((k) => keys.add(k));
  return [...keys];
}

export function toForm(s: WorkspaceSettings): SettingsForm {
  const keys = new Set<string>([...ROUTE_KEYS.map((r) => r.key as string), ...Object.keys(s.routing.executor.byAgent), ...Object.keys(s.routing.model.byAgent)]);
  const executor: Record<string, string> = {};
  const model: Record<string, ModelField> = {};
  for (const k of keys) {
    executor[k] = s.routing.executor.byAgent[k] ?? "native";
    model[k] = modelToField(s.routing.model.byAgent[k]);
  }
  const complexityExecutor: SettingsForm["complexityExecutor"] = {};
  const complexityModel: SettingsForm["complexityModel"] = {};
  const complexityEffort: SettingsForm["complexityEffort"] = {};
  for (const a of complexityAgents(s)) {
    const ex = s.routing.executor.byPhaseComplexity[a] ?? {};
    const mo = s.routing.model.byPhaseComplexity[a] ?? {};
    const ef = s.routing.effort.byPhaseComplexity[a] ?? {};
    complexityExecutor[a] = { low: ex.low ?? "", medium: ex.medium ?? "", high: ex.high ?? "" };
    complexityModel[a] = { low: modelToField(mo.low), medium: modelToField(mo.medium), high: modelToField(mo.high) };
    complexityEffort[a] = { low: ef.low ?? "", medium: ef.medium ?? "", high: ef.high ?? "" };
  }
  return {
    name: s.name,
    projects: s.projects.map((p) => ({ key: p.key, path: p.path, stack: p.stack ?? "" })),
    executor,
    model,
    complexityExecutor,
    complexityModel,
    effort: { ...s.routing.effort.byAgent },
    complexityEffort,
    instructionsAll: s.instructions.all ?? "",
    instructionsAgents: { ...s.instructions.agents },
    yolo: s.yolo.default,
    mode: s.permissions.mode,
    allow: lines(s.permissions.allow),
    ask: lines(s.permissions.ask),
    deny: lines(s.permissions.deny),
    push: s.notifications.push,
    maxParallel: String(s.limits.max_parallel_executions),
    budget: String(s.limits.session_budget_usd),
  };
}

/**
 * The settings a form describes, on top of `base` so fields the form does not know survive. `issues` are
 * problems the browser can see before the server does (a model table it cannot parse, a budget that is
 * not a number); the settings then keep the base value for that field.
 */
export function fromForm(form: SettingsForm, base: WorkspaceSettings): { settings: WorkspaceSettings; issues: ValidationIssue[] } {
  const s = structuredClone(base);
  const issues: ValidationIssue[] = [];
  const route = (path: string, f: ModelField, keep: ModelChoice | undefined) => {
    const r = fieldToModel(f);
    if (r.error) {
      issues.push({ path, message: r.error });
      return keep;
    }
    return r.value;
  };

  s.name = form.name;
  s.projects = form.projects.map((p) => ({ key: p.key, path: p.path, stack: p.stack || null }));

  s.routing.executor.byAgent = {};
  s.routing.model.byAgent = {};
  for (const [k, ex] of Object.entries(form.executor)) if (ex && ex !== "native") s.routing.executor.byAgent[k] = ex;
  for (const [k, f] of Object.entries(form.model)) {
    const v = route(`routing.model.byAgent.${k}`, f, base.routing.model.byAgent[k]);
    if (v !== undefined) s.routing.model.byAgent[k] = v;
  }

  s.routing.executor.byPhaseComplexity = {};
  s.routing.model.byPhaseComplexity = {};
  for (const [a, byC] of Object.entries(form.complexityExecutor)) {
    const m: Partial<Record<Complexity, string>> = {};
    for (const c of COMPLEXITIES) if (byC[c]) m[c] = byC[c];
    if (Object.keys(m).length) s.routing.executor.byPhaseComplexity[a] = m;
  }
  for (const [a, byC] of Object.entries(form.complexityModel)) {
    const m: Partial<Record<Complexity, ModelChoice>> = {};
    for (const c of COMPLEXITIES) {
      const v = route(`routing.model.byPhaseComplexity.${a}.${c}`, byC[c], base.routing.model.byPhaseComplexity[a]?.[c]);
      if (v !== undefined) m[c] = v;
    }
    if (Object.keys(m).length) s.routing.model.byPhaseComplexity[a] = m;
  }

  s.routing.effort = { byAgent: {}, byPhaseComplexity: {} };
  for (const [k, e] of Object.entries(form.effort)) if (e) s.routing.effort.byAgent[k] = e as Effort;
  for (const [a, byC] of Object.entries(form.complexityEffort)) {
    const m: Partial<Record<Complexity, Effort>> = {};
    for (const c of COMPLEXITIES) if (byC[c]) m[c] = byC[c] as Effort;
    if (Object.keys(m).length) s.routing.effort.byPhaseComplexity[a] = m;
  }

  s.instructions.all = form.instructionsAll.trim() ? form.instructionsAll : null;
  s.instructions.agents = {};
  for (const [k, t] of Object.entries(form.instructionsAgents)) if (t.trim()) s.instructions.agents[k] = t;

  s.yolo.default = form.yolo;
  s.permissions.mode = form.mode;
  s.permissions.allow = unlines(form.allow);
  s.permissions.ask = unlines(form.ask);
  s.permissions.deny = unlines(form.deny);
  s.notifications.push = form.push;

  if (/^\s*\d+\s*$/.test(form.maxParallel)) s.limits.max_parallel_executions = Number(form.maxParallel);
  else issues.push({ path: "limits.max_parallel_executions", message: "Enter a whole number of 1 or more." });
  if (/^\s*\d+(\.\d+)?\s*$/.test(form.budget)) s.limits.session_budget_usd = Number(form.budget);
  else issues.push({ path: "limits.session_budget_usd", message: "Enter a dollar amount such as 25, or 0 for no limit." });

  return { settings: s, issues };
}

/** JSON with object keys sorted, so two settings compare equal whatever order their tables were built in. */
export function stableJson(v: unknown): string {
  return JSON.stringify(v, (_k, x: unknown) =>
    x && typeof x === "object" && !Array.isArray(x)
      ? Object.fromEntries(Object.entries(x as Record<string, unknown>).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))
      : x,
  );
}

// ---------------------------------------------------------------------------------------------
// Issues, tabs and deep links
// ---------------------------------------------------------------------------------------------

export type SettingsTab = "general" | "projects" | "routing" | "permissions" | "instructions" | "notifications";

export const SETTINGS_TABS: { id: SettingsTab; label: string }[] = [
  { id: "general", label: "General" },
  { id: "projects", label: "Projects" },
  { id: "routing", label: "Routing" },
  { id: "permissions", label: "Permissions" },
  { id: "instructions", label: "Instructions" },
  { id: "notifications", label: "Notifications" },
];

/** The tab that holds a settings path or key. */
export function tabOf(path: string): SettingsTab {
  const head = path.split(/[.[]/)[0];
  if (head === "projects" || head === "routing" || head === "permissions" || head === "instructions" || head === "notifications") return head;
  return "general";
}

/** Every field the form renders an issue list for, as dotted settings paths. */
export function fieldIds(form: SettingsForm): string[] {
  const ids = ["name", "yolo.default", "limits.max_parallel_executions", "limits.session_budget_usd", "projects", "instructions.all", "notifications.push"];
  ids.push("permissions.mode", "permissions.allow", "permissions.ask", "permissions.deny");
  form.projects.forEach((_, i) => ids.push(`projects[${i}]`));
  for (const k of routeKeys(form)) ids.push(`routing.executor.byAgent.${k}`, `routing.model.byAgent.${k}`, `routing.effort.byAgent.${k}`, `instructions.agents.${k}`);
  for (const k of Object.keys(form.effort)) ids.push(`routing.effort.byAgent.${k}`);
  for (const a of Object.keys(form.complexityModel))
    for (const c of COMPLEXITIES)
      ids.push(`routing.executor.byPhaseComplexity.${a}.${c}`, `routing.model.byPhaseComplexity.${a}.${c}`, `routing.effort.byPhaseComplexity.${a}.${c}`);
  ids.push("routing.executor.byPhaseComplexity", "routing.model.byPhaseComplexity", "routing.effort.byPhaseComplexity");
  return [...new Set(ids)];
}

const covers = (field: string, path: string) => path === field || path.startsWith(`${field}.`) || path.startsWith(`${field}[`);

/** The field an issue belongs to: the longest field id that is the path or a parent of it. */
export function fieldForIssue(path: string, fields: string[]): string | null {
  let best: string | null = null;
  for (const f of fields) if (covers(f, path) && (!best || f.length > best.length)) best = f;
  return best;
}

export type IssueMap = { byField: Record<string, ValidationIssue[]>; unmatched: ValidationIssue[]; byTab: Record<SettingsTab, number> };

export function mapIssues(issues: ValidationIssue[], fields: string[]): IssueMap {
  const byField: Record<string, ValidationIssue[]> = {};
  const unmatched: ValidationIssue[] = [];
  const byTab: Record<SettingsTab, number> = { general: 0, projects: 0, routing: 0, permissions: 0, instructions: 0, notifications: 0 };
  for (const i of issues) {
    byTab[tabOf(i.path)] += 1;
    const f = fieldForIssue(i.path, fields);
    if (f) (byField[f] ??= []).push(i);
    else unmatched.push(i);
  }
  return { byField, unmatched, byTab };
}

// Settings keys that name a whole table land on the panel that shows it.
const ALIASES: Record<string, string> = {
  routing: "routing.byAgent",
  "routing.executor": "routing.byAgent",
  "routing.executor.byAgent": "routing.byAgent",
  "routing.model": "routing.byAgent",
  "routing.model.byAgent": "routing.byAgent",
  "routing.effort": "routing.byAgent",
  "routing.effort.byAgent": "routing.byAgent",
  "routing.effort.byPhaseComplexity": "routing.byPhaseComplexity",
  "routing.executor.byPhaseComplexity": "routing.byPhaseComplexity",
  "routing.model.byPhaseComplexity": "routing.byPhaseComplexity",
  limits: "limits",
  permissions: "permissions.mode",
};

/** Element ids to try, most specific first, for a settings key or issue path: `setting:<id>`. */
export function anchorCandidates(key: string): string[] {
  const out: string[] = [];
  let k = key;
  while (k) {
    out.push(`setting:${ALIASES[k] ?? k}`);
    const cut = Math.max(k.lastIndexOf("."), k.lastIndexOf("["));
    k = cut > 0 ? k.slice(0, cut) : "";
  }
  return [...new Set(out)];
}

/** The settings key a `setting:<key>` anchor names, or null for any other anchor. */
export const settingKeyOf = (anchor: string | null): string | null => (anchor?.startsWith("setting:") ? anchor.slice(8) || null : null);
