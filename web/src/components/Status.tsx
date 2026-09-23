import type { ReactNode } from "react";
import type { ExecutionStatus, InitStatus, PhaseStatus, SessionStatus, StageStatus } from "../api/types";
import { humanize } from "../lib/format";

type Tone = "ok" | "warn" | "bad" | "info" | "accent" | "";

export function Chip({ tone = "", children, title }: { tone?: Tone; children: ReactNode; title?: string }) {
  return (
    <span className={`chip ${tone}`} title={title}>
      {children}
    </span>
  );
}

const SESSION_TONE: Record<SessionStatus, Tone> = {
  running: "accent",
  waiting: "warn",
  completed: "ok",
  failed: "bad",
  stalled: "bad",
};
const SESSION_LABEL: Record<SessionStatus, string> = {
  running: "Running",
  waiting: "Waiting for you",
  completed: "Completed",
  failed: "Failed",
  stalled: "Stalled",
};

export function SessionStatusChip({ status }: { status: SessionStatus }) {
  return <Chip tone={SESSION_TONE[status]}>{SESSION_LABEL[status]}</Chip>;
}

const EXEC_TONE: Record<ExecutionStatus, Tone> = {
  running: "accent",
  ok: "ok",
  stuck: "warn",
  handoff: "info",
  error: "bad",
  denied: "bad",
  interrupted: "warn",
  cancelled: "",
};

export function ExecStatusChip({ status }: { status: ExecutionStatus }) {
  return <Chip tone={EXEC_TONE[status]}>{humanize(status)}</Chip>;
}

const STAGE_TONE: Record<StageStatus, Tone> = {
  pending: "",
  running: "accent",
  waiting: "warn",
  done: "ok",
  skipped: "",
  failed: "bad",
  blocked: "bad",
};

export function StageStatusChip({ status }: { status: StageStatus }) {
  return <Chip tone={STAGE_TONE[status]}>{humanize(status)}</Chip>;
}

const PHASE_TONE: Record<PhaseStatus, Tone> = {
  queued: "",
  implementing: "accent",
  reviewing: "accent",
  passed: "ok",
  blocked: "bad",
  removed: "bad",
};

export function PhaseStatusChip({ status }: { status: PhaseStatus }) {
  return <Chip tone={PHASE_TONE[status]}>{humanize(status)}</Chip>;
}

const INIT_TONE: Record<InitStatus, Tone> = {
  initialized: "ok",
  not_initialized: "warn",
  initializing: "accent",
  missing: "bad",
};
const INIT_LABEL: Record<InitStatus, string> = {
  initialized: "Initialized",
  not_initialized: "Not initialized",
  initializing: "Initializing",
  missing: "Folder missing",
};

export function InitStatusChip({ status }: { status: InitStatus }) {
  return <Chip tone={INIT_TONE[status]}>{INIT_LABEL[status]}</Chip>;
}

export function Loading({ label = "Loading" }: { label?: string }) {
  return <div className="empty">{label}…</div>;
}

export function ErrorBox({ error, onRetry }: { error: Error; onRetry?: () => void }) {
  return (
    <div className="banner bad row between">
      <span>{error.message}</span>
      {onRetry && (
        <button className="small" onClick={onRetry}>
          Retry
        </button>
      )}
    </div>
  );
}
