import { type KeyboardEvent, useRef } from "react";
import { cx } from "../../cx";
import { arrowIndex } from "../../focus";
import { Icon } from "../core/Icon";
import type { IconName } from "../core/icons";

export type LaneId =
  | "research"
  | "requirements"
  | "verification"
  | "design"
  | "build"
  | "review"
  | "test"
  | "docs"
  | "done";

export interface LaneState {
  status: "pending" | "current" | "waiting" | "done" | "failed" | "skipped";
  /** One short line under the bar: "3 docs", "Waiting for you", "Phase 2 of 4". */
  detail?: string;
  /** Tooltip: why this lane exists. */
  why?: string;
}

export interface LaneStepperProps {
  lanes: Partial<Record<LaneId, LaneState>>;
  selected?: LaneId;
  onSelect?: (lane: LaneId) => void;
}

export const LANE_ORDER: LaneId[] = [
  "research",
  "requirements",
  "verification",
  "design",
  "build",
  "review",
  "test",
  "docs",
  "done",
];

const TITLES: Record<LaneId, string> = {
  research: "Research",
  requirements: "Requirements",
  verification: "Verification",
  design: "Design",
  build: "Build",
  review: "Review",
  test: "Test",
  docs: "Docs",
  done: "Done",
};

const ICON: Record<LaneState["status"], IconName | null> = {
  done: "check",
  current: null,
  waiting: "hand",
  failed: "x",
  skipped: "minus",
  pending: null,
};

const ICON_COLOR: Partial<Record<LaneState["status"], string>> = {
  done: "var(--ok)",
  waiting: "var(--warn)",
  failed: "var(--bad)",
};

/** The nine SDLC lanes as a horizontal progress strip. Arrow keys, Home and End move between lanes and select them. */
export function LaneStepper({ lanes, selected, onSelect }: LaneStepperProps) {
  const ref = useRef<HTMLDivElement>(null);
  const focusIndex = selected ? LANE_ORDER.indexOf(selected) : 0;

  const onKeyDown = (e: KeyboardEvent, i: number) => {
    const next = arrowIndex(e.key, i, LANE_ORDER.length, "horizontal");
    if (next === null) return;
    e.preventDefault();
    ref.current?.querySelectorAll<HTMLElement>('[role="tab"]')[next]?.focus();
    onSelect?.(LANE_ORDER[next]);
  };

  return (
    <div ref={ref} className="os-lanes" role="tablist" aria-label="Pipeline lanes">
      {LANE_ORDER.map((id, i) => {
        const l = lanes[id] ?? { status: "pending" as const };
        const cls = cx("os-lane", `os-lane--${l.status}`, selected === id && "os-lane--selected");
        const ic = ICON[l.status];
        return (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={selected === id}
            tabIndex={i === focusIndex ? 0 : -1}
            className={cls}
            onClick={() => onSelect?.(id)}
            onKeyDown={(e) => onKeyDown(e, i)}
            title={l.why}
          >
            <span className="os-lane__name">
              {l.status === "current" && <span className="os-dot os-dot--accent os-dot--pulse" />}
              {ic && <Icon name={ic} size={11} style={{ color: ICON_COLOR[l.status] ?? "var(--text-muted)" }} />}
              {TITLES[id]}
            </span>
            <span className="os-lane__bar" />
            <span className="os-lane__detail">{l.detail || "\u00a0"}</span>
          </button>
        );
      })}
    </div>
  );
}
