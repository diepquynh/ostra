import { useState } from "react";
import { api } from "../../api";
import type { DecisionView, SessionDetail } from "../../api/types";
import { Button, Decision, Dialog, ExecutionGroup, Input, Panel, TreeItem } from "../../design";
import { decisionChoice } from "../../lib/events";
import { humanize } from "../../lib/format";
import { useNav } from "../../lib/nav";
import { executionGroups } from "./board";

const empty = { padding: "10px 12px", fontSize: "var(--text-sm)", color: "var(--text-muted)" } as const;
const hhmm = (iso: string) => new Date(iso).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
const lowerFirst = (s: string) => s.replace(/^[A-Z](?![A-Z])/, (c) => c.toLowerCase());

/** Judge decisions as "Ostra chose X because Y", each with an override until dependent work starts. */
export function DecisionsPanel({ decisions, onChanged }: { decisions: DecisionView[]; onChanged: () => void }) {
  const [editing, setEditing] = useState<DecisionView | null>(null);
  return (
    <Panel title="Decisions Ostra made" icon="scale" bodyFlush>
      {decisions.length === 0 ? (
        <div style={empty}>No decisions yet. The judges decide classification, research sufficiency, stakes, and rescues as the pipeline reaches them.</div>
      ) : (
        decisions.map((d) => (
          <Decision
            key={d.id}
            judge={humanize(d.judge)}
            choice={decisionChoice(d)}
            reason={lowerFirst(d.reason)}
            basis={d.input_summary}
            at={hhmm(d.at)}
            overridden={d.overridden}
            canOverride={d.can_override}
            onOverride={() => setEditing(d)}
          />
        ))
      )}
      {editing && (
        <OverrideDialog
          decision={editing}
          onClose={() => setEditing(null)}
          onDone={() => {
            setEditing(null);
            onChanged();
          }}
        />
      )}
    </Panel>
  );
}

function OverrideDialog({ decision, onClose, onDone }: { decision: DecisionView; onClose: () => void; onDone: () => void }) {
  const [output, setOutput] = useState(JSON.stringify(decision.output, null, 2));
  const [reason, setReason] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const save = () => {
    let parsed: unknown;
    try {
      parsed = JSON.parse(output);
    } catch {
      return setError("Write the decision as valid JSON, in the same shape as the judge's output.");
    }
    if (!reason.trim()) return setError("Say why, so the record explains the change.");
    setBusy(true);
    api
      .overrideDecision(decision.id, { output: parsed, reason: reason.trim() })
      .then(onDone)
      .catch((e: Error) => setError(e.message))
      .finally(() => setBusy(false));
  };
  return (
    <Dialog
      title={`Override the ${humanize(decision.judge).toLowerCase()} decision`}
      subtitle="Work that depends on it has not started yet"
      onClose={onClose}
      width={620}
      footer={
        <>
          <span style={{ flex: 1 }} />
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" disabled={busy} onClick={save}>
            Save the override
          </Button>
        </>
      }
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
        <p style={{ margin: 0, fontSize: "var(--text-sm)", color: "var(--text-secondary)" }}>
          Ostra chose {decisionChoice(decision)} because {lowerFirst(decision.reason)} Your decision replaces it and is recorded with your reason.
        </p>
        <Input multiline mono rows={9} label="Your decision" value={output} onChange={(e) => setOutput(e.target.value)} />
        <Input label="Why" value={reason} onChange={(e) => setReason(e.target.value)} error={error} />
      </div>
    </Dialog>
  );
}

/** Spec, plan, reports, and ledgers the session wrote. Rows open in the preview tab. */
export function ArtifactsPanel({ detail }: { detail: SessionDetail }) {
  const nav = useNav();
  return (
    <Panel title="Artifacts" subtitle={detail.artifacts.length || undefined} icon="files" bodyFlush>
      {detail.artifacts.length === 0 ? (
        <div style={empty}>No artifacts yet. The research documents appear here first.</div>
      ) : (
        <div style={{ padding: 4 }} role="tree" aria-label="Artifacts">
          {detail.artifacts.map((a) => (
            <TreeItem
              key={a.path}
              label={a.label}
              icon={a.kind === "ledger" ? "file-diff" : a.kind === "plan" || a.kind === "phase" ? "list-checks" : "file-text"}
              meta={a.project ?? a.kind}
              title={a.path}
              onClick={() => nav.open(`artifact:${a.path}`, { preview: true })}
            />
          ))}
        </div>
      )}
    </Panel>
  );
}

/** Executions grouped by agent and project, oldest group first. Rows open in the preview tab. */
export function ExecutionsPanel({ detail }: { detail: SessionDetail }) {
  const nav = useNav();
  const groups = executionGroups(detail);
  return (
    <Panel title="Executions" subtitle={`${detail.executions.length} run${detail.executions.length === 1 ? "" : "s"}`} icon="square-terminal" bodyFlush>
      {groups.length === 0 ? (
        <div style={empty}>No executions yet.</div>
      ) : (
        groups.map((g) => <ExecutionGroup key={g.key} agent={g.agent} project={g.project} runs={g.runs} onOpen={(r) => nav.open(`exec:${r.id}`, { preview: true })} />)
      )}
    </Panel>
  );
}
