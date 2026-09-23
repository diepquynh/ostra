import type { CSSProperties } from "react";
import { ICONS, type IconName } from "./icons";

export interface IconProps {
  /** Lucide icon name in kebab-case, e.g. "folder", "git-branch", "circle-check". */
  name: IconName;
  /** Pixel size. 14 in dense rows, 16 default, 12 inside chips. */
  size?: number;
  /** CSS color. Defaults to currentColor. */
  color?: string;
  /** Accessible label. Omit for decorative icons. */
  title?: string;
  className?: string;
  style?: CSSProperties;
}

/** Lucide icon at stroke 1.75, inlined so it inherits currentColor. */
export function Icon({ name, size = 16, color, title, style, className = "" }: IconProps) {
  const Svg = ICONS[name];
  return (
    <span
      role={title ? "img" : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : true}
      className={`os-icon ${className}`}
      style={{ display: "inline-flex", flex: "none", width: size, height: size, color, ...style }}
    >
      {Svg && <Svg width="100%" height="100%" strokeWidth={1.75} aria-hidden />}
    </span>
  );
}
