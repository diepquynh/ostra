// Mock data for the workspace pages: lessons per project, a cost report, settings keys and
// settings validation that follows the server's rules closely enough to show issues per field.

import type {
  CostReport,
  CostRow,
  Lesson,
  McpServerConfig,
  McpServerStatus,
  Usage,
  ValidationIssue,
  WorkspaceSettings,
} from "../types";
import { SESSION } from "./fixtures";

const now = new Date("2026-09-22T10:00:00Z");
const at = (minutes: number) => new Date(now.getTime() + minutes * 60000).toISOString();

export const lessonsByProject: Record<string, Lesson[]> = {
  backend: [
    {
      id: 1,
      area: "order::OrderService",
      lesson: "Status changes must go through OrderStateMachine; setStatus skips the event publisher.",
      source: "implementer",
      created_at: at(-3000),
    },
    {
      id: 2,
      area: "build",
      lesson: "./mvnw needs JAVA_HOME pointing at JDK 21; the system JDK 17 fails with class file version 65.",
      source: "explore",
      created_at: at(-2000),
    },
    {
      id: 3,
      area: "db/migrations",
      lesson: "Flyway runs migrations in version order, so a new file must use the next V number even on a branch.",
      source: "code-reviewer",
      created_at: at(-1500),
    },
    {
      id: 4,
      area: "payments::RefundWebhook",
      lesson: "The payment provider retries a webhook for 72 hours; handlers must be idempotent on the event id.",
      source: "explore",
      created_at: at(-900),
    },
    {
      id: 5,
      area: "tests",
      lesson: "Integration tests need Docker running because Testcontainers starts Postgres 16 for each suite.",
      source: "user",
      created_at: at(-300),
    },
  ],
  web: [
    {
      id: 6,
      area: "src/api/client.ts",
      lesson: "The API client adds the CSRF header only on same-origin requests; tests must set the origin.",
      source: "implementer",
      created_at: at(-2500),
    },
    {
      id: 7,
      area: "build",
      lesson: "Vite needs `--host 127.0.0.1` in CI because localhost resolves to IPv6 on the runners.",
      source: "explore",
      created_at: at(-1200),
    },
  ],
};

const usage = (runs: number, cost: number, calls: number, buildMs = 0): Usage => ({
  input_tokens: Math.round(cost * 380000),
  output_tokens: Math.round(cost * 21000),
  cache_read_tokens: Math.round(cost * 1_420_000),
  cache_write_tokens: Math.round(cost * 96000),
  cache_write_1h_tokens: 0,
  cost_usd: cost,
  tool_calls: calls * runs,
  build_ms: buildMs,
});

const row = (key: string, executions: number, cost: number, calls: number, buildMs = 0): CostRow => {
  const u = usage(executions, cost, calls, buildMs);
  return {
    key,
    executions,
    usage: u,
    cache_reads_per_tool_call: u.tool_calls ? u.cache_read_tokens / u.tool_calls : 0,
  };
};

/** The report since `since`: the demo week, which matches the status bar's $4.18. */
export const costSince = (since: string): CostReport => ({
  since,
  by_session: [
    row(SESSION, 16, 3.42, 18, 262000),
    row("s_refund", 3, 0.62, 16),
    row("s_init", 2, 0.1, 9),
    row("side-panel", 2, 0.04, 6),
  ],
  by_stage: [
    row("implement", 6, 1.64, 21, 248000),
    row("fact-check-spec", 4, 0.58, 24),
    row("explore", 4, 0.5, 19),
    row("review", 3, 0.46, 15, 45000),
    row("spec", 2, 0.33, 12),
    row("plan", 1, 0.16, 11),
    row("scout", 1, 0.07, 8),
    row("classify", 3, 0.4, 0),
    row("quick-answer", 2, 0.04, 6),
  ],
  by_agent: [
    row("implementer", 6, 1.64, 21, 248000),
    row("fact-check", 4, 0.58, 24),
    row("explore", 4, 0.5, 19),
    row("code-reviewer", 3, 0.46, 15, 45000),
    row("judge", 3, 0.4, 0),
    row("generate-spec", 2, 0.33, 12),
    row("plan", 1, 0.16, 11),
    row("initializer", 1, 0.07, 8),
    row("quick-answer", 2, 0.04, 6),
  ],
  by_executor: [
    row("native", 17, 1.8, 16, 45000),
    row("harness:codex", 5, 1.52, 22, 248000),
    row("harness:claude", 1, 0.86, 18),
  ],
  total: row("total", 23, 4.18, 17, 293000),
});

