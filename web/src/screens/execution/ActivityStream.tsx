import { useLayoutEffect, useRef } from "react";
import type { ToolCall as Call } from "../../api/types";
import { Markdown } from "../../components/Markdown";
import { DiffView, StatusDot, ToolCall } from "../../design";
import { type ActivityEntry, type ToolEntry, toolSummary } from "../../lib/events";
import { formatDuration, truncate } from "../../lib/format";
import { diffStat, policyInfo, toolDiff } from "./model";

const MAX_OUTPUT = 20000;

const inputOf = (call: Call) => (call.input ?? {}) as Record<string, unknown>;

function ToolBody({ entry }: { entry: ToolEntry }) {
  const { call } = entry;
  const input = inputOf(call);
  const output = entry.done ? entry.output : entry.live;
  const diff = toolDiff(call);
  if (diff) {
    return (
      <>
        <DiffView lines={diff} />
        {entry.isError && output && <pre>{truncate(output, MAX_OUTPUT)}</pre>}
      </>
    );
  }
  if (call.tool === "Bash" && typeof input.command === "string") {
    return <pre>{`$ ${input.command}${output ? `\n${truncate(output, MAX_OUTPUT)}` : ""}`}</pre>;
  }
  if (call.tool === "Memory" && typeof input.lesson === "string") {
    return <pre style={{ whiteSpace: "pre-wrap" }}>{`${input.area ?? ""}\n${input.lesson}`}</pre>;
  }
  const summarized = ["Read", "Grep", "Glob", "WebFetch", "WebSearch", "Skill", "MemoryRecall"].includes(call.tool);
  return (
    <>
      {!summarized && <pre>{JSON.stringify(call.input, null, 2)}</pre>}
      {output && <pre>{truncate(output, MAX_OUTPUT)}</pre>}
    </>
  );
}

export function ActivityTool({ entry, summarize }: { entry: ToolEntry; summarize: (c: Call) => string }) {
  const diff = toolDiff(entry.call);
  const summary =
    diff && entry.call.tool !== "ApplyPatch" ? `${summarize(entry.call)}  ${diffStat(diff)}` : summarize(entry.call);
  const policy = policyInfo(entry.policy);
  const state = !entry.done ? "running" : entry.isError ? "error" : "done";
  return (
    <ToolCall
      tool={entry.call.tool}
      summary={summary}
      state={state}
      duration={entry.durationMs !== null ? formatDuration(entry.durationMs) : undefined}
      policy={policy}
    >
      <ToolBody entry={entry} />
    </ToolCall>
  );
}

export function ActivityItem({ entry, summarize }: { entry: ActivityEntry; summarize: (c: Call) => string }) {
  switch (entry.kind) {
    case "status":
      return <div className="ex-status">{entry.message}</div>;
    case "thinking":
      return (
        <details className="ex-thinking">
          <summary>Thinking summary</summary>
          <div>{entry.text}</div>
        </details>
      );
    case "text":
      return <Markdown text={entry.text} className="ex-text" />;
    case "tool":
      return <ActivityTool entry={entry} summarize={summarize} />;
  }
}

function scrollParent(el: HTMLElement | null): HTMLElement | null {
  for (let p = el?.parentElement ?? null; p; p = p.parentElement) {
    const o = getComputedStyle(p).overflowY;
    if (o === "auto" || o === "scroll") return p;
  }
  return null;
}

export interface ActivityStreamProps {
  entries: ActivityEntry[];
  live: boolean;
  summarize?: (c: Call) => string;
}

/** The Activity stream: status lines, thinking summaries, text, and tool calls with their policy decisions. */
export function ActivityStream({ entries, live, summarize = toolSummary }: ActivityStreamProps) {
  const end = useRef<HTMLDivElement>(null);

  // Follow new output while the reader was at the bottom of the pane before it arrived. The snapshot itself
  // does not scroll, so the page opens at the usage strip.
  const lastHeight = useRef(0);
  const lastCount = useRef(0);
  useLayoutEffect(() => {
    const pane = scrollParent(end.current);
    if (!pane) return;
    const before = lastHeight.current;
    const hadEntries = lastCount.current > 0;
    lastHeight.current = pane.scrollHeight;
    lastCount.current = entries.length;
    if (!hadEntries || !live) return;
    if (before - pane.scrollTop - pane.clientHeight < 48 && pane.scrollHeight > before) {
      pane.scrollTop = pane.scrollHeight;
      lastHeight.current = pane.scrollHeight;
    }
  }, [entries, live]);

  return (
    <div className="ex-activity">
      {entries.length === 0 && (
        <div className="ex-empty">
          {live ? "No activity yet. Items appear here as the agent works." : "This execution recorded no activity."}
        </div>
      )}
      {/* A tool row is keyed by its decision, so a deny or ask that arrives after the call remounts it open. */}
      {entries.map((e) => (
        <ActivityItem
          key={e.kind === "tool" ? `${e.callId}:${e.policy?.decision ?? ""}` : e.seq}
          entry={e}
          summarize={summarize}
        />
      ))}
      {live && (
        <div className="ex-live">
          <StatusDot tone="accent" pulse /> Streaming from the agent loop
        </div>
      )}
      <div ref={end} />
    </div>
  );
}
