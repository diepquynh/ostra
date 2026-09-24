// Pure state for the setup steps: values, step order, what each step needs before Continue, the
// request body, the workspace.toml preview, and which step a validation issue belongs to.

import type { CreateWorkspace, EnvironmentStatus, HarnessKind, PermissionMode, RoutingPreset, ValidationIssue } from "../../api/types";

export type StepId = "welcome" | "check" | "name" | "projects" | "defaults" | "review";

export type WizardStep = { id: StepId; label: string; hint: string };

export const ALL_STEPS: WizardStep[] = [
  { id: "welcome", label: "Welcome", hint: "What Ostra does" },
  { id: "check", label: "Check this machine", hint: "Keys and harnesses" },
  { id: "name", label: "Name and folder", hint: "Where .ostra/ lives" },
  { id: "projects", label: "Add projects", hint: "Folders to work on" },
  { id: "defaults", label: "Defaults", hint: "Permissions and routing" },
  { id: "review", label: "Review", hint: "Create the workspace" },
];

export const stepsFor = (skipWelcome: boolean): WizardStep[] => (skipWelcome ? ALL_STEPS.filter((s) => s.id !== "welcome") : ALL_STEPS);

/**
 * Stack choices: detection first, then the stacks the server has a seed reference for
 * (`EnvironmentStatus.stacks` or `WorkspaceDetail.stacks`). A `current` value the list does not name stays selectable.
 */
export function stackOptions(stacks: string[], current = ""): { value: string; label: string }[] {
  const names = current && !stacks.includes(current) ? [...stacks, current] : stacks;
  return [{ value: "", label: "Detect from the code" }, ...names.map((s) => ({ value: s, label: s }))];
}

export type DraftProject = {
  key: string;
  /** Absolute, with "~" expanded when the home folder is known. */
  path: string;
  stack: string;
  /** From the folder listing: `.ostra/INVENTORY.md` exists, so the project is already initialized. */
  isOstraProject: boolean;
  isGit: boolean;
};

export type WizardValues = {
  name: string;
  /** False until the user types a name; until then the name follows the chosen folder. */
  nameTouched: boolean;
  root: string;
  projects: DraftProject[];
  mode: PermissionMode;
  preset: RoutingPreset;
  yolo: boolean;
  push: boolean;
};

export const initialValues = (): WizardValues => ({
  name: "",
  nameTouched: false,
  root: "~/",
  projects: [],
  mode: "default",
  preset: "native",
  yolo: false,
  push: true,
});

/** Same slug rule as the server: lowercase letters, digits, and dashes. */
export function suggestKey(folder: string): string {
  const slug = folder
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug || "project";
}

/** The repository name at the end of a git URL: `git@github.com:acme/shop-api.git` gives `shop-api`. */
export function repoName(url: string): string {
  const tail = url.trim().replace(/\/+$/, "").split(/[/:]/).pop() ?? "";
  return tail.replace(/\.git$/, "");
}

export const isProjectKey = (k: string) => /^[a-z0-9][a-z0-9-]*$/.test(k);

export function keyError(key: string, taken: string[]): string | null {
  if (!key) return "Give the project a key.";
  if (!isProjectKey(key)) return "Use lowercase letters, digits, and dashes, starting with a letter or digit.";
  if (taken.includes(key)) return "This key is already used in the workspace.";
  return null;
}

export const basename = (p: string) => p.replace(/\/+$/, "").split("/").pop() ?? "";

/** A typed path picks a folder when it is absolute (or starts with "~/") and does not end in "/". */
export const isChosen = (p: string) => (p.startsWith("/") || p.startsWith("~/")) && !p.endsWith("/") && basename(p) !== "";

export const expandHome = (p: string, home: string | null) => (home && (p === "~" || p.startsWith("~/")) ? home + p.slice(1) : p);

/** Set the folder. Until the user types a name, the name follows the chosen folder's. */
export function patchName(v: WizardValues, root: string): Partial<WizardValues> {
  return v.nameTouched ? { root } : { root, name: isChosen(root) ? basename(root) : "" };
}

/** The name step needs a name and a chosen folder; the folder may not exist yet, because Ostra creates it. */
export function canContinue(id: StepId, v: WizardValues): boolean {
  if (id === "name") return v.name.trim() !== "" && isChosen(v.root);
  return true;
}

/** The folder to start browsing for projects: the one that holds the workspace folder. */
export const projectStart = (root: string) => (isChosen(root) ? root.slice(0, root.lastIndexOf("/") + 1) : "~/");

export type ImportField = "key" | "path" | "stack";
export type ImportErrors = Partial<Record<ImportField, string>>;

/**
 * Place the import endpoint's 422 issues on the form's fields (`key`, `path`, `stack`). Issues on no known field,
 * or an error without issues, come back as `general`.
 */
