import { cx } from "../../cx";
import type { Tone } from "./Chip";

export interface StatusDotProps {
  tone?: Tone;
  /** Slow opacity pulse for live/running state. */
  pulse?: boolean;
  /** Outline only, for pending/queued. */
  hollow?: boolean;
  size?: number;
  title?: string;
}

export function StatusDot({ tone = "neutral", pulse, hollow, size = 7, title }: StatusDotProps) {
  const cls = cx("os-dot", tone !== "neutral" && `os-dot--${tone}`, pulse && "os-dot--pulse", hollow && "os-dot--hollow");
  return <span className={cls} title={title} style={size !== 7 ? { width: size, height: size } : undefined} />;
}
