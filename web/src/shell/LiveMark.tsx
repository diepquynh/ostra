import { useEffect } from "react";
import type { Lane, SessionStatus } from "../api/types";

type Arc = "done" | "run" | "wait" | "fail" | "off";
type Pearl = "run" | "wait" | "fail" | "ok" | "rest";
export type MarkState = { arcs: Arc[]; pearl: Pearl | null };

const LANES: Lane[] = ["research", "requirements", "verification", "design", "build", "review", "test", "docs"];

export const REST: MarkState = { arcs: LANES.map(() => "done"), pearl: "rest" };
export const IDLE: MarkState = { arcs: LANES.map(() => "off"), pearl: null };

/** The eight arcs are the eight board lanes; the center shows what the session needs from the user. */
export function markState(s: { lane: Lane; status: SessionStatus } | null): MarkState {
  if (!s) return IDLE;
  if (s.status === "completed" || s.lane === "done") return { arcs: LANES.map(() => "done"), pearl: "ok" };
  const kind: "run" | "wait" | "fail" =
    s.status === "failed" || s.status === "stalled" ? "fail" : s.status === "running" ? "run" : "wait";
  const cur = LANES.indexOf(s.lane);
  return { arcs: LANES.map((_, i) => (i < cur ? "done" : i === cur ? kind : "off")), pearl: kind };
}

type Palette = Record<"tile" | "edge" | "done" | "off" | "wait" | "fail" | "ok" | "rest", string>;

const CSS_PAL: Palette = {
  tile: "var(--surface-raised)",
  edge: "var(--border-default)",
  done: "var(--accent)",
  off: "var(--border-strong)",
  wait: "var(--warn)",
  fail: "var(--bad)",
  ok: "var(--ok)",
  rest: "var(--text-primary)",
};

// Literal colors for the favicon, which cannot read CSS variables.
const PAL: Record<"dark" | "light", Palette> = {
  dark: {
    tile: "#1f1f1d",
    edge: "#363633",
    done: "#7ea3ff",
    off: "#4a4a46",
    wait: "#f0b458",
    fail: "#ff8a80",
    ok: "#6fcf8f",
    rest: "#e8e8e3",
  },
  light: {
    tile: "#ffffff",
    edge: "#e0e0da",
    done: "#2459c9",
    off: "#c9c9c2",
    wait: "#9a5b00",
    fail: "#b3261e",
    ok: "#1f7a3f",
    rest: "#2459c9",
  },
};

const R = 9.5;
const GAP = 4.5;
const pt = (deg: number) => {
  const t = (deg * Math.PI) / 180;
  return `${(16 + R * Math.cos(t)).toFixed(2)} ${(16 + R * Math.sin(t)).toFixed(2)}`;
};
const ARC_D = LANES.map((_, i) => `M${pt(-90 + i * 45 + GAP)}A${R} ${R} 0 0 1 ${pt(-90 + (i + 1) * 45 - GAP)}`);
const arcColor = (p: Palette, k: Arc) => (k === "run" ? p.done : p[k]);
const pearlColor = (p: Palette, k: Pearl) => (k === "run" ? p.done : p[k]);

export function LiveMark({ state, size = 16, label }: { state: MarkState; size?: number; label?: string }) {
  const p = CSS_PAL;
  return (
    <svg
      viewBox="0 0 32 32"
      width={size}
      height={size}
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      style={{ display: "block", flex: "none" }}
    >
      <rect x={0.5} y={0.5} width={31} height={31} rx={8.5} fill={p.tile} stroke={p.edge} />
      {state.arcs.map((k, i) => (
        <path
          key={LANES[i]}
          d={ARC_D[i]}
          fill="none"
          stroke={arcColor(p, k)}
          strokeWidth={3.6}
          className={k === "run" ? "live-mark-pulse" : undefined}
          style={{ transition: "stroke var(--dur-slow) var(--ease-out)" }}
        />
      ))}
      {state.pearl && (
        <circle
          cx={16}
          cy={16}
          r={3.6}
          fill={pearlColor(p, state.pearl)}
          className={state.pearl === "run" ? "live-mark-pulse" : undefined}
          style={{ transition: "fill var(--dur-slow)" }}
        />
      )}
    </svg>
  );
}

function markSvg(state: MarkState, theme: "dark" | "light"): string {
  const p = PAL[theme];
  const arcs = state.arcs
    .map((k, i) => `<path d="${ARC_D[i]}" fill="none" stroke="${arcColor(p, k)}" stroke-width="3.6"/>`)
    .join("");
  const pearl = state.pearl ? `<circle cx="16" cy="16" r="3.6" fill="${pearlColor(p, state.pearl)}"/>` : "";
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect x=".5" y=".5" width="31" height="31" rx="8.5" fill="${p.tile}" stroke="${p.edge}"/>${arcs}${pearl}</svg>`;
}

/** Points the browser tab's icon at the mark for `state`, and back at the static favicon on unmount. */
export function useLiveFavicon(state: MarkState, theme: "dark" | "light") {
  const svg = markSvg(state, theme);
  useEffect(() => {
    const link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
    if (!link) return;
    link.href = `data:image/svg+xml,${encodeURIComponent(svg)}`;
    return () => {
      link.href = "/favicon.svg";
    };
  }, [svg]);
}
