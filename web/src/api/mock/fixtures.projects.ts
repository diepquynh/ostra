// Mock data for the project screens and the setup steps: the backend's initializer profile, the extra
// folders the folder picker browses (from the design kit's fsdata.js), varied modified times, and the
// server's create and validate answers for a new workspace.

import type { ProjectProfile } from "../gen/ProjectProfile";
import type { CreateWorkspace, ImportProject, ProjectView, ValidationIssue, WorkspaceDetail } from "../types";

export const backendProfile: ProjectProfile = {
  schema_version: 1,
  generated_at: "2026-09-02T14:10:00Z",
  stack: { language: "rust", frameworks: ["axum", "sqlx"], build_tool: "cargo" },
  commands: {
    build: "cargo check --workspace",
    test: "cargo test --workspace",
    test_one: "cargo test -p orders {name}",
    format: "cargo fmt --all",
    lint: "cargo clippy --workspace -- -D warnings",
    typecheck: null,
    run: null,
  },
  test_framework: "cargo test",
  test_types: {},
  module_map: [
    { glob: "crates/orders/**", area: "orders", reference: ".ostra/skills/module-hub/references/orders.md" },
    { glob: "crates/payments/**", area: "payments", reference: null },
    { glob: "migrations/**", area: "schema", reference: null },
  ],
  skills: [
    {
      name: "axum-handler",
      kind: "creation",
      path: ".ostra/skills/axum-handler/SKILL.md",
      component_type: "handler",
      source: "generated",
    },
    {
      name: "sqlx-repo",
      kind: "creation",
      path: ".ostra/skills/sqlx-repo/SKILL.md",
      component_type: "repository",
      source: "generated",
    },
    {
      name: "convention",
      kind: "convention",
      path: ".ostra/skills/convention/SKILL.md",
      component_type: null,
      source: "generated",
    },
    {
      name: "module-hub",
      kind: "module-hub",
      path: ".ostra/skills/module-hub/SKILL.md",
      component_type: null,
      source: "generated",
    },
  ],
  conventions: { immutability_keyword: null, naming: "snake_case modules, CamelCase types", notes: [] },
  review_rules: [
    { id: "R-a", rule: "Names follow the module's existing pattern.", severity: "L", auto_fixable: true },
    { id: "R-b", rule: "Errors map to the crate's Error enum, never a string.", severity: "M", auto_fixable: false },
    { id: "R-c", rule: "Writes that span tables run in one transaction.", severity: "H", auto_fixable: false },
  ],
};

/** Folders the kit lists that the Files dock's mock did not have, so every listed folder opens. */
export const extraDirs: Record<string, { name: string; is_git?: boolean; is_ostra_project?: boolean }[]> = {
  "/etc": [],
  "/opt": [],
  "/tmp": [],
  "/usr": [{ name: "local" }],
  "/usr/local": [],
  "/var": [],
  "/home/me/notes": [],
  "/home/me/Downloads": [],
  "/home/me/code/billing-service": [{ name: "migrations" }, { name: "src" }],
  "/home/me/code/billing-service/src": [],
  "/home/me/code/billing-service/migrations": [],
  "/home/me/code/shop-admin": [{ name: "app" }, { name: "public" }],
  "/home/me/code/shop-admin/app": [],
  "/home/me/code/shop-admin/public": [],
  "/home/me/code/dotfiles": [],
  "/home/me/code/shop-backend/crates": [{ name: "orders" }, { name: "payments" }],
  "/home/me/code/shop-backend/migrations": [],
  "/home/me/code/shop-backend/tests": [],
  "/home/me/code/shop-web/src": [{ name: "api" }, { name: "components" }, { name: "hooks" }, { name: "pages" }],
  "/home/me/code/shop-web/public": [],
  "/srv/repos/legacy-erp": [],
  "/srv/repos/reporting": [],
};

const NOW = Date.parse("2026-09-24T09:30:00Z");

function hash(s: string) {
  let h = 0;
  for (const c of s) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return h;
}

/** A stable modified time per path: changed files today, the rest spread over the last six weeks. */
export const modifiedFor = (path: string, changed: boolean) =>
  new Date(changed ? NOW - (hash(path) % 7200) * 1000 : NOW - (1 + (hash(path) % 42)) * 86_400_000).toISOString();

