import type { DiffLine, PolicyInfo, SelectOption } from "@ostra/design";
import type { ExecutionStatus, ExecutionView, PolicyDecision, SessionDetail, ToolCall } from "../../api/types";
import { type ActivityEntry, denialAdvice, type ToolEntry } from "../../lib/events";

/**
 * How the Terminal stream shows a harness run: `live` while a PTY exists or the run is still going (keys and
 * resizes go to it), `replay` for an ended run with a stored transcript (read-only), `none` when there is nothing
 * to show.
 */
export type TerminalMode = "live" | "replay" | "none";

export function terminalMode(
  e: Pick<ExecutionView, "has_terminal" | "has_transcript">,
  status: ExecutionStatus,
): TerminalMode {
  if (e.has_terminal || status === "running") return "live";
  return e.has_transcript ? "replay" : "none";
}

/** The other runs of this execution's group (same agent and project in the session), oldest first. */
export function siblingRuns(
  detail: SessionDetail | null,
  e: Pick<ExecutionView, "id" | "group" | "run_label">,
): SelectOption[] {
  const group = detail?.execution_groups.find((g) => g.group === e.group);
  if (!group) return [{ value: e.id, label: e.run_label }];
  const labels = new Map(detail!.executions.map((x) => [x.id, x.run_label]));
  return group.executions.map((id) => ({ value: id, label: labels.get(id) ?? id }));
}

/** The last tool call still waiting on an ask decision, as the activity stream records it. */
export function pendingAsk(entries: ActivityEntry[]): ToolEntry | null {
  for (let i = entries.length - 1; i >= 0; i--) {
    const e = entries[i];
    if (e.kind === "tool") return e.policy?.decision === "ask" && !e.done ? e : null;
  }
  return null;
}

export const toolEntries = (entries: ActivityEntry[]): ToolEntry[] =>
  entries.filter((e): e is ToolEntry => e.kind === "tool");

/** The design's ToolCall policy line for a decision. An allow with no rule is the default and shows nothing. */
export function policyInfo(p: PolicyDecision | null): PolicyInfo | undefined {
  if (!p) return undefined;
  if (p.decision === "allow") return p.rule ? { decision: "allow", layer: p.rule.layer, rule: p.rule.rule } : undefined;
  return {
    decision: p.decision,
    layer: p.rule.layer,
    rule: p.rule.rule,
    reason: p.reason,
    advice: denialAdvice(p) ?? undefined,
  };
}

const str = (input: unknown, key: string): string | null => {
  const v = (input as Record<string, unknown> | null)?.[key];
  return typeof v === "string" ? v : null;
};

const splitLines = (s: string): string[] => (s === "" ? [] : s.replace(/\n$/, "").split("\n"));

/** Unified lines for one string replaced by another: the shared first and last lines stay as context. */
export function replaceDiff(before: string, after: string): DiffLine[] {
  const a = splitLines(before);
  const b = splitLines(after);
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head++;
  let tail = 0;
  while (tail < a.length - head && tail < b.length - head && a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail++;
  return [
    ...a.slice(0, head).map((text) => ({ type: "ctx" as const, text })),
    ...a.slice(head, a.length - tail).map((text) => ({ type: "del" as const, text })),
    ...b.slice(head, b.length - tail).map((text) => ({ type: "add" as const, text })),
    ...a.slice(a.length - tail).map((text) => ({ type: "ctx" as const, text })),
  ];
}

const MAX_WRITE_LINES = 400;

/** Lines of a Codex `*** Begin Patch` body. File headers and hunk markers read as context. */
function patchDiff(patch: string): DiffLine[] {
  return splitLines(patch)
    .filter((l) => l !== "*** Begin Patch" && l !== "*** End Patch")
    .map((l) => {
      if (l.startsWith("+")) return { type: "add" as const, text: l.slice(1) };
      if (l.startsWith("-")) return { type: "del" as const, text: l.slice(1) };
      return { type: "ctx" as const, text: l.startsWith(" ") ? l.slice(1) : l };
    });
}

/** The inline diff an Edit, Write or ApplyPatch call shows, or null for every other tool. */
export function toolDiff(call: ToolCall): DiffLine[] | null {
  switch (call.tool) {
    case "Edit": {
      const before = str(call.input, "old_string");
      const after = str(call.input, "new_string");
      return before === null || after === null ? null : replaceDiff(before, after);
    }
    case "Write": {
      const content = str(call.input, "content");
      if (content === null) return null;
      const lines = splitLines(content);
      const shown = lines.slice(0, MAX_WRITE_LINES).map((text) => ({ type: "add" as const, text }));
      const rest = lines.length - shown.length;
      return rest > 0 ? [...shown, { type: "ctx", text: `${rest} more lines not shown` }] : shown;
    }
    case "ApplyPatch": {
      const patch = str(call.input, "patch");
      return patch === null ? null : patchDiff(patch);
    }
    default:
      return null;
  }
}

/** `+3 −1` for a diff, counting only changed lines. */
export function diffStat(lines: DiffLine[]): string {
  const add = lines.filter((l) => l.type === "add").length;
  const del = lines.filter((l) => l.type === "del").length;
  return `+${add} −${del}`;
}

/** Tool summaries with paths under `root` (`ExecutionView.repo_root`) shown project-relative. */
export function relativize(text: string, root: string | null): string {
  if (!root) return text;
  const prefix = root.endsWith("/") ? root : `${root}/`;
  return text.split(prefix).join("");
}

export type HookRow = {
  id: string;
  tool: string;
  summary: string;
  decision: "allow" | "ask" | "deny" | null;
  rule: { layer: string; rule: string } | null;
  reason: string | null;
  advice: string | null;
  done: boolean;
  durationMs: number | null;
};

/** One row per tool call the hook bridge saw, with the decision the policy made. */
export function hookRows(entries: ActivityEntry[], summarize: (call: ToolCall) => string): HookRow[] {
  return toolEntries(entries).map((t) => {
    const p = t.policy;
    return {
      id: t.callId,
      tool: t.call.tool,
      summary: summarize(t.call),
      decision: p?.decision ?? null,
      rule: p?.rule ?? null,
      reason: p && p.decision !== "allow" ? p.reason : null,
      advice: p ? denialAdvice(p) : null,
      done: t.done,
      durationMs: t.durationMs,
    };
  });
}
