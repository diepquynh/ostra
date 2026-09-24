import { useState } from "react";
import type { ExecutionView, Lane, SessionDetail } from "../../api/types";
import { Button, Panel, StageRow, StatusChip } from "../../design";
import { LANES, STAGES } from "../../content/stages";
import { humanize } from "../../lib/format";
import { useNav } from "../../lib/nav";
import { stageKey, stageMeta } from "./board";

const label = { fontSize: "var(--text-sm)", color: "var(--text-muted)" } as const;

/** One lane's stages with its "why this step exists" note; selecting a stage shows what it produces and protects against. */
export function LanePanel({ detail, lane, onGate }: { detail: SessionDetail; lane: Lane; onGate: (id: string) => void }) {
  const nav = useNav();
  const [selected, setSelected] = useState<string | null>(null);
  const execs = new Map(detail.executions.map((x) => [x.id, x]));
  const stages = detail.stages.map((s, i) => ({ s, key: stageKey(s, i) })).filter(({ s }) => s.lane === lane);
  const picked = stages.find((x) => x.key === selected)?.s ?? null;
  return (
    <Panel title={LANES[lane].title} subtitle={`${stages.length} stage${stages.length === 1 ? "" : "s"}`} icon="list-tree">
      <div style={{ fontSize: "var(--text-sm)", color: "var(--text-secondary)", lineHeight: 1.5, marginBottom: 8 }}>{LANES[lane].why}</div>
      {stages.length === 0 ? (
        <div style={label}>No stage in this lane has started. It runs when the pipeline reaches it, or the completion report says why it was skipped.</div>
      ) : (
        <div style={{ margin: "0 -6px" }}>
          {stages.map(({ s, key }) => (
            <StageRow key={key} label={s.label} status={s.status} meta={stageMeta(s, execs)} selected={selected === key} onClick={() => setSelected(selected === key ? null : key)} />
          ))}
        </div>
      )}
      {picked && (
        <div className="os-rise" style={{ marginTop: 10, padding: "10px 12px", border: "1px solid var(--border-subtle)", borderRadius: "var(--radius-md)", display: "flex", flexDirection: "column", gap: 6 }}>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <strong style={{ flex: 1 }}>{picked.label}</strong>
            <StatusChip kind="stage" status={picked.status} />
          </div>
          <div>
            <span style={label}>Produces: </span>
            {STAGES[picked.stage].produces}
          </div>
          <div>
            <span style={label}>Protects against: </span>
            {STAGES[picked.stage].protects}
          </div>
          {picked.detail && <div style={label}>{picked.detail}</div>}
          {(picked.executions.length > 0 || picked.gate) && (
            <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
              {picked.executions.map((id) => execs.get(id)).filter((x): x is ExecutionView => !!x).map((x) => (
                <Button key={x.id} size="sm" icon={x.stream === "activity" ? "activity" : "square-terminal"} onClick={() => nav.open(`exec:${x.id}`)} title={x.summary ?? undefined}>
                  {humanize(x.agent)} · {x.run_label}
                </Button>
              ))}
              {picked.gate && (
                <Button size="sm" icon="hand" onClick={() => onGate(picked.gate!)}>
                  Go to the gate
                </Button>
              )}
            </div>
          )}
        </div>
      )}
    </Panel>
  );
}
