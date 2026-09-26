import type { CSSProperties, ReactNode } from "react";

export interface SectionLabelProps {
  children?: ReactNode;
  style?: CSSProperties;
}

/** Uppercase 10.5px group label. */
export function SectionLabel({ children, style }: SectionLabelProps) {
  return (
    <div className="os-section-label" style={style}>
      {children}
    </div>
  );
}
