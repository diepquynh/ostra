import { type ReactNode, useState } from "react";
import type { GatePayload } from "../../api/types";
import { Button, Chip, Input } from "../../design";
import { formatCost, humanize } from "../../lib/format";
import { CHOICES, type ChoiceGateKind, choiceAnswer } from "../../lib/gateAnswers";
import { FactFindings, ReviewFindings } from "./Findings";
import { ArtifactLink, ExecutionLink, type GateFormProps, muted, OpenGate, para, row } from "./kit";

type ChoicePayload = Extract<GatePayload, { kind: ChoiceGateKind }>;
type Props = GateFormProps<ChoiceGateKind>;

type TextField = { label: string; hint?: string; placeholder?: string; rows?: number; mono?: boolean };

/** The gate's optional or required text field, if its options send one. */
function textField(p: ChoicePayload): TextField | null {
  switch (p.kind) {
    case "fact_check_recurring":
      return {
        label: `What the ${p.target} agent should know to resolve these findings`,
        hint: "Optional. It goes to the agent with the findings.",
      };
    case "review_cap":
      return {
        label: "Instruction for the fix pass",
        hint: "Optional. It is added after the findings the fix agent receives.",
      };
    case "stuck":
      return { label: "The missing fact", hint: "State it plainly. It is quoted to the agent verbatim.", rows: 3 };
    case "phase_blocked":
      return {
        label: "Instructions for the retry",
        hint: "Optional. Leave empty to retry with the last review's findings.",
      };
    case "budget_reached":
      return {
        label: "Amount to add, in US dollars",
        hint: `Optional. Leave empty to add ${formatCost(p.budget_usd)} again.`,
        placeholder: p.budget_usd.toFixed(2),
        rows: 1,
        mono: true,
      };
    default:
      return null;
  }
}

function body(p: ChoicePayload): ReactNode {
  switch (p.kind) {
    case "fact_check_recurring":
      return (
        <>
          <p style={para}>
            The same findings came back on {p.passes} passes over the {p.target}. Stopping ends the session.
          </p>
          <FactFindings findings={p.findings} />
        </>
      );
    case "review_cap":
      return (
        <>
          <p style={para}>
            A fourth pass on phase {p.phase}
            {p.tests ? " tests" : ""} in <code>{p.project}</code> runs only if you choose it. Stopping blocks this phase
            and every phase that depends on it; independent phases continue.
          </p>
          <ReviewFindings findings={p.findings} />
          <div style={row}>
            <ArtifactLink path={p.ledger_path} icon="file-diff">
              Open the review ledger
            </ArtifactLink>
          </div>
        </>
      );
    case "stuck":
      return (
        <>
          <div style={row}>
            <Chip>{humanize(p.agent)}</Chip>
            <Chip mono outline>
              {p.project}
            </Chip>
            {p.phase !== null && <Chip mono>phase {p.phase}</Chip>}
            <ExecutionLink id={p.execution}>Open the execution</ExecutionLink>
          </div>
          <div style={muted}>Diagnostic</div>
          <pre style={{ margin: 0 }}>{p.diagnostic}</pre>
          <p style={para}>
            <strong>Needs:</strong> {p.need}
          </p>
        </>
      );
    case "phase_blocked":
      return (
        <>
          <p style={para}>
            Phase {p.phase} in <code>{p.project}</code> cannot complete: {p.reason}
          </p>
        </>
      );
    case "harness_failure":
      return (
        <>
          <div style={row}>
            <Chip mono>harness:{p.harness}</Chip>
            <ExecutionLink id={p.execution}>Open the terminal</ExecutionLink>
          </div>
          <pre style={{ margin: 0 }}>{p.error}</pre>
          <p style={muted}>
            A retry uses the same harness, so log in first. The native executor runs this agent on Ostra's own loop for
            the rest of the session.
          </p>
        </>
      );
    case "execution_failed":
      return (
        <>
          <div style={row}>
            <Chip>{humanize(p.agent)}</Chip>
            <Chip mono outline>
              {p.project}
            </Chip>
            <ExecutionLink id={p.execution}>Open the execution</ExecutionLink>
          </div>
          <pre style={{ margin: 0 }}>{p.error}</pre>
        </>
      );
    case "budget_reached":
      return (
        <>
          <div style={row}>
            <Chip mono tone="warn">
              spent {formatCost(p.spent_usd)}
            </Chip>
            <Chip mono outline>
              budget {formatCost(p.budget_usd)}
            </Chip>
          </div>
          <p style={muted}>YOLO never answers this gate, because spending more is your call.</p>
        </>
      );
  }
}

/** Every gate answered by picking one option: review cap, stuck, blocked phase, failures, budget, recurring fact-check. */
export function ChoiceGate({ gate, payload, submit, fail, busy, error }: Props) {
  const [text, setText] = useState("");
  const field = textField(payload);
  const pick = (option: string) => {
    const r = choiceAnswer(payload.kind, option, text);
    if ("error" in r) fail(r.error);
    else submit(r.answer);
  };
  return (
    <OpenGate
      gate={gate}
      error={error}
      actions={CHOICES[payload.kind].map((o) => (
        <Button key={o.option} variant={o.variant} disabled={busy} onClick={() => pick(o.option)}>
          {o.label}
        </Button>
      ))}
    >
      {body(payload)}
      {field && (
        <Input
          multiline={(field.rows ?? 2) > 1}
          rows={field.rows ?? 2}
          mono={field.mono}
          label={field.label}
          hint={field.hint}
          placeholder={field.placeholder}
          value={text}
          onChange={(e) => setText(e.target.value)}
        />
      )}
    </OpenGate>
  );
}
