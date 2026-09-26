import { IDLE, type MarkState, markSvg } from "@ostra/design";
import { useEffect } from "react";
import type { Lane, SessionStatus } from "../api/types";

const LANES: Lane[] = ["research", "requirements", "verification", "design", "build", "review", "test", "docs"];

/** The eight arcs are the eight board lanes; the center shows what the session needs from the user. */
export function markState(s: { lane: Lane; status: SessionStatus } | null): MarkState {
  if (!s) return IDLE;
  if (s.status === "completed" || s.lane === "done") return { arcs: LANES.map(() => "done"), pearl: "ok" };
  const kind: "run" | "wait" | "fail" =
    s.status === "failed" || s.status === "stalled" ? "fail" : s.status === "running" ? "run" : "wait";
  const cur = LANES.indexOf(s.lane);
  return { arcs: LANES.map((_, i) => (i < cur ? "done" : i === cur ? kind : "off")), pearl: kind };
}

/** Points the browser tab's icon at the mark for `state`, and back at the static favicon on unmount. */
export function useLiveFavicon(state: MarkState, theme: "dark" | "light") {
  const svg = markSvg(state, theme);
  useEffect(() => {
    const link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
    if (!link) return;
    const rest = link.href;
    link.href = `data:image/svg+xml,${encodeURIComponent(svg)}`;
    return () => {
      link.href = rest;
    };
  }, [svg]);
}
