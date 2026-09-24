import type { CostReport, CostRow, SessionSummary, StageKind } from "../../api/types";
import { STAGES } from "../../content/stages";
import { formatCost, formatDuration, formatTokens, humanize, truncate } from "../../lib/format";
import { sessionTitle } from "./sessionFilter";

export type CostGroup = "session" | "stage" | "agent" | "executor";

export const COST_GROUPS: { id: CostGroup; title: string; column: string; field: keyof Omit<CostReport, "total" | "since"> }[] = [
  { id: "session", title: "By session", column: "Session", field: "by_session" },
  { id: "stage", title: "By stage", column: "Stage", field: "by_stage" },
  { id: "agent", title: "By agent", column: "Agent", field: "by_agent" },
  { id: "executor", title: "By executor", column: "Executor", field: "by_executor" },
];

/** One table row, formatted. Empty strings mean "nothing to show", such as build time for a run with no builds. */
export type CostLine = {
  id: string;
  label: string;
  /** Machine names (agents, executors, unknown ids) render in mono. */
  mono: boolean;
  /** The resource the row opens, for sessions. */
  open: string | null;
  runs: number;
  input: string;
  output: string;
  cacheReads: string;
  /** Cache writes by TTL, priced at 1.25x (5 minutes) and 2x (1 hour) the input rate on Anthropic. */
  cacheWrites5m: string;
  cacheWrites1h: string;
  perCall: string;
  build: string;
  cost: string;
  /** Share of the total cost, such as "34%". */
  share: string;
};

function label(group: CostGroup, key: string, sessions: Map<string, SessionSummary>): { label: string; mono: boolean; open: string | null } {
  switch (group) {
    case "session": {
      if (key === "side-panel") return { label: "Side-panel questions", mono: false, open: null };
      const s = sessions.get(key);
      return s ? { label: truncate(sessionTitle(s), 90), mono: false, open: `session:${key}` } : { label: key, mono: true, open: `session:${key}` };
    }
    case "stage":
      if (key === "none") return { label: "No stage", mono: false, open: null };
      return { label: STAGES[key as StageKind]?.label ?? humanize(key), mono: false, open: null };
    default:
      return { label: key, mono: true, open: null };
  }
}

/** The rows of one grouping, highest cost first, with labels resolved and numbers formatted. */
export function costLines(rows: CostRow[], group: CostGroup, total: number, sessions: SessionSummary[] = []): CostLine[] {
  const byId = new Map(sessions.map((s) => [s.id, s]));
  return [...rows]
    .sort((a, b) => b.usage.cost_usd - a.usage.cost_usd || a.key.localeCompare(b.key))
    .map((r) => ({
      id: r.key,
      ...label(group, r.key, byId),
      runs: r.executions,
      input: formatTokens(r.usage.input_tokens),
      output: formatTokens(r.usage.output_tokens),
      cacheReads: formatTokens(r.usage.cache_read_tokens),
      cacheWrites5m: formatTokens(r.usage.cache_write_tokens - r.usage.cache_write_1h_tokens),
      cacheWrites1h: formatTokens(r.usage.cache_write_1h_tokens),
      perCall: r.usage.tool_calls > 0 ? formatTokens(r.cache_reads_per_tool_call) : "",
      build: r.usage.build_ms > 0 ? formatDuration(r.usage.build_ms) : "",
      cost: formatCost(r.usage.cost_usd),
      share: total > 0 ? `${Math.round((r.usage.cost_usd / total) * 100)}%` : "",
    }));
}

/** The figures in the summary strip. */
export function costTotals(report: CostReport) {
  const u = report.total.usage;
  return {
    total: formatCost(u.cost_usd),
    runs: String(report.total.executions),
    cacheReads: formatTokens(u.cache_read_tokens),
    toolCalls: String(u.tool_calls),
    perCall: u.tool_calls > 0 ? formatTokens(report.total.cache_reads_per_tool_call) : "none",
    build: u.build_ms > 0 ? formatDuration(u.build_ms) : "none",
  };
}
