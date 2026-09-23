import { Chip, type Tone } from "./Chip";

export type StatusKind = "session" | "execution" | "stage" | "phase" | "init";

export interface StatusChipProps {
  kind?: StatusKind;
  /** The backend enum value, e.g. "waiting", "stuck", "implementing", "not_initialized". */
  status: string;
}

const TONES: Record<StatusKind, Record<string, Tone>> = {
  session: { running: "accent", waiting: "warn", completed: "ok", failed: "bad", stalled: "bad" },
  execution: { running: "accent", ok: "ok", stuck: "warn", handoff: "info", error: "bad", denied: "bad", interrupted: "warn", cancelled: "neutral" },
  stage: { pending: "neutral", running: "accent", waiting: "warn", done: "ok", skipped: "neutral", failed: "bad", blocked: "bad" },
  phase: { queued: "neutral", implementing: "accent", reviewing: "accent", passed: "ok", blocked: "bad", removed: "bad" },
  init: { initialized: "ok", not_initialized: "warn", initializing: "accent", missing: "bad" },
};

const LABELS: Partial<Record<StatusKind, Record<string, string>>> = {
  session: { waiting: "Waiting for you" },
  init: { not_initialized: "Not initialized", missing: "Folder missing" },
};

const human = (s: string) => s.charAt(0).toUpperCase() + s.slice(1).replace(/_/g, " ");

/** Tone for a backend status value; exported so rows and dots can match the chip. */
export function statusTone(kind: StatusKind, status: string): Tone {
  return TONES[kind][status] ?? "neutral";
}

/** Status chip for a session, execution, stage, phase or project init state. */
export function StatusChip({ kind = "session", status }: StatusChipProps) {
  const label = LABELS[kind]?.[status] ?? human(status);
  return <Chip tone={statusTone(kind, status)}>{label}</Chip>;
}
