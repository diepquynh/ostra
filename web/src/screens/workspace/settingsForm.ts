// The settings page edits a form model rather than `WorkspaceSettings` itself, so text fields keep what the
// user typed (a trailing newline in a rule list, a half-written model table) while the settings sent for
// validation and saving are derived from it.

import type { Effort } from "../../api/gen/Effort";
import type { LoopbackAccess } from "../../api/gen/LoopbackAccess";
import type {
  Complexity,
  McpServerConfig,
  ModelChoice,
  PermissionMode,
  SandboxMode,
  SandboxNetwork,
  ValidationIssue,
  WorkspaceSettings,
} from "../../api/types";
import { COMPLEXITY_AGENTS, ROUTE_KEYS } from "../../content/agents";

export const COMPLEXITIES: Complexity[] = ["low", "medium", "high"];
export const TIER_NAMES = ["fast", "balanced", "advanced", "frontier"];
export const EFFORTS = ["low", "medium", "high", "xhigh", "max"];

export type ModelKind = "unset" | "default" | "tier" | "custom" | "per-executor";
/** One model route as the form holds it. `text` is the tier, the model, or `executor = model, ...`. */
export type ModelField = { kind: ModelKind; text: string };

export type ProjectRow = { key: string; path: string; stack: string };

/** One MCP server as the form holds it. Only the fields of the chosen transport are saved. */
export type McpRow = {
  /** Stable React key for the row. */
  id: string;
  name: string;
  enabled: boolean;
  transport: "http" | "stdio";
  url: string;
  /** Program and arguments as a shell line, for example `npx -y @upstash/context7-mcp`. */
  command: string;
  /** `KEY=value` per line. */
  env: string;
  /** `Name: value` per line. */
  headers: string;
  disabledTools: string[];
  /** Empty means every agent. */
  agents: string[];
  timeout: string;
  clientId: string;
  clientSecretEnv: string;
  /** Space-separated. */
  scopes: string;
};

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
  /** `""` follows the global `[sandbox] mode`. */
  sandbox: SandboxMode | "";
  /** `""` follows the global `[sandbox] network`. */
  network: SandboxNetwork | "";
  /** The workspace's own allowed hosts, one per line. */
  allowedHosts: string;
  /** The workspace's own decoy paths, one per line. */
  decoys: string;
  /** Which loopback ports commands on macOS may connect to. */
  loopback: LoopbackAccess;
  /** Blocked loopback ports, separated by spaces, commas, or lines. */
  blockedPorts: string;
  allow: string;
  ask: string;
  deny: string;
  push: boolean;
  maxParallel: string;
  budget: string;
  mcp: McpRow[];
};

// ---------------------------------------------------------------------------------------------
// Model routes
// ---------------------------------------------------------------------------------------------

export function formatPerExecutor(table: Record<string, string>): string {
  return Object.entries(table)
    .map(([k, v]) => `${k} = ${v}`)
    .join(", ");
}

const PER_EXECUTOR_HELP =
  "Write each entry as executor = model, for example native = anthropic:claude-sonnet-5-5, codex = gpt-5.6-terra.";

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
      return f.text.trim()
        ? { value: f.text.trim(), error: null }
        : { value: undefined, error: "Enter a model, such as anthropic:claude-sonnet-5-5 or gpt-5.6-terra." };
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
// MCP servers
// ---------------------------------------------------------------------------------------------

/** Split a shell line into words: spaces separate, quotes group, a backslash escapes. `null` for an open quote. */
export function splitCommand(line: string): string[] | null {
  const words: string[] = [];
  let word = "";
  let started = false;
  let quote: "'" | '"' | null = null;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (quote) {
      if (c === quote) quote = null;
      else if (c === "\\" && quote === '"' && i + 1 < line.length) word += line[++i];
      else word += c;
    } else if (c === "'" || c === '"') {
      quote = c;
      started = true;
    } else if (c === "\\" && i + 1 < line.length) {
      word += line[++i];
      started = true;
    } else if (/\s/.test(c)) {
      if (started) words.push(word);
      word = "";
      started = false;
    } else {
      word += c;
      started = true;
    }
  }
  if (quote) return null;
  if (started) words.push(word);
  return words;
}