export const cost: CostReport = {
  since: null,
  by_session: [
    row(SESSION, 16, 3.42, 18, 262000),
    row("s_refund", 3, 0.62, 16),
    row("s_research", 2, 0.41, 22),
    row("s_n1", 2, 0.33, 14, 31000),
    row("s_init", 4, 0.2, 9),
    row("side-panel", 2, 0.04, 6),
  ],
  by_stage: [
    row("implement", 6, 1.64, 21, 248000),
    row("fact-check-spec", 4, 0.58, 24),
    row("explore", 5, 0.55, 19),
    row("review", 3, 0.46, 15, 45000),
    row("spec", 2, 0.33, 12),
    row("plan", 1, 0.16, 11),
    row("scout", 3, 0.14, 8),
    row("classify", 5, 0.1, 0),
    row("quick-answer", 2, 0.04, 6),
    row("none", 1, 0.02, 0),
  ],
  by_agent: [
    row("implementer", 6, 1.64, 21, 248000),
    row("fact-check", 4, 0.58, 24),
    row("explore", 5, 0.55, 19),
    row("code-reviewer", 3, 0.46, 15, 45000),
    row("generate-spec", 2, 0.33, 12),
    row("plan", 1, 0.16, 11),
    row("initializer", 3, 0.14, 8),
    row("judge", 6, 0.12, 0),
    row("quick-answer", 2, 0.04, 6),
  ],
  by_executor: [
    row("native", 23, 2.64, 16, 45000),
    row("harness:codex", 5, 1.52, 22, 248000),
    row("harness:claude", 1, 0.86, 18),
  ],
  total: row("total", 29, 5.02, 17, 293000),
};

/** `ostra_core::config::SETTING_KEYS`, for mock search. */
export const SETTING_KEYS: [string, string][] = [
  ["name", "Workspace name"],
  ["projects", "Projects in this workspace"],
  ["routing.executor", "Which executor runs each agent"],
  ["routing.executor.byAgent", "Executor per agent"],
  ["routing.executor.byPhaseComplexity", "Executor per agent and phase complexity"],
  ["routing.model", "Which model or tier each agent uses"],
  ["routing.model.byAgent", "Model or tier per agent"],
  ["routing.model.byPhaseComplexity", "Model or tier per agent and phase complexity"],
  ["routing.effort", "Reasoning effort per agent"],
  ["instructions.all", "Instructions every agent receives"],
  ["instructions.agents", "Instructions per agent"],
  ["yolo.default", "Start new sessions in YOLO mode"],
  ["permissions.mode", "Permission mode for tool calls"],
  ["permissions.allow", "Tool calls allowed without asking"],
  ["permissions.ask", "Tool calls that always ask"],
  ["permissions.deny", "Tool calls that are always denied"],
  ["notifications.push", "Push notifications for gates and finished sessions"],
  ["limits.max_parallel_executions", "Executions that may run at once"],
  ["limits.session_budget_usd", "Dollars one session may spend before it pauses"],
];

const TIERS = ["fast", "balanced", "advanced", "frontier", "default"];
const ROUTED = [
  "explore",
  "generate-spec",
  "fact-check",
  "plan",
  "code-reviewer",
  "execution-path-analyzer",
  "module-documentation",
  "prompt-generation",
  "initializer",
  "quick-answer",
  "judge",
];
const NOT_INSTALLED = ["harness:agy"];

