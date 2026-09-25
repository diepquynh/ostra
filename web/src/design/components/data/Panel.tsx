import type { CSSProperties, ReactNode } from "react";
import { cx } from "../../cx";
import { Icon } from "../core/Icon";
import type { IconName } from "../core/icons";

export interface PanelProps {
  title?: ReactNode;
  subtitle?: ReactNode;
  icon?: IconName;
  /** Right side of the header. */
  actions?: ReactNode;
  /** highlight = the current item; warn/bad = attention border. */
  tone?: "highlight" | "warn" | "bad";
  /** No background, border only. */
  flush?: boolean;
  /** Remove body padding (for tables and lists). */
  bodyFlush?: boolean;
  children?: ReactNode;
  style?: CSSProperties;
  className?: string;
}

/** Bordered container with an optional 36px header; the only card shape. */
export function Panel({
  title,
  subtitle,
  icon,
  actions,
  tone,
  flush,
  bodyFlush,
  children,
  style,
  className = "",
}: PanelProps) {
  const cls = cx("os-panel", tone && `os-panel--${tone}`, flush && "os-panel--flush", className);
  return (
    <section className={cls} style={style}>
      {(title || actions) && (
        <header className="os-panel__head">
          {icon && <Icon name={icon} size={14} style={{ color: "var(--text-muted)" }} />}
          <div className="os-panel__title">
            {title}
            {subtitle && <span className="os-panel__sub"> · {subtitle}</span>}
          </div>
          {actions}
        </header>
      )}
      <div className={`os-panel__body ${bodyFlush ? "os-panel__body--flush" : ""}`}>{children}</div>
    </section>
  );
}
