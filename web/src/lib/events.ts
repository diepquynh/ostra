import type {
  ActivityItem,
  DecisionView,
  ExecutionDelta,
  GateView,
  Lane,
  PhaseView,
  PolicyDecision,
  SessionDetail,
  SessionEvent,
  StageCard,
  ToolCall,
  Usage,
} from "../api/types";
import { humanize, truncate } from "./format";

// ---------------------------------------------------------------------------------------------
// Activity: fold execution deltas into display entries
// ---------------------------------------------------------------------------------------------

export type ToolEntry = {
  kind: "tool";
  seq: number;
  callId: string;
  call: ToolCall;
  policy: PolicyDecision | null;
  output: string;
  live: string;
  isError: boolean;
  durationMs: number | null;
  done: boolean;
};

export type ActivityEntry =
  | { kind: "text"; seq: number; text: string }
  | { kind: "thinking"; seq: number; text: string }
  | { kind: "status"; seq: number; message: string }
  | ToolEntry;

export type ActivityState = {
  entries: ActivityEntry[];
  lastSeq: number;
  usage: Usage | null;
  nativeSessionId: string | null;
};

export const emptyActivity = (): ActivityState => ({ entries: [], lastSeq: 0, usage: null, nativeSessionId: null });

/**
 * Apply one delta. Consecutive text or thinking deltas merge into one block, tool calls collect
 * their policy decision, live output, and result. Deltas at or below `lastSeq` are ignored, so a
 * REST snapshot and the live stream can overlap safely.
 */
export function applyDelta(state: ActivityState, seq: number, delta: ExecutionDelta): ActivityState {
  if (seq <= state.lastSeq) return state;
  const entries = state.entries.slice();
  const last = entries[entries.length - 1];
  const findTool = (callId: string) => {
    for (let i = entries.length - 1; i >= 0; i--) {
      const e = entries[i];
      if (e.kind === "tool" && e.callId === callId) return i;
    }
    return -1;
  };
  const updateTool = (callId: string, fn: (t: ToolEntry) => ToolEntry) => {
    const i = findTool(callId);
    if (i >= 0) entries[i] = fn(entries[i] as ToolEntry);
  };
  let usage = state.usage;
  let nativeSessionId = state.nativeSessionId;
  switch (delta.kind) {
    case "text":
      if (last && last.kind === "text") entries[entries.length - 1] = { ...last, text: last.text + delta.text };
      else entries.push({ kind: "text", seq, text: delta.text });
      break;
    case "thinking":
      if (last && last.kind === "thinking") entries[entries.length - 1] = { ...last, text: last.text + delta.text };
      else entries.push({ kind: "thinking", seq, text: delta.text });
      break;
    case "status":
      entries.push({ kind: "status", seq, message: delta.message });
      break;
    case "tool_call":
      entries.push({
        kind: "tool",
        seq,
        callId: delta.call_id,
        call: delta.call,
        policy: null,
        output: "",
        live: "",
        isError: false,
        durationMs: null,
        done: false,
      });
      break;
    case "policy":
      updateTool(delta.call_id, (t) => ({ ...t, policy: delta.decision }));
      break;
    case "tool_output":
      updateTool(delta.call_id, (t) => ({ ...t, live: t.live + delta.chunk }));
      break;
    case "tool_result":
      updateTool(delta.call_id, (t) => ({
        ...t,
        output: delta.output,
        isError: delta.is_error,
        durationMs: delta.duration_ms,
        done: true,
      }));
      break;
    case "usage":
      usage = delta.usage;
      break;
    case "native_session_id":
      nativeSessionId = delta.id;
      break;
  }
  return { entries, lastSeq: seq, usage, nativeSessionId };
}

export function foldActivity(items: ActivityItem[], start: ActivityState = emptyActivity()): ActivityState {
  return items.reduce((s, item) => applyDelta(s, item.seq, item.delta), start);
}

/** A short one-line summary of a tool call's input. */
export function toolSummary(call: ToolCall): string {
  const input = (call.input ?? {}) as Record<string, unknown>;
  const s = (k: string) => (typeof input[k] === "string" ? (input[k] as string) : "");
  switch (call.tool) {
    case "Read":
    case "Write":
    case "Edit":
      return s("file_path");
    case "Bash":
      return truncate(s("command"), 160);
    case "Grep":
      return `${s("pattern")}${s("path") ? ` in ${s("path")}` : ""}`;
    case "Glob":
      return `${s("pattern")}${s("path") ? ` in ${s("path")}` : ""}`;
    case "WebFetch":
      return s("url");
    case "WebSearch":
      return s("query");
    case "Skill":
      return s("name") || s("path");
    case "Memory":
      return `${s("area")}: ${truncate(s("lesson"), 100)}`;
    case "MemoryRecall":
      return `${s("area") ? `${s("area")}: ` : ""}${s("query")}`;
    default:
      if (call.tool.startsWith("submit_")) return "result submitted to Ostra";
      return truncate(JSON.stringify(call.input), 160);
  }
}

