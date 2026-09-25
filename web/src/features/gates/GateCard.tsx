import { useState } from "react";
import { api } from "../../api";
import type { GateAnswer, GateView } from "../../api/types";
import { GateCard as DesignGateCard } from "../../design";
import { answerSummary, type ChoiceGateKind, isChoiceKind } from "../../lib/gateAnswers";
import { ApprovalGate } from "./ApprovalGate";
import { ChoiceGate } from "./ChoiceGate";
import { ClosingGate } from "./ClosingGate";
import type { GateFormProps } from "./kit";
import { PermissionGate } from "./PermissionGate";
import { QuestionsGate } from "./QuestionsGate";
import { SkillsGate } from "./SkillsGate";

type Common = Omit<GateFormProps<never>, "payload">;

function OpenForm({ gate, ...rest }: Common) {
  const p = gate.payload;
  const props = { gate, ...rest };
  if (isChoiceKind(p.kind)) return <ChoiceGate {...props} payload={p as GateFormProps<ChoiceGateKind>["payload"]} />;
  switch (p.kind) {
    case "open_questions":
      return <QuestionsGate {...props} payload={p} />;
    case "spec_approval":
    case "plan_approval":
      return <ApprovalGate {...props} payload={p} />;
    case "closing_gate":
      return <ClosingGate {...props} payload={p} />;
    case "permission":
      return <PermissionGate {...props} payload={p} />;
    case "skill_approval":
      return <SkillsGate {...props} payload={p} />;
    default:
      return <DesignGateCard kind={p.kind} title={gate.title} explanation={gate.explanation} />;
  }
}

/**
 * One gate: the answer form while open, the recorded answer once answered. `#gate-<id>` anchors it. The answer
 * shape per kind is `lib/gateAnswers.ts`, which mirrors the engine's `validate_answer`.
 */
export function GateCard({
  gate,
  onAnswered,
}: {
  gate: GateView;
  highlight?: boolean;
  onAnswered?: (g: GateView) => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const submit = (answer: GateAnswer) => {
    setBusy(true);
    setError(null);
    api
      .answerGate(gate.id, { answer })
      .then((g) => onAnswered?.(g))
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setBusy(false));
  };
  if (gate.answer !== null) {
    const summary = answerSummary(gate);
    return (
      <div id={`gate-${gate.id}`}>
        <DesignGateCard
          kind={gate.payload.kind}
          title={gate.title}
          answered
          answeredBy={gate.source === "yolo" ? "yolo" : "user"}
          answer={gate.source === "engine" ? `Closed by Ostra: ${summary}` : summary}
          reason={gate.reason ?? undefined}
        />
      </div>
    );
  }
  return (
    <div id={`gate-${gate.id}`} style={busy ? { opacity: 0.7 } : undefined}>
      <OpenForm gate={gate} submit={submit} fail={setError} busy={busy} error={error} />
    </div>
  );
}
