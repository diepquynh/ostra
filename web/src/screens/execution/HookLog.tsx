import { Chip, Panel, Table, type Tone } from "../../design";
import { formatDuration } from "../../lib/format";
import { inlineCode } from "./inline";
import type { HookRow } from "./model";

const DECISION: Record<string, { tone: Tone; label: string }> = {
  allow: { tone: "ok", label: "Allowed" },
  deny: { tone: "bad", label: "Denied" },
};

function decisionChip(r: HookRow) {
  if (r.decision === "ask") return <Chip tone="warn">{r.done ? "Asked you" : "Asking you"}</Chip>;
  if (r.decision) return <Chip tone={DECISION[r.decision].tone}>{DECISION[r.decision].label}</Chip>;
  return <Chip>No decision</Chip>;
}

/** Every tool call the hook bridge reported for a harness run, with the policy decision and the rule. */
export function HookLog({ rows }: { rows: HookRow[] }) {
  return (
    <Panel bodyFlush title="Tool calls seen by the hook bridge" subtitle={rows.length}>
      <Table<HookRow>
        dense
        rowKey="id"
        empty="No tool calls yet. Each call the harness makes appears here with the decision Ostra's policy made."
        columns={[
          { key: "tool", label: "Tool", width: 110, render: (r) => <span className="ex-mono-strong">{r.tool}</span> },
          {
            key: "summary",
            label: "Input",
            render: (r) => (
              <div className="ex-hook-input">
                <span className="ex-mono">{r.summary}</span>
                {r.reason && <span className="ex-hook-note">{inlineCode(r.reason)}</span>}
                {r.decision === "deny" && r.advice && <span className="ex-hook-note">What to do instead: {inlineCode(r.advice)}</span>}
              </div>
            ),
          },
          { key: "decision", label: "Decision", width: 100, render: decisionChip },
          {
            key: "rule",
            label: "Rule",
            render: (r) =>
              r.rule ? (
                <span className="ex-hook-rule">
                  <span className="ex-muted">{r.rule.layer}</span> <code>{r.rule.rule}</code>
                </span>
              ) : (
                <span className="ex-muted">default</span>
              ),
          },
          { key: "duration", label: "Time", num: true, width: 70, render: (r) => (r.durationMs !== null ? formatDuration(r.durationMs) : "") },
        ]}
        rows={rows}
      />
    </Panel>
  );
}
