import type { CSSProperties } from "react";

export interface SpinnerProps {
  size?: number;
  style?: CSSProperties;
}

export function Spinner({ size = 12, style }: SpinnerProps) {
  return (
    <span className="os-spinner" role="status" style={{ width: size, height: size, ...style }} aria-label="Loading" />
  );
}
