import {
  Banner,
  Button,
  Chip,
  Icon,
  Input,
  Select,
  Spinner,
  StatusChip,
  StatusDot,
  type TabItem,
  Tabs,
} from "@ostra/design";
import { useMemo, useState } from "react";
import { api } from "../api";
import type { ExecutionView, ToolCall, Usage } from "../api/types";
import { STAGES } from "../content/stages";
import { toolSummary } from "../lib/events";
import { formatCost, formatDuration, formatTokens, humanize } from "../lib/format";
import { useNav } from "../lib/nav";
import { ActivityStream } from "./execution/ActivityStream";
import { HookLog } from "./execution/HookLog";
import { hookRows, pendingAsk, relativize, siblingRuns, terminalMode } from "./execution/model";
import { PermissionNotice } from "./execution/PermissionNotice";
import { TerminalStream } from "./execution/TerminalStream";
import { useExecution, useNow } from "./execution/useExecution";
import "./execution/execution.css";

export type ExecutionScreenProps = {
  ws: string;
  /** Execution id. */
  id: string;
};

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="ex-stat">
      <span className="ex-stat__label">{label}</span>
      <span className="ex-stat__value">{value}</span>
    </div>
  );
}

function UsageStrip({ usage, duration }: { usage: Usage; duration: string }) {
  const perCall = usage.tool_calls > 0 ? usage.cache_read_tokens / usage.tool_calls : null;
  return (
    <div className="ex-stats" role="group" aria-label="Usage">
      <Stat label="Input" value={formatTokens(usage.input_tokens)} />
      <Stat label="Output" value={formatTokens(usage.output_tokens)} />
      <Stat label="Cache reads" value={formatTokens(usage.cache_read_tokens)} />
      <Stat label="Tool calls" value={String(usage.tool_calls)} />
      {perCall !== null && <Stat label="Cache / call" value={formatTokens(perCall)} />}
      {usage.build_ms > 0 && <Stat label="Build time" value={formatDuration(usage.build_ms)} />}
      <Stat label="Cost" value={formatCost(usage.cost_usd)} />
      <Stat label="Duration" value={duration} />
    </div>
  );
}

