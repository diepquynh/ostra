import { useState } from "react";
import { api } from "../../api";
import type { DecisionView } from "../../api/types";
import { Chip } from "../../components/Status";
import { decisionChoice } from "../../lib/events";
import { formatTime, humanize } from "../../lib/format";

function OverrideForm({ decision, onDone }: { decision: DecisionView; onDone: (d: DecisionView) => void }) {
  const [output, setOutput] = useState(JSON.stringify(decision.output, null, 2));
  const [reason, setReason] = useState("");
  const [error, setError] = useState<string | null>(null);
  const submit = async () => {
    let parsed: unknown;
    try {
      parsed = JSON.parse(output);
    } catch {
      setError("The decision must be valid JSON.");
      return;
    }
    if (!reason.trim()) {
      setError("Say why, so the record explains the change.");
      return;
    }
    try {
      onDone(await api.overrideDecision(decision.id, { output: parsed, reason: reason.trim() }));
    } catch (e) {
      setError((e as Error).message);
    }
  };
  return (
    <div className="stack mt">
      <label className="field">
        <span className="label">Your decision</span>
        <textarea className="mono" rows={8} value={output} onChange={(e) => setOutput(e.target.value)} />
      </label>
      <label className="field">
        <span className="label">Why</span>
        <input value={reason} onChange={(e) => setReason(e.target.value)} />
      </label>
      {error && <div className="form-error">{error}</div>}
      <div>
        <button className="primary" onClick={() => void submit()}>
          Save override
        </button>
      </div>
    </div>
  );
}

/** Judge decisions, each shown as "Ostra chose X because Y" with an Override button. */
export function Decisions({ decisions, onChange }: { decisions: DecisionView[]; onChange: () => void }) {
  const [editing, setEditing] = useState<string | null>(null);
  if (decisions.length === 0) return <div className="muted small">No decisions yet.</div>;
  return (
    <div className="stack">
      {decisions.map((d) => (
        <div className="card" key={d.id}>
          <div className="row between">
            <div>
              <Chip tone="info">{humanize(d.judge)}</Chip> <span className="muted small">{formatTime(d.at)}</span>
              {d.overridden && (
                <>
                  {" "}
                  <Chip tone="warn">Overridden by you</Chip>
                </>
              )}
            </div>
            {d.can_override && editing !== d.id && (
              <button className="small" onClick={() => setEditing(d.id)}>
                Override
              </button>
            )}
            {!d.can_override && <span className="small muted" title="Work that depends on this decision has already started">Settled</span>}
          </div>
          <p style={{ marginTop: 6 }}>
            Ostra chose <strong>{decisionChoice(d)}</strong> because {d.reason.replace(/^[A-Z]/, (c) => c.toLowerCase())}
          </p>
          <div className="small muted">Based on: {d.input_summary}</div>
          {editing === d.id && (
            <OverrideForm
              decision={d}
              onDone={() => {
                setEditing(null);
                onChange();
              }}
            />
          )}
        </div>
      ))}
    </div>
  );
}
