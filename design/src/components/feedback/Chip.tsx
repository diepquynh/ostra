import type { ReactNode } from "react";
import { cx } from "../../cx";
import { Icon } from "../core/Icon";
import type { IconName } from "../core/icons";

export type Tone = "neutral" | "accent" | "ok" | "warn" | "bad" | "info";

export interface ChipProps {
  tone?: Tone;
  /** Lucide icon at 11px. */
  icon?: IconName;
  /** Monospace label, for ids, executors, models, rule names. */
  mono?: boolean;
  outline?: boolean;
  title?: string;
  className?: string;
  children?: ReactNode;
}

export function Chip({ tone = "neutral", icon, mono, outline, title, className = "", children }: ChipProps) {
  const cls = cx(
    "os-chip",
    tone !== "neutral" && `os-chip--${tone}`,
    mono && "os-chip--mono",
    outline && "os-chip--outline",
    className,
  );
  return (
    <span className={cls} title={title}>
      {icon && <Icon name={icon} size={11} />}
      {children}
    </span>
  );
}
