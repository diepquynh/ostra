import type { ReactNode } from "react";
import { Icon } from "../core/Icon";
import type { IconName } from "../core/icons";
import { Chip } from "../feedback/Chip";

export type GateKind =
  | "open_questions"
  | "spec_approval"
  | "plan_approval"
  | "fact_check_recurring"
  | "review_cap"
  | "stuck"
  | "phase_blocked"
  | "closing_gate"
  | "permission"
  | "harness_failure"
  | "skill_approval"
  | "execution_failed"
  | "budget_reached";

export interface GateCardProps {
  /** Gate kind; unknown kinds from newer servers fall back to a pause icon. */
  kind: GateKind | (string & {});
  title: string;
  /** Why the pipeline is waiting, in one or two sentences. */
  explanation?: ReactNode;
  answered?: boolean;
  answeredBy?: "user" | "yolo";
  /** Answer summary shown once answered. */
  answer?: ReactNode;
  reason?: string;
  /** Button row at the bottom of an open gate. */
  actions?: ReactNode;
  /** The gate's form body (questions, findings, the tool call JSON). */
  children?: ReactNode;
}

const KIND_ICON: Record<GateKind, IconName> = {
  open_questions: "message-circle-question",
  spec_approval: "file-check",
  plan_approval: "list-checks",
  permission: "shield-alert",
  review_cap: "repeat",
  stuck: "life-buoy",
  phase_blocked: "octagon-x",
  closing_gate: "flag",
  skill_approval: "book-check",
  harness_failure: "plug-zap",
  execution_failed: "circle-x",
  budget_reached: "coins",
  fact_check_recurring: "refresh-ccw",
};

/** Shell for every gate: header, explanation, then either the answer form (children) or the recorded answer. */
export function GateCard({ kind, title, explanation, answered, answeredBy, answer, reason, actions, children }: GateCardProps) {
  return (
    <section className={`os-gate ${answered ? "" : "os-gate--open"}`} aria-label={title}>
      <header className="os-gate__head">
        <Icon name={KIND_ICON[kind as GateKind] ?? "circle-pause"} size={16} style={{ color: answered ? "var(--text-muted)" : "var(--warn)" }} />
        <span className="os-gate__title">{title}</span>
        {answered ? (
          <Chip tone={answeredBy === "yolo" ? "warn" : "ok"}>Answered by {answeredBy === "yolo" ? "Ostra (YOLO)" : "you"}</Chip>
        ) : (
          <Chip tone="warn">Waiting for you</Chip>
        )}
      </header>
      {explanation && <div className="os-gate__explain">{explanation}</div>}
      <div className="os-gate__body">
        {answered ? (
          <>
            <div>{answer}</div>
            {reason && <div style={{ fontSize: "var(--text-sm)", color: "var(--text-muted)" }}>Reason: {reason}</div>}
          </>
        ) : (
          <>
            {children}
            {actions && <div className="os-gate__actions">{actions}</div>}
          </>
        )}
      </div>
    </section>
  );
}
