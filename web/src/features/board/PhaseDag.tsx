import type { PhaseView } from "../../api/types";
import { Chip, PhaseStatusChip } from "../../components/Status";
import { phaseLayers } from "../../lib/events";

/** The plan's phases as dependency layers: each column depends only on columns to its left. */
export function PhaseDag({ phases, onOpen }: { phases: PhaseView[]; onOpen?: (p: PhaseView) => void }) {
  if (phases.length === 0) return <div className="muted small">No phases yet. The plan stage writes them.</div>;
  const layers = phaseLayers(phases);
  return (
    <div className="dag">
      {layers.map((layer, i) => (
        <div className="dag-layer" key={i}>
          <div className="muted small">{i === 0 ? "No dependencies" : `Step ${i + 1}`}</div>
          {layer.map((p) => (
            <div
              key={p.info.id}
              className={`phase ${p.status}`}
              onClick={() => onOpen?.(p)}
              style={onOpen ? { cursor: "pointer" } : undefined}
              title={p.info.file ?? "Inline work with no phase file"}
            >
              <div className="row between">
                <strong>
                  {p.info.id}. {p.info.title}
                </strong>
              </div>
              <div className="row small mt" style={{ marginTop: 4 }}>
                {p.info.deliverable && <Chip>{p.info.deliverable}</Chip>}
                <Chip tone="info">{p.info.project}</Chip>
                <Chip title="Picks the implementer and write-test model tier">{p.info.complexity}</Chip>
                <Chip tone={p.info.test_policy === "Skip" ? "" : "accent"} title={p.info.test_rationale ?? "Covered if you ask for tests"}>
                  tests {p.info.test_policy}
                </Chip>
              </div>
              <div className="row small" style={{ marginTop: 4 }}>
                <PhaseStatusChip status={p.status} />
                {p.review_iterations > 0 && <span className="muted">review pass {p.review_iterations}</span>}
                {p.security_block && <Chip tone="bad">security block</Chip>}
              </div>
              {p.info.depends_on && p.info.depends_on.length > 0 && (
                <div className="small muted">depends on {p.info.depends_on.join(", ")}</div>
              )}
              {p.info.depends_on === null && <div className="small muted">dependencies unclear, queued behind earlier phases</div>}
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}
