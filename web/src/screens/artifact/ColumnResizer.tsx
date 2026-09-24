import type { KeyboardEvent, PointerEvent } from "react";

const MIN = 48;

/** Freezes every column at its rendered width, so moving one edge does not reflow the others. */
function freeze(table: HTMLTableElement) {
  if (table.style.tableLayout === "fixed") return;
  const heads = Array.from(table.tHead?.rows[0]?.cells ?? []);
  const widths = heads.map((th) => th.getBoundingClientRect().width);
  heads.forEach((th, i) => (th.style.width = `${widths[i]}px`));
  table.style.width = `${widths.reduce((a, b) => a + b, 0)}px`;
  table.style.tableLayout = "fixed";
}

function resize(th: HTMLTableCellElement, width: number) {
  const table = th.closest("table");
  if (!table) return;
  freeze(table);
  const before = th.getBoundingClientRect().width;
  const next = Math.max(MIN, Math.round(width));
  th.style.width = `${next}px`;
  table.style.width = `${parseFloat(table.style.width) + next - before}px`;
}

function reset(th: HTMLTableCellElement) {
  const table = th.closest("table");
  if (!table) return;
  Array.from(table.tHead?.rows[0]?.cells ?? []).forEach((c) => (c.style.width = ""));
  table.style.width = "";
  table.style.tableLayout = "";
}

/** The drag handle on a header cell's right edge. Double-click returns the table to automatic widths. */
export function ColumnResizer() {
  const cell = (el: HTMLElement) => el.closest("th") as HTMLTableCellElement | null;
  const onPointerDown = (e: PointerEvent<HTMLSpanElement>) => {
    const th = cell(e.currentTarget);
    if (e.button !== 0 || !th) return;
    e.preventDefault();
    const el = e.currentTarget;
    const startX = e.clientX;
    const start = th.getBoundingClientRect().width;
    el.setPointerCapture(e.pointerId);
    document.body.classList.add("art-col-resizing");
    const move = (ev: globalThis.PointerEvent) => resize(th, start + ev.clientX - startX);
    const up = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
      document.body.classList.remove("art-col-resizing");
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLSpanElement>) => {
    const th = cell(e.currentTarget);
    if (!th || (e.key !== "ArrowLeft" && e.key !== "ArrowRight")) return;
    e.preventDefault();
    const step = (e.shiftKey ? 48 : 16) * (e.key === "ArrowRight" ? 1 : -1);
    resize(th, th.getBoundingClientRect().width + step);
  };
  return (
    <span
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize column"
      tabIndex={0}
      className="art-col-resize"
      onPointerDown={onPointerDown}
      onKeyDown={onKeyDown}
      onDoubleClick={(e) => {
        const th = cell(e.currentTarget);
        if (th) reset(th);
      }}
      onClick={(e) => e.stopPropagation()}
    />
  );
}
