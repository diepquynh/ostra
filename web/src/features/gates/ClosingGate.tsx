import { useState } from "react";
import { Button, Checkbox, Chip } from "../../design";
import { closingAnswer } from "../../lib/gateAnswers";
import { type GateFormProps, muted, OpenGate, row } from "./kit";

type Picks = Record<string, { tests: boolean; docs: boolean }>;

/** Closing gate: per project, whether to write tests and refresh the module documentation (Rule D8, T3). */
export function ClosingGate({ gate, payload, submit, busy, error }: GateFormProps<"closing_gate">) {
  const [picks, setPicks] = useState<Picks>({});
  const pick = (p: string) => picks[p] ?? { tests: false, docs: false };
  const set = (p: string, k: "tests" | "docs", v: boolean) => setPicks({ ...picks, [p]: { ...pick(p), [k]: v } });
  return (
    <OpenGate
      gate={gate}
      error={error}
      actions={
        <Button variant="primary" disabled={busy} onClick={() => submit(closingAnswer(payload.items, picks))}>
          Confirm the closing choices
        </Button>
      }
    >
      {payload.items.map((item) => (
        <div
          key={item.project}
          style={{
            display: "flex",
            flexDirection: "column",
            gap: 8,
            padding: "10px 12px",
            border: "1px solid var(--border-subtle)",
            borderRadius: "var(--radius-md)",
          }}
        >
          <div style={row}>
            <Chip mono outline>
              {item.project}
            </Chip>
            <span style={muted}>
              {item.phases} phase{item.phases === 1 ? "" : "s"}
            </span>
          </div>
          {item.ask_tests && (
            <Checkbox
              checked={pick(item.project).tests}
              onChange={(e) => set(item.project, "tests", e.target.checked)}
              label="Write tests for these phases"
              description="Lists every execution path first, then writes one test per path. Phases tagged Skip stay uncovered with the plan's reason."
            />
          )}
          {item.ask_docs && (
            <Checkbox
              checked={pick(item.project).docs}
              onChange={(e) => set(item.project, "docs", e.target.checked)}
              label="Update the module documentation"
              description="Refreshes the area references for the changed code, grounded in the real source."
            />
          )}
          {!item.ask_tests && !item.ask_docs && (
            <span style={muted}>Your request already decided tests and documentation for this project.</span>
          )}
        </div>
      ))}
      <p style={muted}>
        Leave both unchecked to finish now; that is the recommended default. The completion report says how to run
        either stage later.
      </p>
    </OpenGate>
  );
}