/** The shell line for a word list, quoting words that need it. */
export function joinCommand(words: string[]): string {
  return words.map((w) => (w && /^[\w@%+=:,./-]+$/.test(w) ? w : `'${w.replaceAll("'", `'\\''`)}'`)).join(" ");
}

let mcpRowIds = 0;

export function mcpRow(c?: McpServerConfig): McpRow {
  mcpRowIds += 1;
  return {
    id: `mcp-${mcpRowIds}`,
    name: c?.name ?? "",
    enabled: c?.enabled ?? true,
    transport: c && (c.command?.length ?? 0) > 0 ? "stdio" : "http",
    url: c?.url ?? "",
    command: joinCommand(c?.command ?? []),
    env: Object.entries(c?.env ?? {})
      .map(([k, v]) => `${k}=${v}`)
      .join("\n"),
    headers: Object.entries(c?.headers ?? {})
      .map(([k, v]) => `${k}: ${v}`)
      .join("\n"),
    disabledTools: [...(c?.disabled_tools ?? [])],
    agents: [...(c?.agents ?? [])],
    timeout: String(c?.timeout_secs ?? 120),
    clientId: c?.oauth?.client_id ?? "",
    clientSecretEnv: c?.oauth?.client_secret_env ?? "",
    scopes: (c?.oauth?.scopes ?? []).join(" "),
  };
}

/** Parse `key<sep>value` lines. Returns the table, or the text of the first bad line. */
function pairs(text: string, sep: "=" | ":"): { value: Record<string, string>; bad: string | null } {
  const value: Record<string, string> = {};
  for (const line of unlines(text)) {
    const at = line.indexOf(sep);
    const k = at > 0 ? line.slice(0, at).trim() : "";
    if (!k) return { value, bad: line };
    value[k] = line.slice(at + 1).trim();
  }
  return { value, bad: null };
}

export function mcpFromRow(row: McpRow, at: string, issues: ValidationIssue[]): McpServerConfig {
  const c: McpServerConfig = { name: row.name.trim(), enabled: row.enabled, timeout_secs: 120 };
  if (row.transport === "http") {
    c.url = row.url.trim();
    const h = pairs(row.headers, ":");
    if (h.bad) issues.push({ path: `${at}.headers`, message: `Write each header as Name: value, not \`${h.bad}\`.` });
    if (Object.keys(h.value).length) c.headers = h.value;
    const scopes = row.scopes.split(/\s+/).filter(Boolean);
    if (row.clientId.trim() || row.clientSecretEnv.trim() || scopes.length) {
      c.oauth = { scopes };
      if (row.clientId.trim()) c.oauth.client_id = row.clientId.trim();
      if (row.clientSecretEnv.trim()) c.oauth.client_secret_env = row.clientSecretEnv.trim();
    }
  } else {
    const words = splitCommand(row.command);
    if (words === null) issues.push({ path: `${at}.command`, message: "Close the quote in the command." });
    else c.command = words;
    const e = pairs(row.env, "=");
    if (e.bad) issues.push({ path: `${at}.env`, message: `Write each variable as KEY=value, not \`${e.bad}\`.` });
    if (Object.keys(e.value).length) c.env = e.value;
  }
  if (row.disabledTools.length) c.disabled_tools = [...row.disabledTools];
  if (row.agents.length) c.agents = [...row.agents];
  if (/^\s*\d+\s*$/.test(row.timeout)) c.timeout_secs = Number(row.timeout);
  else issues.push({ path: `${at}.timeout_secs`, message: "Enter a whole number of seconds, from 1 to 600." });
  return c;
}

// ---------------------------------------------------------------------------------------------
// Form <-> settings
// ---------------------------------------------------------------------------------------------

const lines = (list: string[]) => list.join("\n");
/** Port numbers in `text`; a word that is not a port is dropped, and 0 stays for the server to name. */
export const ports = (text: string) =>
  text
    .split(/[\s,]+/)
    .filter((w) => /^\d{1,5}$/.test(w) && Number(w) <= 65535)
    .map(Number);

