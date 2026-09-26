import { activateOnKey } from "../../keys";
import { Chip } from "../feedback/Chip";
import { StatusChip } from "../feedback/StatusChip";

export interface PhaseNodeProps {
  id: string | number;
  title: string;
  project?: string;
  complexity?: "low" | "medium" | "high";
  testPolicy?: "Required" | "Optional" | "Skip";
  deliverable?: string;
  status?: "queued" | "implementing" | "reviewing" | "passed" | "blocked" | "removed";
  reviewPass?: number;
  securityBlock?: boolean;
  dependsOn?: (string | number)[];
  selected?: boolean;
  onClick?: () => void;
}

/** One phase in the build-lane DAG. */
export function PhaseNode({
  id,
  title,
  project,
  complexity,
  testPolicy,
  deliverable,
  status = "queued",
  reviewPass,
  securityBlock,
  dependsOn,
  onClick,
  selected,
}: PhaseNodeProps) {
  return (
    <div
      className={`os-phase os-phase--${status}`}
      onClick={onClick}
      role={onClick ? "button" : undefined}
      tabIndex={onClick ? 0 : undefined}
      aria-pressed={onClick ? !!selected : undefined}
      onKeyDown={onClick ? activateOnKey(onClick) : undefined}
      style={selected ? { boxShadow: "0 0 0 2px var(--accent)" } : undefined}
    >
      <div className="os-phase__head">
        <span className="os-phase__id">{id}</span>
        <span className="os-phase__title">{title}</span>
      </div>
      <div className="os-phase__chips">
        {project && <Chip mono>{project}</Chip>}
        {complexity && <Chip outline>{complexity}</Chip>}
        {testPolicy && (
          <Chip tone={testPolicy === "Skip" ? "neutral" : "accent"} outline={testPolicy === "Skip"}>
            tests {testPolicy}
          </Chip>
        )}
        {deliverable && <Chip>{deliverable}</Chip>}
      </div>
      <div className="os-phase__foot">
        <StatusChip kind="phase" status={status} />
        {reviewPass != null && reviewPass > 0 && <span>review pass {reviewPass}</span>}
        {securityBlock && (
          <Chip tone="bad" icon="shield-alert">
            security block
          </Chip>
        )}
        {dependsOn && dependsOn.length > 0 && (
          <span style={{ marginLeft: "auto", fontFamily: "var(--font-mono)" }}>← {dependsOn.join(", ")}</span>
        )}
      </div>
    </div>
  );
}