/** A subset of `validate_workspace`: the checks a user can trigger from the mock settings form. */
export function validate(s: WorkspaceSettings): ValidationIssue[] {
  const issues: ValidationIssue[] = [];
  const add = (path: string, message: string) => issues.push({ path, message });
  if (!s.name.trim()) add("name", "The workspace needs a name.");
  if (s.limits.max_parallel_executions < 1)
    add("limits.max_parallel_executions", "Allow at least one execution at a time.");
  if (!Number.isFinite(s.limits.session_budget_usd) || s.limits.session_budget_usd < 0)
    add("limits.session_budget_usd", "The budget is a dollar amount of 0 or more; 0 means no limit.");
  if (s.routing.executor.byAgent.judge)
    add("routing.executor.byAgent.judge", "Judge calls always run natively; remove this route.");
  for (const [key, ex] of Object.entries(s.routing.executor.byAgent))
    if (NOT_INSTALLED.includes(ex))
      add(`routing.executor.byAgent.${key}`, `\`${key}\` routes to ${ex}, which is not installed on this machine.`);
  for (const key of ROUTED) {
    const m = s.routing.model.byAgent[key];
    if (m === undefined)
      add(
        `routing.model.byAgent.${key}`,
        `\`${key}\` has no model route. Add \`${key}\` under \`[routing.model.byAgent]\`.`,
      );
    else if (
      typeof m === "string" &&
      !TIERS.includes(m) &&
      (s.routing.executor.byAgent[key] ?? "native") === "native" &&
      !/^(anthropic|openai):./.test(m)
    )
      add(
        `routing.model.byAgent.${key}`,
        `\`${key}\` resolves to native model \`${m}\`; native models are written \`anthropic:<model>\` or \`openai:<model>\``,
      );
  }
  const names = new Set<string>();
  s.mcp_servers.forEach((m, i) => {
    const at = `mcp_servers[${i}]`;
    if (!/^[a-z][a-z0-9-]{0,23}$/.test(m.name))
      add(
        `${at}.name`,
        "Name the server with lowercase letters, digits, and dashes, starting with a letter, at most 24 characters.",
      );
    else if (names.has(m.name)) add(`${at}.name`, `Server name \`${m.name}\` is used twice.`);
    names.add(m.name);
    if (m.url === undefined && !m.command?.length)
      add(`${at}.command`, "Set `command` to run a local server, or `url` to reach a remote one.");
    if (m.url !== undefined && !/^https?:\/\//.test(m.url))
      add(`${at}.url`, `Use an http or https URL for the server, not \`${m.url}\`.`);
    if (m.timeout_secs < 1 || m.timeout_secs > 600) add(`${at}.timeout_secs`, "Use a timeout from 1 to 600 seconds.");
  });
  return issues.sort((a, b) => a.path.localeCompare(b.path));
}

const MOCK_TOOLS: Record<string, [string, string, boolean][]> = {
  github: [
    ["search_code", "Search code across GitHub repositories.", true],
    ["get_issue", "Read one issue with its comments.", true],
    ["create_issue", "Open a new issue in a repository.", false],
    ["delete_repository", "Delete a repository.", false],
  ],
  docs: [
    ["resolve-library-id", "Find the Context7 id of a library by name.", true],
    ["get-library-docs", "Fetch current documentation for a library.", true],
  ],
  linear: [
    ["list_issues", "List issues in a team.", true],
    ["create_issue", "Create an issue.", false],
  ],
};

/** The status a mock server reports: known names connect, `linear` needs a sign-in until signed in. */
export function mockMcpStatus(m: McpServerConfig, signedIn: Set<string>): McpServerStatus {
  const base: McpServerStatus = {
    name: m.name,
    transport: m.url !== undefined ? "http" : "stdio",
    state: "connected",
    tools: [],
    signed_in: m.name === "linear" ? signedIn.has(m.name) : undefined,
  };
  if (!m.enabled) return { ...base, state: "disabled" };
  if (m.name === "linear" && !signedIn.has(m.name))
    return {
      ...base,
      state: "needs_auth",
      message: `Sign in to MCP server \`${m.name}\` in Settings, MCP servers, or give it a token in a header, because it refuses requests without one.`,
    };
  const tools = MOCK_TOOLS[m.name];
  if (!tools)
    return {
      ...base,
      state: "error",
      message: `MCP server \`${m.name}\` is not available: the server answered 404: Not Found`,
    };
  return {
    ...base,
    server_info: `${m.name}-mcp 1.4.0`,
    tools: tools.map(([name, description, read_only]) => ({
      name,
      canonical: `mcp__${m.name}__${name}`,
      description,
      read_only,
      enabled: !(m.disabled_tools ?? []).includes(name),
    })),
  };
}
