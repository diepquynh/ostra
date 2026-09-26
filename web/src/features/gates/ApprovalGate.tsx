import { Button, Chip, Input, Table } from "@ostra/design";
import { useState } from "react";
import type { PhaseInfo } from "../../api/types";
import { approval, changeRequest } from "../../lib/gateAnswers";
import { FactFindings } from "./Findings";
import { ArtifactLink, type GateFormProps, muted, OpenGate, para, row } from "./kit";

type Props = GateFormProps<"spec_approval" | "plan_approval">;

const NOTE = {
  spec: "Approving lets Ostra judge the stakes and, unless they are low, plan the phases from this spec.",
  plan: "Approving starts implementation, phase by phase in dependency order.",
};

/** Spec or plan approval. The gate opens only after a fact-check PASS; LOW findings are shown. */
export function ApprovalGate({ gate, payload, submit, fail, busy, error }: Props) {
  const [feedback, setFeedback] = useState("");
  const what = payload.kind === "spec_approval" ? "spec" : "plan";
  const path = payload.kind === "spec_approval" ? payload.spec_path : payload.plan_path;
  const requestChanges = () => {
    const r = changeRequest(feedback);
    if ("error" in r) fail(r.error);
    else submit(r.answer);
  };
  return (
    <OpenGate
      gate={gate}
      error={error}
      actions={
        <>
          <Button
            variant="primary"
            icon="check"
            disabled={busy || Boolean(feedback.trim())}
            onClick={() => submit(approval(true))}
            title={feedback.trim() ? "Clear the change request to approve" : undefined}
          >
            Approve the {what}
          </Button>
          <Button disabled={busy} onClick={requestChanges}>
            Request changes to the {what}
          </Button>
        </>
      }
    >
      <div style={row}>
        <Chip tone="ok" icon="check">
          Fact-check PASS
        </Chip>
        <ArtifactLink path={path}>Read the {what}</ArtifactLink>
      </div>
      <p style={para}>{payload.summary}</p>
      {payload.kind === "plan_approval" && <PhaseTable phases={payload.phases} />}
      {payload.findings.length > 0 && (
        <>
          <p style={muted}>These LOW findings do not block approval. Read them before you approve.</p>
          <FactFindings findings={payload.findings} />
        </>
      )}
      <p style={muted}>{NOTE[what]}</p>
      <Input
        multiline
        rows={2}
        aria-label={`What should change in the ${what}`}
        placeholder={`What should change in the ${what}? Leave empty to approve.`}
        value={feedback}
        onChange={(e) => setFeedback(e.target.value)}
      />
    </OpenGate>
  );
}

function PhaseTable({ phases }: { phases: PhaseInfo[] }) {
  return (
    <Table
      dense
      rows={phases}
      columns={[
        {
          key: "title",
          label: "Phase",
          render: (p) => (
            <span style={row}>
              <span style={{ fontFamily: "var(--font-mono)" }}>{p.id}</span> {p.title}{" "}
              {p.deliverable && <Chip>{p.deliverable}</Chip>}
            </span>
          ),
        },
        {
          key: "project",
          label: "Project",
          render: (p) => (
            <Chip mono outline>
              {p.project}
            </Chip>
          ),
        },
        { key: "complexity", label: "Complexity" },
        {
          key: "test_policy",
          label: "Tests",
          render: (p) => (
            <span title={p.test_rationale ?? undefined}>
              <Chip tone={p.test_policy === "Skip" ? "neutral" : "accent"} outline={p.test_policy === "Skip"}>
                {p.test_policy}
              </Chip>
            </span>
          ),
        },
        {
          key: "depends_on",
          label: "Depends on",
          render: (p) =>
            p.depends_on === null ? (
              <span style={muted}>Unclear, runs after earlier phases</span>
            ) : p.depends_on.length ? (
              p.depends_on.join(", ")
            ) : (
              <span style={muted}>None</span>
            ),
        },
      ]}
    />
  );
}
