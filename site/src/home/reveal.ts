export const reducedMotion = () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

/** Reveals each `[data-reveal]` element as it scrolls into view, a short stagger apart within one batch. */
export function startReveal(instant: boolean): () => void {
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
