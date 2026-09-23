import { useState } from "react";
import { api } from "../../api";
import type { ExecStream, PendingGate, PermissionAnswer, ToolCall } from "../../api/types";
import { Banner, Button } from "../../design";
import type { ToolEntry } from "../../lib/events";
import { inlineCode } from "./inline";

export interface PermissionNoticeProps {
  stream: ExecStream;
  /** `ExecutionView.pending_gate`: the open gate that names this execution. */
  gate: PendingGate | null;
  /** The call the activity stream shows waiting on an ask. */
  pending: ToolEntry | null;
  summarize: (call: ToolCall) => string;
  onAnswered: () => void;
  onOpenGate: (gate: PendingGate | null) => void;
}

/**
 * A paused permission ask, shown above the stream with Allow once and Deny for its gate. Any other open gate that
 * names the execution (stuck, failed, harness failure) shows its title and a link to the gate.
 */
export function PermissionNotice({ stream, gate, pending, summarize, onAnswered, onOpenGate }: PermissionNoticeProps) {
  const [busy, setBusy] = useState<PermissionAnswer | null>(null);
  const [error, setError] = useState<string | null>(null);
  const permission = gate?.kind === "permission" ? gate : null;
  const call = pending?.call ?? null;
  if (gate && !permission) {
    return (
      <Banner
        tone="warn"
        title="A gate waits on this run"
        actions={
          <Button size="sm" variant="ghost" iconRight="arrow-right" onClick={() => onOpenGate(gate)}>
            Open the gate
          </Button>
        }
      >
        {gate.title}. Answer it on the session board.
      </Banner>
    );
  }
  if (!call && !permission) return null;
  const reason = pending?.policy?.decision === "ask" ? pending.policy.reason : null;
  const holder = stream === "terminal" ? "The hook bridge" : "Ostra";

  const answer = async (a: PermissionAnswer) => {
    if (!permission) return;
    setBusy(a);
    setError(null);
    try {
      await api.answerGate(permission.id, { answer: { kind: "permission", answer: a } });
      onAnswered();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
    }
  };

  return (
    <Banner
      tone="warn"
      title={stream === "terminal" ? "The harness is paused on a permission ask" : "The agent is paused on a permission ask"}
      actions={
        <>
          {permission && (
            <>
              <Button size="sm" variant="primary" disabled={busy !== null} onClick={() => void answer("allow-once")}>
                Allow once
              </Button>
              <Button size="sm" disabled={busy !== null} onClick={() => void answer("deny")}>
                Deny
              </Button>
            </>
          )}
          <Button size="sm" variant="ghost" iconRight="arrow-right" onClick={() => onOpenGate(permission)}>
            {permission ? "Open the gate" : "Open the session"}
          </Button>
        </>
      }
    >
      {call ? <code>{summarize(call)}</code> : permission?.title} needs your answer. {reason && <>{inlineCode(reason)} </>}
      {holder} holds the call until you decide.
      {error && <div style={{ color: "var(--bad)", marginTop: 4 }}>{error}</div>}
    </Banner>
  );
}
