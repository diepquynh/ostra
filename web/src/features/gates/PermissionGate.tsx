import { Button, Chip } from "../../design";
import { humanize } from "../../lib/format";
import { permission } from "../../lib/gateAnswers";
import { ExecutionLink, OpenGate, muted, row, type GateFormProps } from "./kit";

/** The tool call input, as a shell line for Bash and as JSON otherwise. */
function callText(tool: string, input: unknown): string {
  const i = (input ?? {}) as Record<string, unknown>;
  if (tool === "Bash" && typeof i.command === "string") return `$ ${i.command}`;
  return JSON.stringify(input, null, 2);
}

/** A permission ask from the policy's second layer: allow once, allow for the workspace, or deny. */
export function PermissionGate({ gate, payload, submit, busy, error }: GateFormProps<"permission">) {
  const input = (payload.call.input ?? {}) as Record<string, unknown>;
  return (
    <OpenGate
      gate={gate}
      error={error}
      actions={
        <>
          <Button variant="primary" disabled={busy} onClick={() => submit(permission("allow-once"))}>
            Allow once
          </Button>
          <Button
            disabled={busy || !payload.suggestion}
            onClick={() => submit(permission("always-in-workspace"))}
            title={payload.suggestion ? `Adds ${payload.suggestion} to the workspace allow rules` : "This call has no rule to add"}
          >
            Always in this workspace{payload.suggestion ? ":" : ""}
            {payload.suggestion && <code style={{ marginLeft: 4 }}>{payload.suggestion}</code>}
          </Button>
          <Button variant="danger" disabled={busy} onClick={() => submit(permission("deny"))}>
            Deny
          </Button>
        </>
      }
    >
      <div style={row}>
        <Chip>{humanize(payload.agent)}</Chip>
        <span style={{ fontFamily: "var(--font-mono)", fontSize: "var(--text-sm)" }}>{payload.call.tool}</span>
        <ExecutionLink id={payload.execution}>Open the execution</ExecutionLink>
      </div>
      {typeof input.description === "string" && <div style={muted}>{input.description}</div>}
      <pre style={{ margin: 0 }}>{callText(payload.call.tool, payload.call.input)}</pre>
      <div style={{ fontSize: "var(--text-sm)", color: "var(--text-secondary)" }}>
        {payload.reason !== gate.explanation && <>{payload.reason} </>}
        <span style={{ color: "var(--text-muted)" }}>
          Rule: {payload.rule.layer} <code>{payload.rule.rule}</code>
        </span>
      </div>
      <p style={muted}>The execution is paused until you answer. A denial tells the agent to continue without the call.</p>
    </OpenGate>
  );
}