function SpawnSection({
  e,
  nativeSessionId,
  onOpenReport,
}: {
  e: ExecutionView;
  nativeSessionId: string | null;
  onOpenReport: (path: string) => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className="ex-spawn">
      <div
        className="ex-spawn__head"
        role="button"
        tabIndex={0}
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        onKeyDown={(ev) => (ev.key === "Enter" || ev.key === " ") && (ev.preventDefault(), setOpen((o) => !o))}
      >
        <Icon
          name="chevron-right"
          size={12}
          style={{ transform: open ? "rotate(90deg)" : "none", transition: "transform var(--dur-fast)" }}
        />
        <Icon name="braces" size={13} />
        <span className="ex-spawn__title">Spawn parameters</span>
        <span style={{ flex: 1 }} />
        {e.report_path && <span className="ex-mono ex-muted ex-ellipsis">{e.report_path.split("/").pop()}</span>}
      </div>
      {open && (
        <div className="ex-spawn__body">
          <div className="ex-hint">
            The exact parameter block the agent received, followed by the project brief. No agent sees another agent's
            conversation.
          </div>
          <pre>{e.spawn_block}</pre>
          {e.report_path && (
            <div className="ex-hint">
              Report:{" "}
              <a
                href="#"
                className="ex-mono"
                onClick={(ev) => {
                  ev.preventDefault();
                  onOpenReport(e.report_path!);
                }}
              >
                {e.report_path}
              </a>
            </div>
          )}
          {nativeSessionId && (
            <div className="ex-hint">
              Harness session id <code>{nativeSessionId}</code>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/** Rule U2: a correction for this run. A running run stops and resumes with it; a paused run reads it on continue. */
function SteerBox({ e, onView }: { e: ExecutionView; onView: (v: ExecutionView) => void }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const running = e.status === "running";
  const call = (p: Promise<ExecutionView>, clear: boolean) => {
    setBusy(true);
    setError(null);
    p.then(
      (v) => {
        onView(v);
        if (clear) setText("");
      },
      (err: Error) => setError(err.message),
    ).finally(() => setBusy(false));
  };
  return (
    <div className="ex-steer" style={{ display: "grid", gap: 6, margin: "8px 0" }}>
      {e.queued_steer && (
        <Banner
          tone="info"
          title="Correction waiting for the session to continue"
          actions={
            <Button size="sm" disabled={busy} onClick={() => call(api.withdrawSteer(e.id), false)}>
              Withdraw
            </Button>
          }
        >
          {e.queued_steer}
        </Banner>
      )}
      <Input
        multiline
        rows={2}
        size="sm"
        aria-label="Correction for this run"
        placeholder="Tell this agent what to do differently"
        value={text}
        onChange={(ev) => setText(ev.target.value)}
        onKeyDown={(ev) => {
          if (ev.key === "Enter" && (ev.metaKey || ev.ctrlKey) && text.trim() && !busy) {
            ev.preventDefault();
            call(api.steerExecution(e.id, text), true);
          }
        }}
        hint={
          running
            ? "Send now stops this run and resumes it in the same conversation with your correction. It cannot be withdrawn."
            : "The session is paused, so this run reads the correction when you continue. You can withdraw it until then."
        }
      />
      {error && <Banner tone="bad">{error}</Banner>}
      <div>
        <Button
          size="sm"
          variant="primary"
          icon="message-square"
          disabled={busy || !text.trim()}
          onClick={() => call(api.steerExecution(e.id, text), true)}
        >
          {running ? "Send now" : "Queue for the resume"}
        </Button>
      </div>
    </div>
  );
}

/** Resource `exec:<id>`: usage, then the Activity or Terminal stream the execution's `stream` names. */
export function ExecutionScreen({ id }: ExecutionScreenProps) {
  const nav = useNav();
  const { exec, status, activity: act, activityError, session } = useExecution(id);
  const [tab, setTab] = useState<"stream" | "tools">("stream");
  const [busy, setBusy] = useState<"cancel" | "skip" | "resume" | "inspect" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const e = exec.data;
  const st = status ?? e?.status ?? "running";
  const running = st === "running";
  const now = useNow(running);
  const root = e?.repo_root ?? null;
  const summarize = useMemo(() => (call: ToolCall) => relativize(toolSummary(call), root), [root]);

  if (exec.error && !e) {
    return (
      <div className="ex-page">
        <Banner
          tone="bad"
          title="Ostra could not load this execution"
          actions={
            <Button size="sm" onClick={exec.reload}>
              Try again
            </Button>
          }
        >
          {exec.error.message}
        </Banner>
      </div>
    );
  }
  if (!e) {
    return (
      <div className="ex-page ex-loading">
        <Spinner size={11} /> Loading the execution…
      </div>
    );
  }

  const usage = act.usage ?? e.usage;
  const endMs = e.ended_at ? new Date(e.ended_at).getTime() : running ? now : new Date(e.started_at).getTime();
  const duration = formatDuration(Math.max(0, endMs - new Date(e.started_at).getTime()));
  const siblings = siblingRuns(session.data, e);
  const gate = e.pending_gate;
  const pending = running ? pendingAsk(act.entries) : null;
  const mode = terminalMode(e, st);
  const stageLabel = e.stage ? STAGES[e.stage]?.label : null;

  const run = async (kind: "cancel" | "skip" | "resume") => {
    setBusy(kind);
    setError(null);
    try {
      const next =
        kind === "cancel"
          ? await api.cancelExecution(e.id)
          : kind === "skip"
            ? await api.skipExecution(e.id)
            : await api.resumeExecution(e.id);
      exec.set(next);
      setTab("stream");
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(null);
    }
  };

  const inspects = e.purpose?.kind === "inspect" ? e.purpose.of : null;
  const canInspect =
    !running && !inspects && e.stream === "terminal" && !!e.native_session_id && !!e.session && !e.has_terminal;
  const inspect = async () => {
    setBusy("inspect");
    setError(null);
    try {
      const view = await api.inspectExecution(e.id);
      nav.open(`exec:${view.id}`);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(null);
    }
  };

  const openGate = (g: typeof gate) => {
    if (!e.session) return;
    nav.open(`session:${e.session}`, g ? { anchor: `gate-${g.id}` } : {});
  };

  const notice =
    gate || pending ? (
      <PermissionNotice
        stream={e.stream}
        gate={gate}
        pending={pending}
        summarize={summarize}
        onAnswered={exec.reload}
        onOpenGate={openGate}
      />
    ) : null;

  const liveDot = running ? <StatusDot tone="accent" pulse size={6} /> : undefined;
  const rows = e.stream === "terminal" ? hookRows(act.entries, summarize) : [];
  const tabs: TabItem[] =
    e.stream === "terminal"
      ? [
          { id: "stream", label: "Terminal", icon: "square-terminal", dot: liveDot },
          { id: "tools", label: "Tool calls", icon: "shield-check", count: rows.length },
        ]
      : [{ id: "stream", label: "Activity", icon: "activity", dot: liveDot }];

  return (
    <div className="ex-page">
      <div className="ex-head">
        <div className="ex-head__main">
          <h1 className="ex-title">
            {humanize(e.agent)} · {e.run_label} <span className="ex-title__sub">in {e.project}</span>
          </h1>
          <div className="ex-chips">
            <StatusChip kind="execution" status={st} />
            {stageLabel && <Chip tone="accent">{stageLabel}</Chip>}
            <Chip mono icon={e.stream === "terminal" ? "square-terminal" : "activity"}>
              {e.executor}
            </Chip>
            <span className="ex-mono ex-muted">
              {e.model} · {duration}
            </span>
          </div>
        </div>
        <div className="ex-head__actions">
          {siblings.length > 1 && (
            <Select
              size="sm"
              aria-label="Run"
              value={e.id}
              title={`${siblings.length} ${humanize(e.agent)} runs in ${e.project}`}
              onChange={(ev) => ev.target.value !== e.id && nav.open(`exec:${ev.target.value}`)}
              options={siblings}
            />
          )}
          {running && e.can_skip && (
            <Button
              size="sm"
              icon="corner-down-right"
              disabled={busy !== null}
              onClick={() => void run("skip")}
              title="Stop this task and let the session continue without its result"
            >
              {busy === "skip" ? "Skipping…" : "Skip"}
            </Button>
          )}
          {running ? (
            <Button
              variant="danger"
              size="sm"
              icon="square"
              disabled={busy !== null}
              onClick={() => void run("cancel")}
            >
              {busy === "cancel" ? "Cancelling…" : "Cancel"}
            </Button>
          ) : (
            e.can_resume &&
            !inspects &&
            !e.has_terminal && (
              <Button
                size="sm"
                icon="rotate-ccw"
                disabled={busy !== null}
                onClick={() => void run("resume")}
                title="Reopen the harness session with its own resume command"
              >
                {busy === "resume" ? "Resuming…" : "Resume"}
              </Button>
            )
          )}
          {canInspect && (
            <Button
              size="sm"
              icon="eye"
              disabled={busy !== null}
              onClick={() => void inspect()}
              title="Reopen the harness session to read its work and ask about it. Every tool call is refused."
            >
              {busy === "inspect" ? "Opening…" : "Open the session"}
            </Button>
          )}
        </div>
      </div>

      <UsageStrip usage={usage} duration={duration} />
      {(e.can_steer || e.queued_steer) && <SteerBox e={{ ...e, status: st }} onView={exec.set} />}
      <SpawnSection
        e={e}
        nativeSessionId={act.nativeSessionId ?? e.native_session_id}
        onOpenReport={(p) => nav.open(`artifact:${p}`)}
      />

      {inspects && (
        <Banner
          tone="info"
          title="Read-only session"
          actions={
            <Button size="sm" onClick={() => nav.open(`exec:${inspects}`)}>
              Open the original run
            </Button>
          }
        >
          This reopens a run that has ended so you can scroll its work and ask the agent about it in the terminal. Ostra
          refuses every tool call here, because the work is done. Leave the CLI or cancel to close it.
        </Banner>
      )}
      {error && <Banner tone="bad">{error}</Banner>}
      {e.error && (
        <Banner tone="bad" title="The execution ended with an error">
          {e.error}
        </Banner>
      )}
      {activityError && <Banner tone="bad">Ostra could not load the activity: {activityError.message}</Banner>}

      <Tabs value={tab} onChange={(t) => setTab(t as "stream" | "tools")} tabs={tabs} label="Execution views" />

      {tab === "stream" && e.stream === "activity" && (
        <div className="ex-stack">
          {notice}
          <ActivityStream entries={act.entries} live={running} summarize={summarize} />
        </div>
      )}
      {tab === "stream" && e.stream === "terminal" && (
        <TerminalStream
          execution={e.id}
          mode={mode}
          title={`${e.executor} · ${e.model}`}
          project={e.project}
          canResume={e.can_resume}
          resuming={busy === "resume"}
          onResume={() => void run("resume")}
          notice={notice}
        />
      )}
      {tab === "tools" && e.stream === "terminal" && <HookLog rows={rows} />}
    </div>
  );
}
