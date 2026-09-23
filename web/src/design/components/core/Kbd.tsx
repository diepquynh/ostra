import type { ReactNode } from "react";

export interface KbdProps {
  /** Keys rendered as separate caps, e.g. ["⌘", "K"]. */
  keys?: string[];
  children?: ReactNode;
}

export function Kbd({ keys, children }: KbdProps) {
  const list: ReactNode[] = keys ?? (children != null ? [children] : []);
  return (
    <span className="os-kbd-group">
      {list.map((k, i) => (
        <kbd key={i} className="os-kbd">
          {k}
        </kbd>
      ))}
    </span>
  );
}
