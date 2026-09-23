import { useState } from "react";
import type { SkillProposal } from "../../api/types";
import { Button, Chip, Select, Table } from "../../design";
import { humanize } from "../../lib/format";
import { DISPOSITIONS, skillsAnswer } from "../../lib/gateAnswers";
import { OpenGate, muted, type GateFormProps } from "./kit";

/** The init flow's skill approval table: per skill, generate, regenerate, reuse, or drop (HANDOVER 8.4). */
export function SkillsGate({ gate, payload, submit, busy, error }: GateFormProps<"skill_approval">) {
  const [picks, setPicks] = useState<Record<string, string>>(() => Object.fromEntries(payload.skills.map((s) => [s.name, s.disposition])));
  const kept = payload.skills.filter((s) => picks[s.name] !== "drop").length;
  const rows = payload.skills.map((s) => ({ ...s, id: s.name }));
  return (
    <OpenGate
      gate={gate}
      error={error}
      actions={
        <>
          <Button variant="primary" disabled={busy} onClick={() => submit(skillsAnswer(payload.skills, picks))}>
            Approve the skills
          </Button>
          <span style={muted}>
            {kept} of {payload.skills.length} kept for <code>{payload.project}</code>
          </span>
        </>
      }
    >
      <p style={muted}>
        The defaults come from the proposal. Change any row before you approve; nothing is written until then.
      </p>
      <Table<SkillProposal & { id: string }>
        dense
        rows={rows}
        columns={[
          { key: "name", label: "Skill", render: (s) => <code style={{ whiteSpace: "nowrap" }}>{s.name}</code> },
          { key: "kind", label: "Kind", render: (s) => <Chip outline>{s.kind}</Chip> },
          {
            key: "description",
            label: "What it covers",
            render: (s) => (
              <div style={{ display: "flex", flexDirection: "column", gap: 3, whiteSpace: "normal" }}>
                <span>{s.description}</span>
                {s.exemplars.length > 0 && <span style={{ ...muted, fontFamily: "var(--font-mono)" }}>{s.exemplars.join(", ")}</span>}
              </div>
            ),
          },
          {
            key: "disposition",
            label: "Decision",
            width: 150,
            render: (s) => (
              <Select
                size="sm"
                aria-label={`Decision for ${s.name}`}
                value={picks[s.name]}
                onChange={(e) => setPicks({ ...picks, [s.name]: e.target.value })}
                options={DISPOSITIONS.map((d) => ({ value: d, label: humanize(d) }))}
              />
            ),
          },
        ]}
      />
    </OpenGate>
  );
}
