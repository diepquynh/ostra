import type { ButtonHTMLAttributes, Ref } from "react";
import { cx } from "../../cx";
import { Icon } from "./Icon";
import type { IconName } from "./icons";

export interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** Lucide icon name. */
  icon: IconName;
  /** Required accessible label; also the tooltip. */
  label: string;
  size?: "sm" | "md";
  variant?: "ghost" | "default" | "primary" | "danger";
  active?: boolean;
  ref?: Ref<HTMLButtonElement>;
}

export function IconButton({
  icon,
  label,
  size = "md",
  variant = "ghost",
  active,
  className = "",
  ...rest
}: IconButtonProps) {
  const cls = cx(
    "os-btn",
    "os-btn--icon",
    variant !== "default" && `os-btn--${variant}`,
    size === "sm" && "os-btn--sm",
    active && "os-btn--active",
    className,
  );
  return (
    <button
      type="button"
      className={cls}
      aria-label={label}
      title={label}
      aria-pressed={active === undefined ? undefined : active}
      {...rest}
    >
      <Icon name={icon} size={size === "sm" ? 13 : 15} />
    </button>
  );
}
