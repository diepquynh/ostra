import type { ReactNode } from "react";
import { cx } from "../../cx";
import { activateOnKey } from "../../keys";

export type StageStatus = "pending" | "running" | "waiting" | "done" | "skipped" | "failed" | "blocked";

export interface StageRowProps {
  label: string;
  status?: StageStatus;
  meta?: ReactNode;
  selected?: boolean;
  onClick?: () => void;
}

const DOT: Record<StageStatus, [string | null, boolean?]> = {
  done: ["ok"],
  running: ["accent", true],
  waiting: ["warn"],
  failed: ["bad"],
  blocked: ["bad"],
  skipped: [null],
  pending: [null],
};

/** One stage row inside a lane's detail list. */
export function StageRow({ label, status = "pending", meta, selected, onClick }: StageRowProps) {
  const [tone, pulse] = DOT[status] ?? [null];
  return (
    <div
      className={cx("os-stage", selected && "os-stage--selected", status === "skipped" && "os-stage--skipped")}
      onClick={onClick}
      role={onClick ? "button" : undefined}
      tabIndex={onClick ? 0 : undefined}
      aria-pressed={onClick ? !!selected : undefined}
      onKeyDown={onClick ? activateOnKey(onClick) : undefined}
    >
      <span className={cx("os-dot", tone ? `os-dot--${tone}` : "os-dot--hollow", pulse && "os-dot--pulse")} />
      <span className="os-stage__label">{label}</span>
      {meta && <span className="os-stage__meta">{meta}</span>}
    </div>
  );
}