/** What to do instead after a denial, keyed by guard id. Shown under each denied call. */
const GUARD_ADVICE: Record<string, string> = {
  "write-scope":
    "This agent may write only inside its allowed directories. The agent should write its output to the session dir.",
  "no-tests-from-implementer":
    "Tests belong to the write-test stage, which runs after every phase passes review and only if you ask for tests.",
  "state-ownership":
    "Pipeline state is recorded by the engine from real results. Do the underlying work and the record updates itself.",
  "artifact-ownership":
    "Spec and plan files change only when their own agent runs again. Request the change through the pipeline.",
  "report-path": "Write the report at the exact path given in `Report file:`.",
  "lesson-gate": "Record what fixed the failure with the Memory tool before writing the report.",
  "build-streak":
    "Five builds in a row failed. The agent must return STUCK with the diagnostic so Ostra can find the missing fact.",
  "self-protection": "Ostra's own binary, configuration, and databases are off limits to agents.",
};

export function denialAdvice(decision: PolicyDecision): string | null {
  if (decision.decision === "allow") return null;
  if (decision.rule.layer === "guard") return GUARD_ADVICE[decision.rule.rule] ?? null;
  if (decision.decision === "deny")
    return "A deny rule in your permissions matched. Edit the rule in workspace settings if this should be allowed.";
  return "No rule allows this yet. Allow it once, or allow it for the whole workspace.";
}

// ---------------------------------------------------------------------------------------------
// Session board helpers
// ---------------------------------------------------------------------------------------------

export const LANE_ORDER: Lane[] = [
  "research",
  "requirements",
  "verification",
  "design",
  "build",
  "review",
  "test",
  "docs",
  "done",
];

export function stagesByLane(stages: StageCard[]): Map<Lane, StageCard[]> {
  const map = new Map<Lane, StageCard[]>();
  for (const lane of LANE_ORDER) map.set(lane, []);
  for (const s of stages) map.get(s.lane)?.push(s);
  return map;
}

export function openGates(detail: SessionDetail): GateView[] {
  return detail.gates.filter((g) => g.answer === null);
}

/** The gate the user should look at first: permission asks, then the oldest open gate. */
export function currentGate(detail: SessionDetail): GateView | null {
  const open = openGates(detail);
  if (open.length === 0) return null;
  const perm = open.find((g) => g.payload.kind === "permission");
  if (perm) return perm;
  return open.slice().sort((a, b) => a.opened_at.localeCompare(b.opened_at))[0];
}

/** Layers of the phase DAG: layer 0 has no dependencies, layer n depends only on earlier layers. */
export function phaseLayers(phases: PhaseView[]): PhaseView[][] {
  const byId = new Map(phases.map((p) => [p.info.id, p]));
  const depth = new Map<number, number>();
  const visiting = new Set<number>();
  const depthOf = (id: number): number => {
    const known = depth.get(id);
    if (known !== undefined) return known;
    if (visiting.has(id)) return 0;
    visiting.add(id);
    const p = byId.get(id);
    let deps: number[];
    if (!p) deps = [];
    else if (p.info.depends_on === null) deps = phases.filter((q) => q.info.id < id).map((q) => q.info.id);
    else deps = p.info.depends_on;
    const d = deps.length === 0 ? 0 : 1 + Math.max(...deps.map((x) => (byId.has(x) ? depthOf(x) : 0)));
    visiting.delete(id);
    depth.set(id, d);
    return d;
  };
  const layers: PhaseView[][] = [];
  for (const p of [...phases].sort((a, b) => a.info.id - b.info.id)) {
    const d = depthOf(p.info.id);
    (layers[d] ??= []).push(p);
  }
  return layers.filter(Boolean);
}

/** "Ostra chose X because Y", with X derived from the judge's output. */
export function decisionChoice(d: DecisionView): string {
  const o = (d.output ?? {}) as Record<string, unknown>;
  const str = (k: string) => (typeof o[k] === "string" ? (o[k] as string) : null);
  switch (d.judge) {
    case "classify": {
      const cat = str("category");
      const projects = Array.isArray(o.projects) ? (o.projects as string[]).join(", ") : "";
      return cat ? `to treat this as ${humanize(cat)}${projects ? ` in ${projects}` : ""}` : "a category";
    }
    case "sufficiency": {
      const items = Array.isArray(o.items) ? (o.items as { needed?: boolean }[]) : [];
      const needed = items.filter((i) => i.needed).length;
      return needed
        ? `to run ${needed} more research pass${needed === 1 ? "" : "es"}`
        : "that the research is complete";
    }
    case "stakes": {
      const s = str("stakes");
      return s
        ? `${s} stakes${s === "low" ? ", so the plan stage is skipped" : ", so a phased plan is written"}`
        : "a stakes level";
    }
    case "track":
      return str("track") === "full"
        ? "the full track, so a spec and a plan are written first"
        : "the light track, so the change is built from the research";
    case "feedback":
      return str("route") ? `to route your feedback as ${humanize(str("route")!)}` : "a route for your feedback";
    case "route_answer":
      return str("route") ? `to route your answer as ${humanize(str("route")!)}` : "a route for your answer";
    case "rescue":
      return str("action") ? `to rescue the stuck agent by ${rescueWord(str("action")!)}` : "a rescue";
    case "resolve_review":
      return str("action") === "block" ? "to block the phase" : "one more fix and verify round";
    case "yolo_answer":
      return "an answer on your behalf (YOLO)";
    case "completion":
      return "the completion report";
  }
}

