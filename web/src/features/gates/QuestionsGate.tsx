import { Button, Checkbox, Chip, Input } from "@ostra/design";
import { useState } from "react";
import {
  defaultSelections,
  OTHER,
  orderedOptions,
  type QuestionSelection,
  questionsAnswer,
  toggleSelection,
} from "../../lib/gateAnswers";
import { ArtifactLink, type GateFormProps, muted, OpenGate, row } from "./kit";

/** Open questions from the spec or plan, recommended option first, with an Other answer. */
export function QuestionsGate({ gate, payload, submit, fail, busy, error }: GateFormProps<"open_questions">) {
  const [sel, setSel] = useState<Record<string, QuestionSelection>>(() => defaultSelections(payload.questions));
  const send = () => {
    const r = questionsAnswer(payload.questions, sel);
    if ("error" in r) fail(r.error);
    else submit(r.answer);
  };
  return (
    <OpenGate
      gate={gate}
      error={error}
      actions={
        <Button variant="primary" disabled={busy} onClick={send}>
          Send the answers
        </Button>
      }
    >
      <div style={row}>
        <ArtifactLink path={payload.artifact_path}>Read the {payload.artifact}</ArtifactLink>
      </div>
      {payload.questions.map((q) => {
        const s = sel[q.id];
        const set = (next: QuestionSelection) => setSel({ ...sel, [q.id]: next });
        return (
          <fieldset
            key={q.id}
            style={{
              border: "1px solid var(--border-subtle)",
              borderRadius: "var(--radius-md)",
              margin: 0,
              padding: "10px 12px",
              display: "flex",
              flexDirection: "column",
              gap: 8,
            }}
          >
            <legend style={{ ...row, padding: "0 4px" }}>
              <Chip mono>{q.id}</Chip>
              <Chip tone="info">{q.tag}</Chip>
              {q.multi_select && <span style={muted}>Choose one or more</span>}
            </legend>
            <div style={{ fontWeight: "var(--weight-semibold)" }}>{q.question}</div>
            {orderedOptions(q).map((o) => (
              <Checkbox
                key={o.label}
                radio={!q.multi_select}
                name={`${gate.id}-${q.id}`}
                checked={s.selected.includes(o.label)}
                onChange={() => set(toggleSelection(s, o.label, q.multi_select))}
                label={
                  <>
                    {o.label}
                    {o.recommended && (
                      <span style={{ marginLeft: 6 }}>
                        <Chip tone="ok">Recommended</Chip>
                      </span>
                    )}
                  </>
                }
                description={o.description}
              />
            ))}
            <Checkbox
              radio={!q.multi_select}
              name={`${gate.id}-${q.id}`}
              checked={s.selected.includes(OTHER)}
              onChange={() => set(toggleSelection(s, OTHER, q.multi_select))}
              label="Other"
              description="Write your own answer."
            />
            {s.selected.includes(OTHER) && (
              <Input
                multiline
                rows={2}
                aria-label={`${q.id} answer`}
                placeholder="Your answer"
                value={s.other}
                onChange={(e) => set({ ...s, other: e.target.value })}
              />
            )}
          </fieldset>
        );
      })}
    </OpenGate>
  );
}
