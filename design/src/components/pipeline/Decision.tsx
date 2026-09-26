import type { ReactNode } from "react";
import { Button } from "../core/Button";
import { Chip } from "../feedback/Chip";

export interface DecisionProps {
  /** Judge name: Classify, Sufficiency, Stakes, Route answer, Rescue, Resolve review, YOLO answer. */
  judge: string;
  choice: ReactNode;
  /** Lowercase-first clause that follows "because". */
  reason: string;
  basis?: string;
  at?: string;
  overridden?: boolean;
  /** False once dependent work started; shows "Settled". */
  canOverride?: boolean;
  onOverride?: () => void;
}

/** A judge decision rendered as "Ostra chose X because Y", with override. */
export function Decision({
  judge,
  choice,
  reason,
  basis,
  at,
  overridden,
  canOverride = true,
  onOverride,
}: DecisionProps) {
  return (
    <div className="os-decision">
      <div className="os-decision__head">
        <Chip tone="info">{judge}</Chip>
        {at && <span style={{ font: "var(--text-2xs)/1 var(--font-mono)", color: "var(--text-muted)" }}>{at}</span>}
        {overridden && <Chip tone="warn">Overridden by you</Chip>}
        <span style={{ flex: 1 }} />
        {canOverride ? (
          <Button size="sm" variant="ghost" icon="pencil" onClick={onOverride}>
            Override
          </Button>
        ) : (
          <span
            style={{ fontSize: "var(--text-sm)", color: "var(--text-muted)" }}
            title="Work that depends on this decision has already started"
          >
            Settled
          </span>
        )}
      </div>
      <div className="os-decision__text">
        Ostra chose <strong>{choice}</strong> because {reason}
      </div>
      {basis && <div className="os-decision__basis">Based on: {basis}</div>}
    </div>
  );
}
