import { IDLE, LiveMark, MARK_ARCS, type MarkState } from "@ostra/design";
import { useEffect, useState } from "react";

type Phase = "pre" | "in" | "out" | "done";

const reducedMotion = () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

/** Fades in each `[data-reveal]` element as it scrolls into view, a short stagger apart within one batch. */
function startReveal(instant: boolean): () => void {
  const els = Array.from(document.querySelectorAll<HTMLElement>("[data-reveal]"));
  if (instant || !("IntersectionObserver" in window)) {
    for (const el of els) el.dataset.in = "";
    return () => {};
  }
  const io = new IntersectionObserver(
    (entries) => {
      let n = 0;
      for (const en of entries) {
        if (!en.isIntersecting) continue;
        const el = en.target as HTMLElement;
        el.style.transitionDelay = `${n++ * 110}ms`;
        el.dataset.in = "";
        io.unobserve(el);
      }
    },
    { rootMargin: "0px 0px -8% 0px", threshold: 0.08 },
  );
  for (const el of els) io.observe(el);
  return () => io.disconnect();
}

/**
 * The page opens on the mark filling its eight arcs, one per pipeline stage, then lighting the pearl. The page
 * underneath reveals as the splash fades. Reduced motion skips both.
 */
export function Intro() {
  const [phase, setPhase] = useState<Phase>("pre");
  const [filled, setFilled] = useState(0);
  const [pearl, setPearl] = useState(false);

  useEffect(() => {
    if (reducedMotion()) {
      setPhase("done");
      return startReveal(true);
    }
    const root = document.documentElement;
    let stopReveal = () => {};
    const timers: ReturnType<typeof setTimeout>[] = [];
    const at = (ms: number, fn: () => void) => timers.push(setTimeout(fn, ms));
    root.style.overflow = "hidden";
    at(60, () => setPhase("in"));
    for (let i = 0; i < MARK_ARCS; i++) at(800 + i * 180, () => setFilled(i + 1));
    at(2250, () => setPearl(true));
    at(2800, () => {
      setPhase("out");
      root.style.overflow = "";
      stopReveal = startReveal(false);
    });
    at(3600, () => setPhase("done"));
    return () => {
      for (const t of timers) clearTimeout(t);
      root.style.overflow = "";
      stopReveal();
    };
  }, []);

  if (phase === "done") return null;
  const state: MarkState = {
    arcs: IDLE.arcs.map((_, i) => (i < filled ? "done" : "off")),
    pearl: pearl ? "rest" : "off",
  };
  return (
    <div aria-hidden="true" className="home-intro" data-phase={phase}>
      <div className="home-intro__mark">
        <LiveMark state={state} size={72} />
      </div>
    </div>
  );
}
