import { useState } from "react";
import { activateOnKey } from "../../keys";
import { Icon } from "../core/Icon";
import { Chip, type Tone } from "../feedback/Chip";
import { Spinner } from "../feedback/Spinner";
import { StatusDot } from "../feedback/StatusDot";

export type ExecutionRunStatus = "running" | "ok" | "stuck" | "handoff" | "error" | "denied" | "interrupted" | "cancelled";

export interface ExecutionRun {
  id: string;
  /** Run label within the group: "Phase 2", "Phase 1 · fix pass", "Spec · pass 2". */
  label: string;
  status: ExecutionRunStatus;
  /** "native" (Activity stream) or "harness:<cli>" (Terminal stream). */
  executor: string;
  cost?: string;
}

export interface ExecutionGroupProps {
  /** Agent display name, e.g. "Implementer". */
  agent: string;
  /** Project key the group is scoped to. */
  project?: string;
  runs: ExecutionRun[];
  /** Defaults to open when any run is running. */
  defaultOpen?: boolean;
  selected?: string;
  onOpen?: (run: ExecutionRun) => void;
}

const DOT: Record<ExecutionRunStatus, Tone> = { ok: "ok", running: "accent", stuck: "warn", interrupted: "warn", handoff: "info", error: "bad", denied: "bad", cancelled: "neutral" };

function RunIcon({ status }: { status: ExecutionRunStatus }) {
  return status === "running" ? <Spinner size={10} style={{ color: "var(--accent)", margin: "0 1px" }} /> : <StatusDot tone={DOT[status] ?? "neutral"} size={6} />;
}

/** Executions of one agent in one project, collapsible. A group of one renders as a plain row. */
export function ExecutionGroup({ agent, project, runs, defaultOpen, selected, onOpen }: ExecutionGroupProps) {
  const running = runs.some((r) => r.status === "running");
  const [open, setOpen] = useState(defaultOpen ?? running);
  const total = runs.reduce((s, r) => s + (parseFloat(String(r.cost ?? "0").replace("$", "")) || 0), 0);
  const execs = [...new Set(runs.map((r) => r.executor))];
  const openRun = (r: ExecutionRun) => onOpen?.(r);

  if (runs.length === 1) {
    const r = runs[0];
    return (
      <div className="os-xgroup">
        <div
          className={`os-xgroup__head ${selected === r.id ? "os-xrun--selected" : ""}`}
          role="button"
          tabIndex={0}
          aria-current={selected === r.id ? "true" : undefined}
          onClick={() => openRun(r)}
          onKeyDown={activateOnKey(() => openRun(r))}
        >
          <span style={{ width: 14 }} />
          <RunIcon status={r.status} />
          <span className="os-xgroup__agent">{agent}</span>
          {project && (
            <Chip mono outline>
              {project}
            </Chip>
          )}
          <span className="os-xrun__label" style={{ color: "var(--text-muted)" }}>
            {r.label}
          </span>
          <Icon name={r.executor === "native" ? "activity" : "square-terminal"} size={12} style={{ color: "var(--text-muted)" }} title={r.executor} />
          <span className="os-xgroup__cost">{r.cost}</span>
        </div>
      </div>
    );
  }

  const toggle = () => setOpen((o) => !o);
  return (
    <div className="os-xgroup">
      <div className="os-xgroup__head" role="button" tabIndex={0} aria-expanded={open} onClick={toggle} onKeyDown={activateOnKey(toggle)}>
        <Icon name="chevron-right" size={12} style={{ color: "var(--text-muted)", width: 14, transform: open ? "rotate(90deg)" : "none", transition: "transform var(--dur-fast)" }} />
        {running ? <Spinner size={10} style={{ color: "var(--accent)", margin: "0 1px" }} /> : <StatusDot tone="ok" size={6} />}
        <span className="os-xgroup__agent">{agent}</span>
        {project && (
          <Chip mono outline>
            {project}
          </Chip>
        )}
        <span className="os-xgroup__count">×{runs.length}</span>
        <span style={{ flex: 1 }} />
        {execs.map((e) => (
          <Chip key={e} mono>
            {e}
          </Chip>
        ))}
        <span className="os-xgroup__cost">${total.toFixed(2)}</span>
      </div>
      {open &&
        runs.map((r) => (
          <div
            key={r.id}
            className={`os-xrun ${selected === r.id ? "os-xrun--selected" : ""}`}
            role="button"
            tabIndex={0}
            aria-current={selected === r.id ? "true" : undefined}
            onClick={() => openRun(r)}
            onKeyDown={activateOnKey(() => openRun(r))}
          >
            <RunIcon status={r.status} />
            <span className="os-xrun__label">{r.label}</span>
            <Icon
              name={r.executor === "native" ? "activity" : "square-terminal"}
              size={12}
              style={{ color: "var(--text-muted)" }}
              title={r.executor === "native" ? "Activity stream" : "Terminal stream"}
            />
            <span className="os-xgroup__cost">{r.cost}</span>
          </div>
        ))}
    </div>
  );
}
