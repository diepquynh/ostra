import type { ActivityEntry, ToolEntry } from "../../lib/events";
import { denialAdvice, toolSummary } from "../../lib/events";
import { Chip } from "../../components/Status";
import { formatDuration, truncate } from "../../lib/format";

function EditDiff({ input }: { input: Record<string, unknown> }) {
  const oldS = typeof input.old_string === "string" ? input.old_string : "";
  const newS = typeof input.new_string === "string" ? input.new_string : "";
  return (
    <pre>
      {oldS.split("\n").map((l, i) => (
        <div key={`o${i}`} style={{ color: "var(--bad)" }}>
          - {l}
        </div>
      ))}
      {newS.split("\n").map((l, i) => (
        <div key={`n${i}`} style={{ color: "var(--ok)" }}>
          + {l}
        </div>
      ))}
    </pre>
  );
}

function ToolInput({ entry }: { entry: ToolEntry }) {
  const input = (entry.call.input ?? {}) as Record<string, unknown>;
  if (entry.call.tool === "Edit") return <EditDiff input={input} />;
  if (entry.call.tool === "Bash" && typeof input.command === "string") return <pre>$ {input.command}</pre>;
  if (entry.call.tool === "Write" && typeof input.content === "string")
    return (
      <pre>
        {truncate(input.content, 4000)}
      </pre>
    );
  return <pre>{JSON.stringify(entry.call.input, null, 2)}</pre>;
}

function Tool({ entry }: { entry: ToolEntry }) {
  const p = entry.policy;
  const denied = p?.decision === "deny";
  const asked = p?.decision === "ask";
  const advice = p ? denialAdvice(p) : null;
  const output = entry.done ? entry.output : entry.live;
  return (
    <details className={`tool ${denied ? "denied" : asked ? "asked" : ""}`} open={denied || asked}>
      <summary>
        <span className="tool-name">{entry.call.tool}</span>
        <span className="mono small ellipsis" style={{ flex: 1 }}>
          {toolSummary(entry.call)}
        </span>
        {denied && <Chip tone="bad">Denied</Chip>}
        {asked && !entry.done && <Chip tone="warn">Asking you</Chip>}
        {entry.done && entry.isError && !denied && <Chip tone="bad">Error</Chip>}
        {!entry.done && !asked && <Chip tone="accent">Running</Chip>}
        {entry.durationMs !== null && <span className="small muted">{formatDuration(entry.durationMs)}</span>}
      </summary>
      <div className="tool-body">
        {p && p.decision !== "allow" && (
          <div className={`policy ${p.decision}`}>
            <div>
              <strong>{p.decision === "deny" ? "Denied" : "Needs permission"}</strong> by {p.rule.layer} rule{" "}
              <code>{p.rule.rule}</code>
            </div>
            <div>{p.reason}</div>
            {advice && <div className="small">What to do instead: {advice}</div>}
          </div>
        )}
        {p && p.decision === "allow" && p.rule && (
          <div className="small muted">
            Allowed by {p.rule.layer} rule <code>{p.rule.rule}</code>
          </div>
        )}
        <ToolInput entry={entry} />
        {output && (
          <>
            <div className="small muted">{entry.done ? "Output" : "Live output"}</div>
            <pre style={{ maxHeight: 360 }}>{truncate(output, 20000)}</pre>
          </>
        )}
      </div>
    </details>
  );
}

export function ActivityView({ entries }: { entries: ActivityEntry[] }) {
  if (entries.length === 0) return <div className="muted small">No activity yet.</div>;
  return (
    <div className="activity">
      {entries.map((e) => {
        switch (e.kind) {
          case "text":
            return (
              <div key={e.seq} className="act-text">
                {e.text}
              </div>
            );
          case "thinking":
            return (
              <details key={e.seq}>
                <summary className="small muted">Thinking summary</summary>
                <div className="act-thinking">{e.text}</div>
              </details>
            );
          case "status":
            return (
              <div key={e.seq} className="act-status">
                {e.message}
              </div>
            );
          case "tool":
            return <Tool key={e.seq} entry={e} />;
        }
      })}
    </div>
  );
}
