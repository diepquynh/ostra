// Pure helpers for the mobile session board: lane cards, phase waves, execution groups, stage rows.

import type { IconName, LaneState, Tone } from "@ostra/design";
import type {
  ExecutionStatus,
  ExecutionView,
  Lane,
  PhaseView,
  SessionDetail,
  StageCard,
  StageStatus,
} from "../../api/types";
import { LANE_ORDER, phaseLayers } from "../../lib/events";
import { formatCost, formatDuration, humanize } from "../../lib/format";

export type LaneCard = { lane: Lane; status: LaneState["status"]; detail: string };

/** One card per lane, with the detail line the template shows under the lane name. */
export function laneCards(states: Record<Lane, LaneState>): LaneCard[] {
  return LANE_ORDER.map((lane) => {
    const l = states[lane];
    const detail =
      l.detail ??
      (l.status === "done" ? "Done" : l.status === "skipped" ? "Skipped" : l.status === "pending" ? "Not started" : "");
    return { lane, status: l.status, detail };
  });
}

export const STAGE_TONE: Record<StageStatus, Tone> = {
  pending: "neutral",
  running: "accent",
  waiting: "warn",
  done: "ok",
  skipped: "neutral",
  failed: "bad",
  blocked: "bad",
};

export const EXEC_TONE: Record<ExecutionStatus, Tone> = {
  running: "accent",
  ok: "ok",
  stuck: "warn",
  handoff: "warn",
  interrupted: "warn",
  cancelled: "neutral",
  denied: "bad",
  error: "bad",
};

export type Wave = {
  label: string;
  phases: {
    id: number;
    title: string;
    project: string;
    status: PhaseView["status"];
    meta: string;
    file: string | null;
  }[];
};

/** Phases in waves: each wave depends only on earlier waves, and phases within a wave run in parallel. */
export function phaseWaves(phases: PhaseView[]): Wave[] {
  return phaseLayers(phases).map((layer, i) => ({
    label: `Wave ${i + 1}${layer.length > 1 ? " · runs in parallel" : ""}`,
    phases: layer.map((p) => {
      const deps = p.info.depends_on;
      const parts = [
        humanize(p.info.complexity),
        `tests ${p.info.test_policy.toLowerCase()}`,
        deps?.length ? `after ${deps.map((d) => `P${d}`).join(", ")}` : null,
        p.review_iterations > 0 ? `review pass ${p.review_iterations}` : null,
        p.security_block ? "security block" : null,
      ];
      return {
        id: p.info.id,
        title: p.info.title,
        project: p.info.project,
        status: p.status,
        meta: parts.filter(Boolean).join(" · "),
        file: p.info.file,
      };
    }),
  }));
}

export type ExecRow = { id: string; run: string; icon: IconName; meta: string; cost: string; running: boolean };
export type ExecGroup = {
  key: string;
  agent: string;
  project: string;
  status: ExecutionView["status"];
  cost: string;
  rows: ExecRow[];
};

const elapsed = (x: ExecutionView, nowMs: number) => {
  const end = x.ended_at ? Date.parse(x.ended_at) : nowMs;
  return formatDuration(Math.max(0, end - Date.parse(x.started_at)));
};

/** Executions grouped by agent and project, in the order the server gives the groups. */
export function execGroups(d: SessionDetail, nowMs = Date.now()): ExecGroup[] {
  const byId = new Map(d.executions.map((x) => [x.id, x]));
  return d.execution_groups.map((g) => ({
    key: g.group,
    agent: humanize(g.agent),
    project: g.project,
    status: g.status,
    cost: formatCost(g.cost_usd),
    rows: g.executions
      .map((id) => byId.get(id))
      .filter((x): x is ExecutionView => !!x)
      .map((x) => ({
        id: x.id,
        run: x.run_label,
        icon: x.stream === "terminal" ? "square-terminal" : "activity",
        meta: [x.executor, x.model, elapsed(x, nowMs)].filter(Boolean).join(" · "),
        cost: formatCost(x.usage.cost_usd),
        running: x.status === "running",
      })),
  }));
}

/** Where tapping a stage row goes: its gate, else its running or latest execution, else nowhere. */
export function stageTarget(
  s: StageCard,
  execs: Map<string, ExecutionView>,
): { gate: string } | { exec: string } | null {
  if (s.gate && s.status === "waiting") return { gate: s.gate };
  const runs = s.executions.map((id) => execs.get(id)).filter((x): x is ExecutionView => !!x);
  const pick = runs.find((x) => x.status === "running") ?? runs[runs.length - 1];
  if (pick) return { exec: pick.id };
  return s.gate ? { gate: s.gate } : null;
}

export const artifactIcon = (kind: string): IconName =>
  kind === "upload" ? "paperclip" : kind === "ledger" ? "list-checks" : "file-text";
