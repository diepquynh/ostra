import { useEffect, useRef, useState } from "react";
import { api, socket } from "../../api";
import type { ExecutionDelta, ExecutionStatus } from "../../api/types";
import { type ActivityState, applyDelta, emptyActivity, foldActivity } from "../../lib/events";
import { useAsync, useChannel, useThrottled } from "../../lib/hooks";

/**
 * The activity of one execution: the REST snapshot, then every `execution:<id>` delta. Deltas that arrive before
 * the snapshot are held and applied after it, because `applyDelta` drops anything at or below the last seq it saw.
 * After a reconnect only the items past the last seq are fetched.
 */
export function useActivity(id: string, onStatus: (s: ExecutionStatus) => void) {
  const [state, setState] = useState<ActivityState>(emptyActivity);
  const [error, setError] = useState<Error | null>(null);
  const loaded = useRef(false);
  const held = useRef<{ seq: number; delta: ExecutionDelta }[]>([]);
  const lastSeq = useRef(0);
  lastSeq.current = state.lastSeq;

  useEffect(() => {
    let alive = true;
    loaded.current = false;
    held.current = [];
    setState(emptyActivity());
    setError(null);
    api.activity(id).then(
      (items) => {
        if (!alive) return;
        loaded.current = true;
        const buffered = held.current;
        held.current = [];
        setState((s) => buffered.reduce((acc, d) => applyDelta(acc, d.seq, d.delta), foldActivity(items, s)));
      },
      (e: Error) => alive && setError(e),
    );
    const off = socket().onReconnect(() => {
      api.activity(id, lastSeq.current).then(
        (items) => alive && setState((s) => foldActivity(items, s)),
        () => {},
      );
    });
    return () => {
      alive = false;
      off();
    };
  }, [id]);

  useChannel(`execution:${id}`, (m) => {
    if (m.type === "execution_delta") {
      if (loaded.current) setState((s) => applyDelta(s, m.seq, m.delta));
      else held.current.push({ seq: m.seq, delta: m.delta });
    }
    if (m.type === "execution_status") onStatus(m.status);
  });

  return { activity: state, error };
}

/**
 * The execution (with its `pending_gate`), its session detail for the sibling runs, and live status. A gate opening
 * or closing reloads the execution, because `pending_gate` comes from the session's fold.
 */
export function useExecution(id: string) {
  const exec = useAsync(() => api.execution(id), [id]);
  const [status, setStatus] = useState<ExecutionStatus | null>(null);
  const { activity, error: activityError } = useActivity(id, (s) => {
    setStatus(s);
    exec.reload();
  });
  const sessionId = exec.data?.session ?? null;
  const session = useAsync(() => (sessionId ? api.session(sessionId) : Promise.resolve(null)), [sessionId]);
  const reloadSession = useThrottled(session.reload, 800);
  const reloadExec = useThrottled(exec.reload, 400);
  useChannel(sessionId ? `session:${sessionId}` : null, (m) => {
    if (m.type !== "session_event") return;
    const t = m.event.type;
    if (t === "gate_opened" || t === "gate_answered") reloadExec();
    if (t === "execution_started" || t === "execution_resumed" || t === "execution_finished") reloadSession();
  });
  useEffect(() => setStatus(null), [exec.data?.status]);
  return { exec, status: status ?? exec.data?.status ?? null, activity, activityError, session };
}

/** A clock that ticks once a second while `on`, for the elapsed time of a running execution. */
export function useNow(on: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [on]);
  return now;
}
