import type { CSSProperties, ReactNode } from "react";
import { Icon } from "../core/Icon";
import type { IconName } from "../core/icons";

export interface BannerProps {
  /** warn = YOLO on / not initialized; bad = settings invalid, errors, security blocks; info = neutral notices. */
  tone?: "warn" | "bad" | "info" | "neutral";
  title?: string;
  /** Override the default Lucide icon for the tone. */
  icon?: IconName;
  /** Buttons on the right edge. Security blocks never get a dismiss action. */
  actions?: ReactNode;
  children?: ReactNode;
  style?: CSSProperties;
}

const ICONS: Record<NonNullable<BannerProps["tone"]>, IconName> = {
  warn: "triangle-alert",
  bad: "octagon-alert",
  info: "info",
  neutral: "info",
};

export function Banner({ tone = "warn", title, icon, actions, children, style }: BannerProps) {
  return (
    <div className={`os-banner os-banner--${tone}`} role={tone === "bad" ? "alert" : "status"} style={style}>
      <Icon name={icon ?? ICONS[tone]} size={15} className="os-banner__icon" />
      <div className="os-banner__body">
        {title && <div className="os-banner__title">{title}</div>}
        {children}
      </div>
      {actions && <div style={{ display: "flex", gap: 6, flex: "none" }}>{actions}</div>}
    </div>
  );
}
