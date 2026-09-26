import { type ReactNode, useId } from "react";

export interface TreeSectionProps {
  label: string;
  actions?: ReactNode;
  children?: ReactNode;
}

/** Uppercase group header for sidebar rows. */
export function TreeSection({ label, actions, children }: TreeSectionProps) {
  const id = useId();
  return (
    <div role="group" aria-labelledby={id}>
      <div className="os-tree-section">
        <span id={id}>{label}</span>
        {actions && <span style={{ display: "flex", gap: 2 }}>{actions}</span>}
      </div>
      {children}
    </div>
  );
}
