import { type KeyboardEvent, type RefObject, useCallback, useEffect } from "react";

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function focusables(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter((el) => !el.closest("[inert]"));
}

/**
 * Modal focus handling: while `active`, moves focus into `ref` (the `initial` element, else the first
 * focusable, else the container), keeps Tab inside it, and restores the previous focus on deactivate.
 * Returns the keydown handler that wraps Tab; attach it to the container.
 */
export function useFocusTrap(
  ref: RefObject<HTMLElement | null>,
  active: boolean,
  initial?: RefObject<HTMLElement | null>,
) {
  useEffect(() => {
    if (!active) return;
    const previous = document.activeElement as HTMLElement | null;
    const root = ref.current;
    if (root && !root.contains(document.activeElement)) {
      const target = initial?.current ?? focusables(root)[0] ?? root;
      target.focus({ preventScroll: true });
    }
    return () => {
      if (previous && previous.isConnected && typeof previous.focus === "function")
        previous.focus({ preventScroll: true });
    };
  }, [active, ref, initial]);

  return useCallback(
    (e: KeyboardEvent) => {
      if (e.key !== "Tab" || !ref.current) return;
      const list = focusables(ref.current);
      if (list.length === 0) {
        e.preventDefault();
        return;
      }
      const first = list[0];
      const last = list[list.length - 1];
      const current = document.activeElement;
      if (e.shiftKey && (current === first || current === ref.current)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && current === last) {
        e.preventDefault();
        first.focus();
      }
    },
    [ref],
  );
}

/** Move focus among `items` with arrow keys, Home and End. Returns the new index, or null for other keys. */
export function arrowIndex(
  key: string,
  index: number,
  count: number,
  orientation: "vertical" | "horizontal",
  wrap = true,
): number | null {
  if (count === 0) return null;
  const next = orientation === "vertical" ? "ArrowDown" : "ArrowRight";
  const prev = orientation === "vertical" ? "ArrowUp" : "ArrowLeft";
  if (key === next) return index + 1 < count ? index + 1 : wrap ? 0 : index;
  if (key === prev) return index > 0 ? index - 1 : wrap ? count - 1 : index;
  if (key === "Home") return 0;
  if (key === "End") return count - 1;
  return null;
}
