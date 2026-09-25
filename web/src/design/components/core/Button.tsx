import type { ButtonHTMLAttributes, ReactNode, Ref } from "react";
import { cx } from "../../cx";
import { Icon } from "./Icon";
import type { IconName } from "./icons";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** primary = the one forward action (Approve, Start). danger = Deny, Cancel execution. ghost = toolbar/secondary. */
  variant?: "default" | "primary" | "ghost" | "danger";
  size?: "sm" | "md" | "lg";
  /** Lucide icon name shown before the label. */
  icon?: IconName;
  iconRight?: IconName;
  /** Keyboard hint rendered as a Kbd after the label, e.g. "⌘↵". */
  kbd?: string;
  /** Toggled-on state for toolbar toggles. */
  active?: boolean;
  children?: ReactNode;
  ref?: Ref<HTMLButtonElement>;
}

export function Button({
  variant = "default",
  size = "md",
  icon,
  iconRight,
  kbd,
  active,
  className = "",
  children,
  type = "button",
  ...rest
}: ButtonProps) {
  const cls = cx(
    "os-btn",
    variant !== "default" && `os-btn--${variant}`,
    size !== "md" && `os-btn--${size}`,
    active && "os-btn--active",
    className,
  );
  const is = size === "sm" ? 13 : 15;
  return (
    <button type={type} className={cls} aria-pressed={active === undefined ? undefined : active} {...rest}>
      {icon && <Icon name={icon} size={is} />}
      {children}
      {iconRight && <Icon name={iconRight} size={is} />}
      {kbd && <span className="os-kbd">{kbd}</span>}
    </button>
  );
}
