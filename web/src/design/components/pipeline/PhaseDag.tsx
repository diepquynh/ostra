import { PhaseNode, type PhaseNodeProps } from "./PhaseNode";

export interface PhaseDagProps {
  /** Phases grouped by dependency layer; each column depends only on columns to its left. */
  layers: PhaseNodeProps[][];
  onOpen?: (phase: PhaseNodeProps) => void;
  selected?: string | number;
}

/** Columns of PhaseNodes by dependency layer. */
export function PhaseDag({ layers, onOpen, selected }: PhaseDagProps) {
  return (
    <div style={{ display: "flex", gap: 28, overflowX: "auto", padding: "2px 2px 6px" }}>
      {layers.map((layer, i) => (
        <div key={i} style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          <div className="os-section-label" style={{ height: 14 }}>
            {i === 0 ? "No dependencies" : `Step ${i + 1}`}
          </div>
          {layer.map((p) => (
            <PhaseNode key={p.id} {...p} selected={selected === p.id} onClick={onOpen ? () => onOpen(p) : undefined} />
          ))}
        </div>
      ))}
    </div>
  );
}
