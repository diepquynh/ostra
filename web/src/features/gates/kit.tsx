import type { CSSProperties, ReactNode } from "react";
import type { GateAnswer, GatePayload, GateView } from "../../api/types";
import { Button, GateCard as DesignGateCard, type IconName } from "../../design";
import { useNav } from "../../lib/nav";

export type Submit = (answer: GateAnswer) => void;

/** What every gate form receives from GateCard. */
export type GateFormProps<K extends GatePayload["kind"]> = {
  gate: GateView;
  payload: Extract<GatePayload, { kind: K }>;
  submit: Submit;
  /** Show a validation message under the form. */
  fail: (message: string) => void;
  busy: boolean;
  error: string | null;
};

/** The open gate shell: design GateCard with the form body, an error line, and the button row. */
export function OpenGate({
  gate,
  error,
  actions,
  children,
}: {
  gate: GateView;
  error: string | null;
  actions: ReactNode;
  children?: ReactNode;
}) {
  return (
    <DesignGateCard kind={gate.payload.kind} title={gate.title} explanation={gate.explanation} actions={actions}>
      {children}
      {error && (
        <div className="os-field__error" role="alert">
          {error}
        </div>
      )}
    </DesignGateCard>
  );
}

export const muted: CSSProperties = {
  fontSize: "var(--text-sm)",
  color: "var(--text-muted)",
  margin: 0,
  lineHeight: "var(--leading-normal)",
};
export const para: CSSProperties = { margin: 0, lineHeight: "var(--leading-normal)" };
export const row: CSSProperties = { display: "flex", flexWrap: "wrap", gap: 6, alignItems: "center" };

/** Opens a spec, plan, ledger, or report in a tab. */
export function ArtifactLink({
  path,
  children,
  icon = "file-text",
}: {
  path: string;
  children: ReactNode;
  icon?: IconName;
}) {
  const nav = useNav();
  return (
    <Button size="sm" variant="ghost" icon={icon} onClick={() => nav.open(`artifact:${path}`)}>
      {children}
    </Button>
  );
}

/** Opens an execution's Activity or Terminal tab. */
export function ExecutionLink({
  id,
  children,
  icon = "square-terminal",
}: {
  id: string;
  children: ReactNode;
  icon?: IconName;
}) {
  const nav = useNav();
  return (
    <Button size="sm" variant="ghost" icon={icon} onClick={() => nav.open(`exec:${id}`)}>
      {children}
    </Button>
  );
}
