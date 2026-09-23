import type { KeyboardEvent } from "react";

/** Keydown handler that runs `fn` on Enter or Space, for clickable non-button elements with role="button". */
export function activateOnKey(fn: () => void) {
  return (e: KeyboardEvent) => {
    if (e.target !== e.currentTarget) return;
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      fn();
    }
  };
}
