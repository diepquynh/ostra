import { useEffect, useState, type PointerEvent } from "react";

export type PanelWidths = { sidebar: number; dock: number };

const KEY = "ostra.panels";
const DEFAULT: PanelWidths = { sidebar: 264, dock: 380 };
const LIMITS = { sidebar: [180, 640], dock: [260, 900] } as const;

const clamp = (side: keyof PanelWidths, w: number) => Math.round(Math.min(LIMITS[side][1], Math.max(LIMITS[side][0], w)));

/** Panel widths in pixels, kept per browser because they depend on the screen, not the workspace. */
export function usePanelWidths(): [PanelWidths, (side: keyof PanelWidths, w: number) => void] {
  const [widths, setWidths] = useState<PanelWidths>(() => {
    try {
      const raw = JSON.parse(localStorage.getItem(KEY) ?? "null") as Partial<PanelWidths> | null;
      return { sidebar: clamp("sidebar", raw?.sidebar ?? DEFAULT.sidebar), dock: clamp("dock", raw?.dock ?? DEFAULT.dock) };
    } catch {
      return DEFAULT;
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem(KEY, JSON.stringify(widths));
    } catch {
      // Storage may be unavailable; the widths still apply for this page load.
    }
  }, [widths]);
  return [widths, (side, w) => setWidths((p) => ({ ...p, [side]: clamp(side, w) }))];
}

/** A vertical drag handle on a panel edge. `edge` is the side of the panel the handle sits on. */
export function ResizeHandle({ side, edge, width, onResize }: { side: keyof PanelWidths; edge: "left" | "right"; width: number; onResize: (side: keyof PanelWidths, w: number) => void }) {
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const startX = e.clientX;
    const start = width;
    const el = e.currentTarget;
    el.setPointerCapture(e.pointerId);
    document.body.classList.add("shell-resizing");
    const move = (ev: globalThis.PointerEvent) => onResize(side, start + (edge === "right" ? ev.clientX - startX : startX - ev.clientX));
    const up = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
      document.body.classList.remove("shell-resizing");
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
  };
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={side === "sidebar" ? "Resize left dock" : "Resize question dock"}
      aria-valuenow={width}
      tabIndex={0}
      className={`shell-resize shell-resize--${edge}`}
      onPointerDown={onPointerDown}
      onDoubleClick={() => onResize(side, DEFAULT[side])}
      onKeyDown={(e) => {
        const step = e.shiftKey ? 48 : 16;
        const grow = edge === "right" ? "ArrowRight" : "ArrowLeft";
        const shrink = edge === "right" ? "ArrowLeft" : "ArrowRight";
        if (e.key === grow) onResize(side, width + step);
        else if (e.key === shrink) onResize(side, width - step);
        else return;
        e.preventDefault();
      }}
    />
  );
}
