import { Banner, Chip, Icon, type IconName, Spinner, StatusChip, StatusDot, type Tone } from "@ostra/design";
import { useMemo, useState } from "react";
import { api } from "../../api";
import type { ExecutionStatus, ExecutionView, SessionDetail, ToolCall } from "../../api/types";
import { STAGES } from "../../content/stages";
import { toolSummary } from "../../lib/events";
import { formatCost, formatDuration, formatTokens, humanize, truncate } from "../../lib/format";
import { useNav } from "../../lib/nav";
import { ActivityStream } from "../../screens/execution/ActivityStream";
import { inlineCode } from "../../screens/execution/inline";
import { type HookRow, hookRows, pendingAsk, relativize, terminalMode } from "../../screens/execution/model";
import { PermissionNotice } from "../../screens/execution/PermissionNotice";
import { TerminalStream } from "../../screens/execution/TerminalStream";
import { useExecution, useNow } from "../../screens/execution/useExecution";
import { sessionLabel } from "../../shell/meta";
import "../../screens/execution/execution.css";
import "./MExecution.css";

export const RUN_TONE: Record<ExecutionStatus, Tone> = {
  running: "accent",
  ok: "ok",
  stuck: "warn",
  handoff: "info",
  error: "bad",
  denied: "bad",
  interrupted: "warn",
  cancelled: "neutral",
};

export type SiblingRun = { id: string; label: string; status: ExecutionStatus };

/** The runs of this execution's group (same agent and project in the session), oldest first, with their status. */
export function siblingRunsWithStatus(
  detail: SessionDetail | null,
  e: Pick<ExecutionView, "id" | "group" | "run_label" | "status">,
): SiblingRun[] {
  const group = detail?.execution_groups.find((g) => g.group === e.group);
  if (!detail || !group) return [{ id: e.id, label: e.run_label, status: e.status }];
  const byId = new Map(detail.executions.map((x) => [x.id, x]));
  return group.executions.map((id) => {
    const x = byId.get(id);
    return { id, label: x?.run_label ?? id, status: id === e.id ? e.status : (x?.status ?? "ok") };
  });
}

const TOOL_ICON: Record<string, IconName> = {
  Read: "file",
  Edit: "file-diff",
  MultiEdit: "file-diff",
  Write: "file-plus",
  Bash: "square-terminal",
  Grep: "text-search",
  Glob: "folder-search",
  WebFetch: "globe",
  WebSearch: "globe",
  Skill: "book-open",
  Memory: "brain",
};

const DECISION: Record<string, { tone: Tone; label: string }> = {
  allow: { tone: "ok", label: "Allowed" },
  deny: { tone: "bad", label: "Denied" },
};

function HookList({ rows }: { rows: HookRow[] }) {
  if (rows.length === 0)
    return (
      <span className="m-muted" style={{ padding: "8px 0" }}>
        No tool calls yet. Each call the harness makes appears here with the decision Ostra's policy made.
      </span>
    );
  return (
    <div className="mx-hooks">
      {rows.map((r) => {
        const d = r.decision === "ask" ? null : r.decision ? DECISION[r.decision] : null;
        return (
          <div key={r.id} className="mx-hook">
            <div className="mx-hook__head">
              <Icon name={TOOL_ICON[r.tool] ?? "wrench"} size={14} style={{ color: "var(--text-muted)" }} />
              <span className="mx-hook__tool">{r.tool}</span>
              {r.durationMs !== null && <span className="mx-mono-muted">{formatDuration(r.durationMs)}</span>}
              <span style={{ marginLeft: "auto" }}>
                {r.decision === "ask" ? (
                  <Chip tone="warn">{r.done ? "Asked you" : "Asking you"}</Chip>
                ) : d ? (
                  <Chip tone={d.tone}>{d.label}</Chip>
                ) : (
                  <Chip>No decision</Chip>
                )}
              </span>
            </div>
            <span className="mx-hook__summary">{r.summary}</span>
            {r.reason && <span className="mx-hook__note">{inlineCode(r.reason)}</span>}
            {r.decision === "deny" && r.advice && (
              <span className="mx-hook__note">What to do instead: {inlineCode(r.advice)}</span>
            )}
            <span className="mx-mono-muted">{r.rule ? `${r.rule.layer} · ${r.rule.rule}` : "default"}</span>
          </div>
        );
      })}
    </div>
  );
}

