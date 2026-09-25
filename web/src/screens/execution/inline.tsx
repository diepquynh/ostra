import type { ReactNode } from "react";

/** Plain text with its `code spans` set in mono, for reasons and findings that agents and guards write. */
export function inlineCode(text: string): ReactNode[] {
  return text
    .split(/(`[^`]+`)/g)
    .map((part, i) =>
      part.length > 2 && part.startsWith("`") && part.endsWith("`") ? <code key={i}>{part.slice(1, -1)}</code> : part,
    );
}