const isAbs = (p: string) => p.startsWith("/");
const isKey = (k: string) => /^[a-z0-9][a-z0-9-]*$/.test(k);
const KNOWN = new Set([
  "/home/me/code/shop-backend",
  "/home/me/code/shop-web",
  "/home/me/code/shop-admin",
  "/home/me/code/billing-service",
  "/home/me/code/dotfiles",
  "/srv/repos/legacy-erp",
  "/srv/repos/reporting",
]);

/** `POST /api/workspaces/:ws/projects`: the server's refusals, each on its request field. */
export function mockValidateImport(body: ImportProject, existing: ProjectView[]): ValidationIssue[] {
  const issues: ValidationIssue[] = [];
  const key = body.key.trim();
  if (!isKey(key))
    issues.push({
      path: "key",
      message: `\`${key}\` is not a project key. Use lowercase letters, digits, and dashes, starting with a letter or digit.`,
    });
  else if (existing.some((p) => p.key === key))
    issues.push({
      path: "key",
      message: `A project named \`${key}\` already exists in this workspace. Choose another key.`,
    });
  const stack = body.stack?.trim();
  if (stack && !isKey(stack))
    issues.push({
      path: "stack",
      message: `\`${stack}\` is not a stack name. Pick a listed stack, or leave it empty so the initializer detects it from the code.`,
    });
  if (!isAbs(body.path)) issues.push({ path: "path", message: "Use an absolute path." });
  else if (!KNOWN.has(body.path)) issues.push({ path: "path", message: `${body.path} does not exist.` });
  else {
    const other = existing.find((p) => p.path === body.path);
    if (other) issues.push({ path: "path", message: `That folder is already imported as \`${other.key}\`.` });
  }
  return issues;
}

/** `POST /api/workspaces/validate`, with the checks the server makes that the mock can reproduce. */
export function mockValidateCreate(body: CreateWorkspace, registeredRoots: string[]): ValidationIssue[] {
  const issues: ValidationIssue[] = [];
  const root = body.root.replace(/\/+$/, "");
  if (!body.name.trim()) issues.push({ path: "name", message: "Give the workspace a name." });
  if (!root) issues.push({ path: "root", message: "Choose a folder for the workspace." });
  else if (!isAbs(root)) issues.push({ path: "root", message: "Choose an absolute folder for the workspace." });
  else if (registeredRoots.includes(root))
    issues.push({ path: "root", message: "That folder is already a registered workspace." });
  const seen = new Map<string, number>();
  (body.projects ?? []).forEach((p, i) => {
    if (!isKey(p.key))
      issues.push({
        path: `projects[${i}].key`,
        message: `\`${p.key}\` is not a project key. Use lowercase letters, digits, and dashes.`,
      });
    else if (seen.has(p.key))
      issues.push({ path: `projects[${i}].key`, message: `The key \`${p.key}\` is used twice.` });
    seen.set(p.key, i);
    if (!KNOWN.has(p.path)) issues.push({ path: `projects[${i}].path`, message: `${p.path} is not a folder.` });
    if (root && p.path.startsWith(`${root}/.ostra`))
      issues.push({
        path: `projects[${i}].path`,
        message: "A project cannot live inside the workspace's .ostra directory.",
      });
  });
  return issues.sort((a, b) => a.path.localeCompare(b.path));
}

/** `POST /api/workspaces`: the new workspace with the requested projects, initialized when the folder holds `.ostra/`. */
export function mockCreateWorkspace(body: CreateWorkspace, base: WorkspaceDetail): WorkspaceDetail {
  const initialized = new Set(["/home/me/code/shop-backend", "/home/me/code/shop-web"]);
  const projects: ProjectView[] = (body.projects ?? []).map((p) => ({
    key: p.key,
    path: p.path,
    init_status: initialized.has(p.path) ? "initialized" : "not_initialized",
    ultracode_bootstrap: false,
    is_git: true,
    git_branch: "main",
    stack: p.stack,
    profile: null,
  }));
  return {
    ...base,
    id: "ws_new",
    root: body.root,
    settings: {
      ...base.settings,
      name: body.name,
      projects: projects.map((p) => ({ key: p.key, path: p.path, stack: p.stack })),
    },
    projects,
  };
}
