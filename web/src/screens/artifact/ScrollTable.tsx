import { useEffect, useRef, useState, type ReactNode } from "react";

/**
 * A report table that scrolls sideways. Its scrollbar is a sticky copy pinned to the bottom of the
 * view while the table is on screen, so a tall table can be scrolled without first reaching its end.
 */
export function ScrollTable({ children }: { children: ReactNode }) {
  const box = useRef<HTMLDivElement>(null);
  const bar = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [overflows, setOverflows] = useState(false);
  // The scroll event of the element set programmatically, which must not echo back.
  const echo = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const measure = () => {
      setWidth(el.scrollWidth);
      setOverflows(el.scrollWidth > el.clientWidth + 1);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    if (el.firstElementChild) ro.observe(el.firstElementChild);
    return () => ro.disconnect();
  }, []);

  const sync = (from: HTMLDivElement | null, to: HTMLDivElement | null) => {
    if (!from || !to) return;
    if (echo.current === from) {
      echo.current = null;
      return;
    }
    if (to.scrollLeft !== from.scrollLeft) {
      echo.current = to;
      to.scrollLeft = from.scrollLeft;
    }
  };

  return (
    <div className="art-table-frame">
      <div className="art-table" ref={box} onScroll={() => sync(box.current, bar.current)}>
        {children}
      </div>
      <div className="art-hscroll" ref={bar} hidden={!overflows} aria-hidden="true" onScroll={() => sync(bar.current, box.current)}>
        <div style={{ width, height: 1 }} />
      </div>
    </div>
  );
}