function rescueWord(action: string): string {
  if (action === "explore") return "running a targeted research pass";
  if (action === "rerun") return "re-running with the missing fact";
  return "asking you";
}

/** Split a completion report into the body and its "Decided for you" section. */
export function splitDecidedForYou(markdown: string): { body: string; decided: string | null } {
  const re = /^#{1,3}\s+Decided for you\s*$/im;
  const m = re.exec(markdown);
  if (!m) return { body: markdown, decided: null };
  const start = m.index;
  const after = markdown.slice(start + m[0].length);
  const next = /^#{1,3}\s+\S/m.exec(after);
  const end = next ? start + m[0].length + next.index : markdown.length;
  return {
    body: (markdown.slice(0, start) + markdown.slice(end)).trim(),
    decided: markdown.slice(start + m[0].length, end).trim(),
  };
}

/** One-line description of a session event, for the session timeline. */
export function describeEvent(e: SessionEvent): string {
  switch (e.type) {
    case "session_created":
      return `Session created: ${truncate(e.request, 80)}`;
    case "request_amended": {
      const files = e.files.length ? ` (${e.files.length} file${e.files.length === 1 ? "" : "s"})` : "";
      const how = e.delivery === "now" ? "Context sent now" : "Context queued";
      return `${how}: ${truncate(e.text, 80)}${files}`;
    }
    case "project_created":
      return `Project ${e.project.key} created (${e.project.stack}) at ${e.project.path}`;
    case "project_init_finished":
      return `Project ${e.project} initialized`;
    case "init_step_failed":
      return `Init step of ${e.project} failed: ${truncate(e.error, 80)}`;
    case "session_paused":
      return "Session paused";
    case "session_resumed":
      return "Session continued";
    case "containment_signal":
      switch (e.signal.kind) {
        case "guard":
          return `Containment signal: the ${e.signal.rule} guard refused a tool call`;
        case "egress":
          return `Containment signal: refused a connection to ${e.signal.host}:${e.signal.port}`;
        case "decoy":
          return `Containment signal: a command opened the decoy file ${e.signal.path}`;
      }
      break;
    case "yolo_set":
      return e.enabled ? "YOLO turned on" : "YOLO turned off";
    case "decision_made":
      return `${humanize(e.judge)} decision: ${truncate(e.reason, 100)}`;
    case "decision_overridden":
      return `Decision overridden: ${truncate(e.reason, 100)}`;
    case "execution_started":
      return `${humanize(e.agent)} started in ${e.project} on ${e.executor}`;
    case "execution_resumed":
      return "Execution resumed where it stopped";
    case "execution_finished":
      return `Execution finished: ${e.result.status}`;
    case "gate_opened":
      return `Waiting: ${e.title}`;
    case "gate_answered":
      return `Gate answered by ${e.source}`;
    case "command_started":
      return `${humanize(e.purpose)} started in ${e.project}: ${truncate(e.command, 80)}`;
    case "command_ran":
      return `${humanize(e.purpose)} in ${e.project}: exit ${e.exit_code ?? "unknown"}`;
    case "autofix_applied":
      return `Applied ${e.applied.length} auto-fixable finding${e.applied.length === 1 ? "" : "s"} in phase ${e.phase}`;
    case "security_block":
      return `Security block in ${e.project} phase ${e.phase}: ${e.findings.length} finding${e.findings.length === 1 ? "" : "s"}`;
    case "phase_blocked":
      return `Phase ${e.phase} in ${e.project} blocked: ${truncate(e.reason, 80)}`;
    case "note":
      return e.message;
    case "session_completed":
      return "Session completed";
    case "session_failed":
      return `Session failed: ${e.error}`;
  }
}

/** Security blocks that have not been cleared by a later passing review. */
export function activeSecurityBlocks(events: SessionEvent[]): Extract<SessionEvent, { type: "security_block" }>[] {
  const latest = new Map<string, Extract<SessionEvent, { type: "security_block" }>>();
  for (const e of events) {
    if (e.type === "security_block") {
      const key = `${e.project}:${e.phase}:${e.tests}`;
      if (e.findings.length > 0) latest.set(key, e);
      else latest.delete(key);
    }
  }
  return [...latest.values()];
}
