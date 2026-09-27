import { useSyncExternalStore } from "react";
import { isShot } from "../shell/shot";

export const MOBILE_QUERY = "(max-width: 720px)";

const media = () => (typeof window !== "undefined" && window.matchMedia ? window.matchMedia(MOBILE_QUERY) : null);

function subscribe(onChange: () => void) {
  const m = media();
  m?.addEventListener("change", onChange);
  return () => m?.removeEventListener("change", onChange);
}

/** True on a phone-width viewport, live across rotation and resizes. The homepage shot is always desktop. */
export function useIsMobile(): boolean {
  return useSyncExternalStore(
    subscribe,
    () => !isShot && !!media()?.matches,
    () => false,
  );
}