export const unlines = (text: string) =>
  text
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);

/** Route keys the form shows: the known ones, then any the file names that Ostra does not know. */
export function routeKeys(form: Pick<SettingsForm, "executor" | "model">): string[] {
  const known = ROUTE_KEYS.map((r) => r.key as string);
  const extra = [...new Set([...Object.keys(form.executor), ...Object.keys(form.model)])]
    .filter((k) => !known.includes(k))
    .sort();
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
  const keys = new Set<string>([
    ...ROUTE_KEYS.map((r) => r.key as string),
    ...Object.keys(s.routing.executor.byAgent),
    ...Object.keys(s.routing.model.byAgent),
  ]);
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
    sandbox: s.sandbox_mode ?? "",
    network: s.sandbox_network ?? "",
    allowedHosts: lines(s.sandbox_allowed_hosts),
    decoys: lines(s.sandbox_decoys),
    loopback: s.sandbox_loopback,
    blockedPorts: s.sandbox_blocked_ports.join(", "),
    allow: lines(s.permissions.allow),
    ask: lines(s.permissions.ask),
    deny: lines(s.permissions.deny),
    push: s.notifications.push,
    maxParallel: String(s.limits.max_parallel_executions),
    budget: String(s.limits.session_budget_usd),
    mcp: s.mcp_servers.map((c) => mcpRow(c)),
  };
}

/**
 * The settings a form describes, on top of `base` so fields the form does not know survive. `issues` are
 * problems the browser can see before the server does (a model table it cannot parse, a budget that is
 * not a number); the settings then keep the base value for that field.
 */
export function fromForm(
  form: SettingsForm,
  base: WorkspaceSettings,
): { settings: WorkspaceSettings; issues: ValidationIssue[] } {
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
  // The form has no fields for code providers or language servers, so each row keeps what its project had (by key, or by path after a rename).
  s.projects = form.projects.map((p) => {
    const was = base.projects.find((b) => b.key === p.key) ?? base.projects.find((b) => b.path === p.path);
    return {
      key: p.key,
      path: p.path,
      stack: p.stack.trim() || null,
      ...(was?.code_provider ? { code_provider: was.code_provider } : {}),
      ...(was?.language_servers?.length ? { language_servers: was.language_servers } : {}),
    };
  });

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
      const v = route(
        `routing.model.byPhaseComplexity.${a}.${c}`,
        byC[c],
        base.routing.model.byPhaseComplexity[a]?.[c],
      );
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
  s.sandbox_mode = form.sandbox || null;
  s.sandbox_network = form.network || null;
  s.sandbox_allowed_hosts = unlines(form.allowedHosts);
  s.sandbox_decoys = unlines(form.decoys);
  s.sandbox_loopback = form.loopback;
  s.sandbox_blocked_ports = ports(form.blockedPorts);
  s.permissions.allow = unlines(form.allow);
  s.permissions.ask = unlines(form.ask);
  s.permissions.deny = unlines(form.deny);
  s.notifications.push = form.push;

  if (/^\s*\d+\s*$/.test(form.maxParallel)) s.limits.max_parallel_executions = Number(form.maxParallel);
  else issues.push({ path: "limits.max_parallel_executions", message: "Enter a whole number of 1 or more." });
  if (/^\s*\d+(\.\d+)?\s*$/.test(form.budget)) s.limits.session_budget_usd = Number(form.budget);
  else
    issues.push({ path: "limits.session_budget_usd", message: "Enter a dollar amount such as 25, or 0 for no limit." });

  s.mcp_servers = form.mcp.map((row, i) => mcpFromRow(row, `mcp_servers[${i}]`, issues));

  return { settings: s, issues };
}

/** JSON with object keys sorted, so two settings compare equal whatever order their tables were built in. */
export function stableJson(v: unknown): string {
  return JSON.stringify(v, (_k, x: unknown) =>
    x && typeof x === "object" && !Array.isArray(x)
      ? Object.fromEntries(
          Object.entries(x as Record<string, unknown>).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)),
        )
      : x,
  );
}

// ---------------------------------------------------------------------------------------------
// Issues, tabs and deep links
// ---------------------------------------------------------------------------------------------