/** `exec:<id>`: usage, the session link, other runs, then the Activity stream or the Terminal and its tool calls. */
export function MExecution({ id }: { ws: string; id: string }) {
  const nav = useNav();
  const { exec, status, activity: act, activityError, session } = useExecution(id);
  const [tab, setTab] = useState<"stream" | "tools">("stream");
  const [busy, setBusy] = useState<"cancel" | "resume" | "inspect" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const e = exec.data;
  const st = status ?? e?.status ?? "running";
  const running = st === "running";
  const now = useNow(running);
  const root = e?.repo_root ?? null;
  const summarize = useMemo(() => (call: ToolCall) => relativize(toolSummary(call), root), [root]);

  if (exec.error && !e)
    return (
      <div className="m-page">
        <Banner tone="bad" title="Ostra could not load this execution">
          {exec.error.message}
        </Banner>
        <button type="button" className="m-btn" onClick={exec.reload}>
          Try again
        </button>
      </div>
    );
  if (!e)
    return (
      <div className="m-page">
        <span className="m-muted" style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <Spinner size={11} /> Loading the execution…
        </span>
      </div>
    );

  const usage = act.usage ?? e.usage;
  const endMs = e.ended_at ? new Date(e.ended_at).getTime() : running ? now : new Date(e.started_at).getTime();
  const elapsed = formatDuration(Math.max(0, endMs - new Date(e.started_at).getTime()));
  const siblings = siblingRunsWithStatus(session.data, { ...e, status: st });
  const gate = e.pending_gate;
  const pending = running ? pendingAsk(act.entries) : null;
  const mode = terminalMode(e, st);
  const stageLabel = e.stage ? STAGES[e.stage]?.label : null;
  const terminal = e.stream === "terminal";
  const rows = terminal ? hookRows(act.entries, summarize) : [];
  const inspects = e.purpose?.kind === "inspect" ? e.purpose.of : null;
  const canInspect = !running && !inspects && terminal && !!e.native_session_id && !!e.session && !e.has_terminal;
  const canResume = !running && e.can_resume && !inspects && !e.has_terminal;
  const sess = session.data?.summary;

  const run = async (kind: "cancel" | "resume") => {
    setBusy(kind);
    setError(null);
    try {
      exec.set(kind === "cancel" ? await api.cancelExecution(e.id) : await api.resumeExecution(e.id));
      setTab("stream");
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(null);
    }
  };
  const inspect = async () => {
    setBusy("inspect");
    setError(null);
    try {
      nav.open(`exec:${(await api.inspectExecution(e.id)).id}`);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(null);
    }
  };

  const notice =
    gate || pending ? (
      <PermissionNotice
        stream={e.stream}
        gate={gate}
        pending={pending}
        summarize={summarize}
        onAnswered={exec.reload}
        onOpenGate={(g) => e.session && nav.open(`session:${e.session}`, g ? { anchor: `gate-${g.id}` } : {})}
      />
    ) : null;

  const stats: [string, string][] = [
    ["Executor", e.executor],
    ["Model", e.model],
    ["Cost", formatCost(usage.cost_usd)],
    ["Elapsed", elapsed],
    ["Input", formatTokens(usage.input_tokens)],
    ["Output", formatTokens(usage.output_tokens)],
  ];
  const tabs: { id: "stream" | "tools"; label: string; icon: IconName; count?: number }[] = terminal
    ? [
        { id: "stream", label: "Terminal", icon: "square-terminal" },
        { id: "tools", label: "Tool calls", icon: "list", count: rows.length },
      ]
    : [{ id: "stream", label: "Activity", icon: "activity" }];

  return (
    <div className="mx-page">
      <div className="mx-stats" role="group" aria-label="Usage">
        {stats.map(([k, v]) => (
          <div key={k} className="mx-stat">
            <span className="m-label">{k}</span>
            <span className="mx-stat__value">{v}</span>
          </div>
        ))}
      </div>

      <div className="mx-pad mx-chips">
        <StatusChip kind="execution" status={st} />
        {stageLabel && <Chip tone="accent">{stageLabel}</Chip>}
        {e.session && (
          <button type="button" className="mx-link" onClick={() => nav.open(`session:${e.session}`)}>
            <Icon name="git-pull-request" size={14} />
            {sess ? truncate(sessionLabel(sess), 40) : "Session"}
          </button>
        )}
      </div>

      {siblings.length > 1 && (
        <div className="mx-siblings">
          <span className="mx-pad mx-caption">
            Other runs of {humanize(e.agent)} in {e.project}
          </span>
          <div className="mx-chiprow">
            {siblings.map((r) => (
              <button
                type="button"
                key={r.id}
                className="mx-run"
                aria-current={r.id === e.id || undefined}
                onClick={() => r.id !== e.id && nav.open(`exec:${r.id}`)}
              >
                {r.id === e.id && <Icon name="check" size={13} style={{ color: "var(--accent)" }} />}
                <StatusDot tone={RUN_TONE[r.status]} pulse={r.status === "running"} size={6} />
                {r.label}
              </button>
            ))}
          </div>
        </div>
      )}

      {inspects && (
        <div className="mx-pad">
          <Banner tone="info" title="Read-only session" style={{ margin: 0 }}>
            This reopens a run that has ended so you can scroll its work and ask the agent about it in the terminal.
            Ostra refuses every tool call here, because the work is done.
          </Banner>
          <button type="button" className="m-btn m-btn-sm" onClick={() => nav.open(`exec:${inspects}`)}>
            Open the original run
          </button>
        </div>
      )}
      {error && (
        <div className="mx-pad">
          <Banner tone="bad" style={{ margin: 0 }}>
            {error}
          </Banner>
        </div>
      )}
      {e.error && (
        <div className="mx-pad">
          <Banner tone="bad" title="The execution ended with an error" style={{ margin: 0 }}>
            {e.error}
          </Banner>
        </div>
      )}
      {activityError && (
        <div className="mx-pad">
          <Banner tone="bad" style={{ margin: 0 }}>
            Ostra could not load the activity: {activityError.message}
          </Banner>
        </div>
      )}

      <div className="mx-tabs" role="tablist" aria-label="Execution views">
        {tabs.map((t) => (
          <button
            type="button"
            key={t.id}
            role="tab"
            aria-selected={tab === t.id}
            className="mx-tab"
            onClick={() => setTab(t.id)}
          >
            <Icon name={t.icon} size={14} />
            {t.label}
            {t.id === "stream" && running && <StatusDot tone="accent" pulse size={6} />}
            {t.count !== undefined && <span className="mx-mono-muted">{t.count}</span>}
          </button>
        ))}
      </div>

      {tab === "stream" && !terminal && (
        <div className="mx-pad mx-stream">
          {notice}
          <ActivityStream entries={act.entries} live={running} summarize={summarize} />
        </div>
      )}
      {tab === "stream" && terminal && (
        <div className="mx-pad mx-stream">
          <TerminalStream
            execution={e.id}
            mode={mode}
            title={`${e.executor.replace(/^harness:/, "")} · ${e.model}`}
            project={e.project}
            canResume={e.can_resume}
            resuming={busy === "resume"}
            onResume={() => void run("resume")}
            notice={notice}
          />
        </div>
      )}
      {tab === "tools" && terminal && (
        <div className="mx-pad">
          <HookList rows={rows} />
        </div>
      )}

      <div className="mx-pad mx-actions">
        {running && (
          <button type="button" className="m-btn mx-danger" disabled={busy !== null} onClick={() => void run("cancel")}>
            {busy === "cancel" ? "Cancelling…" : "Cancel the execution"}
          </button>
        )}
        {canResume && !terminal && (
          <button type="button" className="m-btn" disabled={busy !== null} onClick={() => void run("resume")}>
            <Icon name="rotate-ccw" size={14} />
            {busy === "resume" ? "Resuming…" : "Resume"}
          </button>
        )}
        {canInspect && (
          <button type="button" className="m-btn" disabled={busy !== null} onClick={() => void inspect()}>
            <Icon name="eye" size={14} />
            {busy === "inspect" ? "Opening…" : "Open the session to read its work"}
          </button>
        )}
        {e.report_path && (
          <button type="button" className="m-btn" onClick={() => nav.open(`artifact:${e.report_path}`)}>
            <Icon name="file-text" size={14} />
            Open the report
          </button>
        )}
      </div>
    </div>
  );
}
