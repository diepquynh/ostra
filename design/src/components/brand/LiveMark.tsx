export type MarkArc = "done" | "run" | "wait" | "fail" | "off";
export type MarkPearl = "run" | "wait" | "fail" | "ok" | "rest" | "off";
/** Eight arcs, one per board lane from research to docs, and the center pearl. A pearl "off" or null is hidden. */
export type MarkState = { arcs: MarkArc[]; pearl: MarkPearl | null };

export const MARK_ARCS = 8;
export const REST: MarkState = { arcs: Array(MARK_ARCS).fill("done"), pearl: "rest" };
export const IDLE: MarkState = { arcs: Array(MARK_ARCS).fill("off"), pearl: null };

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

// Literal colors for favicons, which cannot read CSS variables.
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
const ARC_D = Array.from(
  { length: MARK_ARCS },
  (_, i) => `M${pt(-90 + i * 45 + GAP)}A${R} ${R} 0 0 1 ${pt(-90 + (i + 1) * 45 - GAP)}`,
);
const arcColor = (p: Palette, k: MarkArc) => (k === "run" ? p.done : p[k]);
const pearlColor = (p: Palette, k: MarkPearl) => (k === "run" ? p.done : p[k]);

/**
 * Each arc draws clockwise over an off-colored track when its state leaves "off", and the pearl scales in and out,
 * so a changing state animates. `draw` is the duration of one arc's draw.
 */
export function LiveMark({
  state,
  size = 16,
  label,
  draw = "0.32s",
}: {
  state: MarkState;
  size?: number;
  label?: string;
  draw?: string;
}) {
  const p = CSS_PAL;
  const pearl = state.pearl && state.pearl !== "off" ? state.pearl : null;
  return (
    <svg
      viewBox="0 0 32 32"
      width={size}
      height={size}
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      style={{ display: "block", flex: "none", overflow: "visible" }}
    >
      <rect x={0.5} y={0.5} width={31} height={31} rx={8.5} fill={p.tile} stroke={p.edge} />
      {state.arcs.map((k, i) => (
        <g key={i}>
          <path d={ARC_D[i]} fill="none" stroke={p.off} strokeWidth={3.6} />
          <path
            d={ARC_D[i]}
            fill="none"
            stroke={arcColor(p, k === "off" ? "done" : k)}
            strokeWidth={3.6}
            pathLength={1}
            strokeDasharray="1 1"
            strokeDashoffset={k === "off" ? 1 : 0}
            className={k === "run" ? "live-mark-pulse" : undefined}
            style={{
              transition: `stroke-dashoffset ${draw} cubic-bezier(0.35, 0.1, 0.35, 1), stroke var(--dur-slow) var(--ease-out)`,
            }}
          />
        </g>
      ))}
      <circle
        cx={16}
        cy={16}
        r={3.6}
        fill={pearlColor(p, pearl ?? "rest")}
        className={pearl === "run" ? "live-mark-pulse" : undefined}
        style={{
          transformOrigin: "16px 16px",
          transform: pearl ? "scale(1)" : "scale(0)",
          transition: "transform 0.55s var(--ease-out), fill var(--dur-slow)",
        }}
      />
    </svg>
  );
}

/** The mark as a standalone SVG document, for a favicon. */
export function markSvg(state: MarkState, theme: "dark" | "light"): string {
  const p = PAL[theme];
  const arcs = state.arcs
    .map((k, i) => `<path d="${ARC_D[i]}" fill="none" stroke="${arcColor(p, k)}" stroke-width="3.6"/>`)
    .join("");
  const pearl =
    state.pearl && state.pearl !== "off"
      ? `<circle cx="16" cy="16" r="3.6" fill="${pearlColor(p, state.pearl)}"/>`
      : "";
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect x=".5" y=".5" width="31" height="31" rx="8.5" fill="${p.tile}" stroke="${p.edge}"/>${arcs}${pearl}</svg>`;
}