export type SettingsTab =
  | "general"
  | "projects"
  | "git"
  | "routing"
  | "mcp"
  | "permissions"
  | "instructions"
  | "notifications"
  | "signin";

export const SETTINGS_TABS: { id: SettingsTab; label: string }[] = [
  { id: "general", label: "General" },
  { id: "projects", label: "Projects" },
  { id: "git", label: "Git" },
  { id: "routing", label: "Routing" },
  { id: "mcp", label: "MCP servers" },
  { id: "permissions", label: "Permissions" },
  { id: "instructions", label: "Instructions" },
  { id: "notifications", label: "Notifications" },
  { id: "signin", label: "Sign-in" },
];

/** The tab that holds a settings path or key. */
export function tabOf(path: string): SettingsTab {
  const head = path.split(/[.[]/)[0];
  if (head === "mcp_servers") return "mcp";
  if (head.startsWith("sandbox_")) return "permissions";
  if (
    head === "projects" ||
    head === "routing" ||
    head === "permissions" ||
    head === "instructions" ||
    head === "notifications"
  )
    return head;
  return "general";
}

/** Every field the form renders an issue list for, as dotted settings paths. */
export function fieldIds(form: SettingsForm): string[] {
  const ids = [
    "name",
    "yolo.default",
    "limits.max_parallel_executions",
    "limits.session_budget_usd",
    "projects",
    "instructions.all",
    "notifications.push",
  ];
  ids.push("permissions.mode", "permissions.allow", "permissions.ask", "permissions.deny");
  ids.push("sandbox_mode", "sandbox_network", "sandbox_allowed_hosts", "sandbox_decoys");
  ids.push("sandbox_loopback", "sandbox_blocked_ports");
  form.projects.forEach((_, i) => ids.push(`projects[${i}]`));
  ids.push("mcp_servers");
  form.mcp.forEach((_, i) => {
    for (const f of ["name", "url", "command", "env", "headers", "oauth", "agents", "timeout_secs"])
      ids.push(`mcp_servers[${i}].${f}`);
    ids.push(`mcp_servers[${i}]`);
  });
  for (const k of routeKeys(form))
    ids.push(
      `routing.executor.byAgent.${k}`,
      `routing.model.byAgent.${k}`,
      `routing.effort.byAgent.${k}`,
      `instructions.agents.${k}`,
    );
  for (const k of Object.keys(form.effort)) ids.push(`routing.effort.byAgent.${k}`);
  for (const a of Object.keys(form.complexityModel))
    for (const c of COMPLEXITIES)
      ids.push(
        `routing.executor.byPhaseComplexity.${a}.${c}`,
        `routing.model.byPhaseComplexity.${a}.${c}`,
        `routing.effort.byPhaseComplexity.${a}.${c}`,
      );
  ids.push("routing.executor.byPhaseComplexity", "routing.model.byPhaseComplexity", "routing.effort.byPhaseComplexity");
  return [...new Set(ids)];
}

const covers = (field: string, path: string) =>
  path === field || path.startsWith(`${field}.`) || path.startsWith(`${field}[`);

/** The field an issue belongs to: the longest field id that is the path or a parent of it. */
export function fieldForIssue(path: string, fields: string[]): string | null {
  let best: string | null = null;
  for (const f of fields) if (covers(f, path) && (!best || f.length > best.length)) best = f;
  return best;
}

export type IssueMap = {
  byField: Record<string, ValidationIssue[]>;
  unmatched: ValidationIssue[];
  byTab: Record<SettingsTab, number>;
};

export function mapIssues(issues: ValidationIssue[], fields: string[]): IssueMap {
  const byField: Record<string, ValidationIssue[]> = {};
  const unmatched: ValidationIssue[] = [];
  const byTab: Record<SettingsTab, number> = {
    general: 0,
    projects: 0,
    git: 0,
    routing: 0,
    mcp: 0,
    permissions: 0,
    instructions: 0,
    notifications: 0,
    signin: 0,
  };
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
export const settingKeyOf = (anchor: string | null): string | null =>
  anchor?.startsWith("setting:") ? anchor.slice(8) || null : null;
