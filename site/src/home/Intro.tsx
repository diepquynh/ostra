import { LiveMark, MARK_ARCS, type MarkState } from "@ostra/design";
import { type CSSProperties, type RefObject, useEffect, useRef, useState } from "react";
import { reducedMotion, startReveal } from "./reveal";

type Phase = "pre" | "in" | "out" | "done";

const SIZE = 88;

/**
 * The page opens on the mark drawing its eight arcs, one per pipeline stage, then lighting the pearl. The mark then
 * flies into the nav slot `navMark` while the backdrop fades and the page reveals under it. A key, the wheel, a touch
 * or a click skips to the flight. Reduced motion skips the splash.
 */
export function Intro({
  navMark,
  onHandoff,
  onLanded,
}: {
  navMark: RefObject<HTMLElement | null>;
  onHandoff: () => void;
  onLanded: () => void;
}) {
  const [phase, setPhase] = useState<Phase>("pre");
  const [filled, setFilled] = useState(0);
  const [pearl, setPearl] = useState(false);
  const [fly, setFly] = useState<string | null>(null);
  const markEl = useRef<HTMLDivElement>(null);
  const skip = useRef<() => void>(() => {});

  // biome-ignore lint/correctness/useExhaustiveDependencies: the intro runs once per page load.
  useEffect(() => {
    let stopReveal = () => {};
    if (reducedMotion()) {
      setPhase("done");
      onHandoff();
      onLanded();
      stopReveal = startReveal(true);
      return () => stopReveal();
    }
    const root = document.documentElement;
    const timers: ReturnType<typeof setTimeout>[] = [];
    const at = (ms: number, fn: () => void) => timers.push(setTimeout(fn, ms));
    const events = ["keydown", "wheel", "touchstart"] as const;
    let handed = false;
    const unlisten = () => {
      for (const e of events) window.removeEventListener(e, handoff);
    };
    function handoff() {
      if (handed) return;
      handed = true;
      for (const t of timers.splice(0)) clearTimeout(t);
      unlisten();
      root.style.overflow = "";
      const a = markEl.current?.getBoundingClientRect();
      const b = navMark.current?.getBoundingClientRect();
      const to =
        a?.width && b?.width
          ? `translate(${b.left + b.width / 2 - a.left - a.width / 2}px, ${b.top + b.height / 2 - a.top - a.height / 2}px) scale(${b.width / a.width})`
          : null;
      setFly(to);
      setFilled(MARK_ARCS);
      setPearl(true);
      setPhase("out");
      stopReveal = startReveal(false);
      onHandoff();
      if (to) at(1000, onLanded);
      else onLanded();
      at(to ? 1060 : 900, () => setPhase("done"));
    }
    skip.current = handoff;

    try {
      history.scrollRestoration = "manual";
    } catch {
      // Some embedded browsers refuse it; the page then keeps its restored scroll.
    }
    window.scrollTo(0, 0);
    root.style.overflow = "hidden";
    for (const e of events) window.addEventListener(e, handoff, { passive: true });
    at(80, () => setPhase("in"));
    for (let i = 0; i < MARK_ARCS; i++) at(950 + i * 230, () => setFilled(i + 1));
    at(950 + MARK_ARCS * 230 + 180, () => setPearl(true));
    at(3620, handoff);
    return () => {
      for (const t of timers) clearTimeout(t);
      unlisten();
      root.style.overflow = "";
      stopReveal();
    };
  }, []);

  if (phase === "done") return null;
  const out = phase === "out";
  const state: MarkState = {
    arcs: Array.from({ length: MARK_ARCS }, (_, i) => (i < filled ? "done" : "off")),
    pearl: pearl ? "rest" : "off",
  };
  const markStyle: CSSProperties = {
    opacity: phase === "pre" || (out && !fly) ? 0 : 1,
    transform: phase === "pre" ? "scale(0.9)" : out && fly ? fly : out ? "scale(1.08)" : "none",
    transition: out && fly ? "transform 1s cubic-bezier(0.7, 0, 0.2, 1)" : undefined,
  };
  return (
    <div aria-hidden="true" className="home-intro" data-phase={phase} onClick={() => skip.current()}>
      <div className="home-intro__backdrop" />
      <div ref={markEl} className="home-intro__mark" style={markStyle}>
        <LiveMark state={state} size={SIZE} draw="0.3s" />
      </div>
    </div>
  );
}