export function importErrors(issues: ValidationIssue[], message: string): { fields: ImportErrors; general: string | null } {
  const fields: ImportErrors = {};
  const rest: string[] = [];
  for (const i of issues) {
    if (i.path === "key" || i.path === "path" || i.path === "stack") fields[i.path] = fields[i.path] ? `${fields[i.path]} ${i.message}` : i.message;
    else rest.push(i.message);
  }
  const general = rest.length ? rest.join(" ") : issues.length ? null : message;
  return { fields, general };
}

export function toCreateBody(v: WizardValues, home: string | null): CreateWorkspace {
  return {
    name: v.name.trim(),
    root: expandHome(v.root.trim(), home),
    projects: v.projects.map((p) => ({ key: p.key, path: expandHome(p.path, home), stack: p.stack || null })),
    permissions: { mode: v.mode },
    yolo: { default: v.yolo },
    routing_preset: v.preset,
    notifications: { push: v.push },
  };
}

const PRESET_HARNESS: Record<Exclude<RoutingPreset, "native">, HarnessKind> = { codex: "codex", claude: "claude" };

/** Agents a harness preset moves off the native loop (`RoutingPreset::HARNESS_AGENTS`). */
export const HARNESS_AGENTS = ["implementer", "write-test"];

export const presetExecutor = (p: RoutingPreset) => (p === "native" ? "native" : `harness:${PRESET_HARNESS[p]}`);

export const PRESET_LABEL: Record<RoutingPreset, string> = { native: "All native", codex: "Implementers on Codex", claude: "Implementers on Claude Code" };

/** Presets for the harnesses that are installed and logged in. Native is always offered. */
export function presetsFor(env: EnvironmentStatus | null): RoutingPreset[] {
  const ready = new Set((env?.harnesses ?? []).filter((h) => h.installed && h.logged_in === true).map((h) => h.harness));
  return ["native", ...(["codex", "claude"] as const).filter((p) => ready.has(PRESET_HARNESS[p]))];
}

const tomlString = (s: string) => `"${s.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;

/** The part of `.ostra/workspace.toml` the setup decides. The server adds the seeded model routes and limits. */
export function tomlFor(v: WizardValues, home: string | null = null): string {
  const out = [`name = ${tomlString(v.name.trim() || "workspace")}`];
  for (const p of v.projects) {
    out.push("", "[[projects]]", `key = ${tomlString(p.key)}`, `path = ${tomlString(expandHome(p.path, home))}`);
    if (p.stack) out.push(`stack = ${tomlString(p.stack)}`);
  }
  if (v.preset !== "native") {
    out.push("", "[routing.executor.byAgent]");
    for (const a of HARNESS_AGENTS) out.push(`${/^[a-z]+$/.test(a) ? a : tomlString(a)} = ${tomlString(presetExecutor(v.preset))}`);
  }
  out.push("", "[yolo]", `default = ${v.yolo}`, "", "[permissions]", `mode = ${tomlString(v.mode)}`, "", "[notifications]", `push = ${v.push}`);
  return out.join("\n") + "\n";
}

/** The step whose fields a validation issue path names. */
export function stepForIssue(path: string): StepId {
  if (path === "name" || path === "root") return "name";
  if (path.startsWith("projects")) return "projects";
  if (/^(permissions|yolo|routing|routing_preset|notifications)\b/.test(path)) return "defaults";
  return "review";
}

/** Index of the project a `projects[i].field` issue names, or null. */
export function projectIndex(path: string): number | null {
  const m = /^projects\[(\d+)\]/.exec(path);
  return m ? Number(m[1]) : null;
}

export function issuesByStep(issues: ValidationIssue[]): Partial<Record<StepId, ValidationIssue[]>> {
  const out: Partial<Record<StepId, ValidationIssue[]>> = {};
  for (const i of issues) (out[stepForIssue(i.path)] ??= []).push(i);
  return out;
}

/** Rows of the animated creation checklist. */
export const creationTasks = (v: WizardValues) => [
  "Write .ostra/workspace.toml",
  "Create the workspace database",
  "Register the workspace in registry.db",
  ...v.projects.map((p) => `Import ${p.key}`),
];

export const PROVIDER_LABEL: Record<string, string> = { anthropic: "Anthropic", openai: "OpenAI" };
export const HARNESS_LABEL: Record<HarnessKind, string> = { claude: "Claude Code", codex: "Codex", grok: "Grok Build", agy: "Antigravity" };

/** `env:ANTHROPIC_API_KEY` reads as "env ANTHROPIC_API_KEY"; `none` as "not set". */
export const keySource = (s: string) => (s === "none" ? "not set" : s.replace(/^env:/, "env "));
