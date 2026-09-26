// Pure helpers for the session board: lane states, gate order, the phase graph, and the
// event log. The screen renders what these return.

import type { LaneState, PhaseNodeProps } from "@ostra/design";
import type { ExecutionView, GateView, Lane, PhaseView, SessionDetail, StageCard, StoredEvent } from "../../api/types";
import { LANES } from "../../content/stages";
import { currentGate, describeEvent, LANE_ORDER, phaseLayers, stagesByLane } from "../../lib/events";
import { humanize } from "../../lib/format";

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** One entry of the lane stepper per lane, from the stage cards. */
export function laneStates(d: SessionDetail): Record<Lane, LaneState> {
  const byLane = stagesByLane(d.stages);
  const terminal = d.summary.status === "completed";
  const out = {} as Record<Lane, LaneState>;
  LANE_ORDER.forEach((lane, i) => {
    const items = byLane.get(lane) ?? [];
    const why = LANES[lane].why;
    const later = LANE_ORDER.slice(i + 1).some((l) => (byLane.get(l) ?? []).some((s) => s.status !== "pending"));
    const find = (...st: StageCard["status"][]) => items.find((s) => st.includes(s.status));
    if (items.length === 0) {
      out[lane] = { status: terminal || later ? "skipped" : "pending", why };
      return;
    }
    const waiting = find("waiting");
    const running = find("running");
    const failed = find("failed", "blocked");
    const done = items.filter((s) => s.status === "done" || s.status === "skipped");
    if (waiting) out[lane] = { status: "waiting", detail: "Waiting for you", why };
    else if (running) out[lane] = { status: "current", detail: running.detail ?? running.label, why };
    else if (failed) out[lane] = { status: "failed", detail: failed.detail ?? humanize(failed.status), why };
    else if (done.length === items.length) {
      const allSkipped = items.every((s) => s.status === "skipped");
      out[lane] = {
        status: allSkipped ? "skipped" : "done",
        detail: allSkipped ? "Skipped" : (done[done.length - 1].detail ?? plural(items.length, "stage")),
        why,
      };
    } else if (done.length > 0)
      out[lane] = { status: "current", detail: `${done.length} of ${items.length} done`, why };
    else out[lane] = { status: "pending", why };
  });
  return out;
}

/** The lane to show first: the current gate's lane, else a lane waiting on you, else the running one. */
export function defaultLane(d: SessionDetail): Lane {
  const gate = currentGate(d);
  const byGate = gate && d.stages.find((s) => s.gate === gate.id);
  if (byGate) return byGate.lane;
  const states = laneStates(d);
  return (
    LANE_ORDER.find((l) => states[l].status === "waiting") ??
    LANE_ORDER.find((l) => states[l].status === "current") ??
    d.summary.lane
  );
}

/** Open gates with the current one first (permission asks, then the oldest), then the rest oldest first. */
export function openGatesInOrder(d: SessionDetail): GateView[] {
  const current = currentGate(d);
  const rest = d.gates
    .filter((g) => g.answer === null && g !== current)
    .sort((a, b) => a.opened_at.localeCompare(b.opened_at));
  return current ? [current, ...rest] : rest;
}

/** Answered gates, newest answer first. */
export function answeredGates(d: SessionDetail): GateView[] {
  return d.gates
    .filter((g) => g.answer !== null)
    .sort((a, b) => (b.answered_at ?? b.opened_at).localeCompare(a.answered_at ?? a.opened_at));
}

/** The right-aligned text of a stage row: what a running execution is doing, else the stage's detail. */
export function stageMeta(s: StageCard, execs: Map<string, ExecutionView>): string | undefined {
  if (s.status === "running") {
    const live = s.executions.map((id) => execs.get(id)).find((x) => x?.status === "running" && x.summary);
    if (live?.summary) return live.summary;
  }
  const parts = [s.detail, s.project && !s.label.includes(s.project) ? s.project : null].filter(Boolean);
  return parts.length ? parts.join(" · ") : undefined;
}

export const stageKey = (s: StageCard, i: number) => `${s.stage}:${s.project ?? ""}:${s.phase ?? ""}:${i}`;

/** Phase graph columns for the design PhaseDag; each column depends only on columns to its left. */
export function phaseNodes(phases: PhaseView[]): PhaseNodeProps[][] {
  return phaseLayers(phases).map((layer) =>
    layer.map((p) => ({
      id: p.info.id,
      title: p.info.title,
      project: p.info.project,
      complexity: p.info.complexity,
      testPolicy: p.info.test_policy,
      deliverable: p.info.deliverable ?? undefined,
      status: p.status,
      reviewPass: p.review_iterations,
      securityBlock: p.security_block,
      dependsOn: p.info.depends_on ?? undefined,
    })),
  );
}

/** The REST snapshot plus live events, deduplicated by sequence number and in order. */
export function mergeEvents(snapshot: StoredEvent[], live: StoredEvent[]): StoredEvent[] {
  const bySeq = new Map<number, StoredEvent>();
  for (const e of [...snapshot, ...live]) bySeq.set(e.seq, e);
  return [...bySeq.values()].sort((a, b) => a.seq - b.seq);
}

const clock = (iso: string) => {
  const d = new Date(iso);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
};

/** One fixed-width line of the event log: time, event type, description. */
export function eventLine(e: StoredEvent): string {
  return `${clock(e.at)}  ${e.event.type.replace(/_/g, ".").padEnd(20)}  ${describeEvent(e.event)}`;
}

/** "started 13:47" for today, else the date too. */
export function startedLabel(iso: string, nowMs = Date.now()): string {
  const d = new Date(iso);
  const sameDay = new Date(nowMs).toDateString() === d.toDateString();
  return sameDay
    ? `started ${d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })}`
    : `started ${d.toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })}`;
}
