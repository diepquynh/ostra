import { lazy, Suspense, useEffect, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router";
import { api } from "../../api";
import type { ExecutionStatus } from "../../api/types";
import { useCrumbs } from "../../components/Layout";
import { Chip, ErrorBox, ExecStatusChip, Loading } from "../../components/Status";
import { STAGES } from "../../content/stages";
import { applyDelta, emptyActivity, foldActivity, type ActivityState } from "../../lib/events";
import { elapsed, formatCost, formatDuration, formatTokens, humanize } from "../../lib/format";
import { useAsync, useChannel } from "../../lib/hooks";
import { ActivityView } from "./ActivityView";

const TerminalView = lazy(() => import("./TerminalView"));

export function ExecutionPage() {
  const { ws = "", id = "" } = useParams();
  const [params, setParams] = useSearchParams();
  const exec = useAsync(() => api.execution(id), [id]);
  const [activity, setActivity] = useState<ActivityState>(emptyActivity);
  const [status, setStatus] = useState<ExecutionStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const tab = params.get("tab") ?? "activity";

  useCrumbs([
    { label: "Workspace", to: `/w/${ws}` },
    ...(exec.data?.session ? [{ label: "Session", to: `/w/${ws}/s/${exec.data.session}` }] : []),
    { label: exec.data ? humanize(exec.data.agent) : "Execution" },
  ]);

  useEffect(() => {
    let alive = true;
    setActivity(emptyActivity());
    api
      .activity(id)
      .then((items) => alive && setActivity((s) => foldActivity(items, s)))
      .catch((e: Error) => alive && setError(e.message));
    return () => {
      alive = false;
    };
  }, [id]);

  useChannel(`execution:${id}`, (m) => {
    if (m.type === "execution_delta") setActivity((s) => applyDelta(s, m.seq, m.delta));
    if (m.type === "execution_status") {
      setStatus(m.status);
      exec.reload();
    }
  });

  if (exec.error) return <ErrorBox error={exec.error} onRetry={exec.reload} />;
  if (!exec.data) return <Loading />;
  const e = exec.data;
  const st = status ?? e.status;
  const isHarness = e.executor.startsWith("harness:");
  const usage = activity.usage ?? e.usage;

  const act = async (fn: () => Promise<unknown>) => {
    setError(null);
    try {
      await fn();
      exec.reload();
    } catch (err) {
      setError((err as Error).message);
    }
  };

  return (
    <div className="page">
      <div className="row between">
        <div>
          <h1>
            {humanize(e.agent)} <span className="muted small">in {e.project}</span>
          </h1>
          <div className="row small">
            <ExecStatusChip status={st} />
            {e.stage && <Chip tone="accent">{STAGES[e.stage].label}</Chip>}
            <Chip>{e.executor}</Chip>
            <span className="mono muted">{e.model}</span>
            <span className="muted">{elapsed(e.started_at, e.ended_at)}</span>
          </div>
        </div>
        <div className="row">
          {st === "running" && (
            <button className="danger" onClick={() => void act(() => api.cancelExecution(id))}>
              Cancel
            </button>
          )}
          {e.can_resume && st !== "running" && (
            <button onClick={() => void act(() => api.resumeExecution(id))} title="Reopen the harness session in a new terminal">
              Resume
            </button>
          )}
        </div>
      </div>
      {error && <div className="banner bad mt">{error}</div>}
      {e.error && <div className="banner bad mt">{e.error}</div>}

      <div className="row small mt">
        <span>
          Tokens in {formatTokens(usage.input_tokens)}, out {formatTokens(usage.output_tokens)}
        </span>
        <span>cache reads {formatTokens(usage.cache_read_tokens)}</span>
        <span>{usage.tool_calls} tool calls</span>
        {usage.tool_calls > 0 && <span>{formatTokens(usage.cache_read_tokens / usage.tool_calls)} cache reads per call</span>}
        {usage.build_ms > 0 && <span>build time {formatDuration(usage.build_ms)}</span>}
        <span>{formatCost(usage.cost_usd)}</span>
        {(activity.nativeSessionId ?? e.native_session_id) && (
          <span className="mono muted">session {activity.nativeSessionId ?? e.native_session_id}</span>
        )}
      </div>

      <nav className="tabs mt">
        <button className={tab === "activity" ? "active" : ""} onClick={() => setParams({ tab: "activity" })}>
          Activity
        </button>
        {isHarness && (
          <button className={tab === "terminal" ? "active" : ""} onClick={() => setParams({ tab: "terminal" })}>
            Terminal
          </button>
        )}
        <button className={tab === "spawn" ? "active" : ""} onClick={() => setParams({ tab: "spawn" })}>
          Spawn parameters
        </button>
      </nav>

      {tab === "activity" && <ActivityView entries={activity.entries} />}
      {tab === "terminal" && isHarness && (
        <Suspense fallback={<Loading label="Loading terminal" />}>
          {!e.has_terminal && st !== "running" && (
            <div className="banner info">
              This harness session has ended. Resume reopens it with the harness's own resume command.
            </div>
          )}
          <TerminalView execution={id} readOnly={st !== "running" && !e.has_terminal} />
        </Suspense>
      )}
      {tab === "spawn" && (
        <div className="card">
          <p className="small muted">
            The exact parameter block the agent received, followed by the project brief. Every agent gets a self-contained prompt; none
            of them sees another agent's conversation.
          </p>
          <pre>{e.spawn_block}</pre>
          {e.report_path && (
            <p>
              Report:{" "}
              <Link to={`/w/${ws}/artifact?path=${encodeURIComponent(e.report_path)}`} className="mono">
                {e.report_path}
              </Link>
            </p>
          )}
        </div>
      )}
    </div>
  );
}
