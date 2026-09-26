import { StatusDot, type Tone } from "@ostra/design";
import { useCallback, useEffect, useRef, useState } from "react";
import type { Theme } from "../shared/theme";

// The web app's mock workspace (web/src/api/mock/fixtures.ts), built into console/ by `npm run build:console`.
const WORKSPACE = "ws_demo";
const SHOTS: { open: string; label: string; tone: Tone }[] = [
  { open: "session:s_refund", label: "Refund webhook retries", tone: "accent" },
  { open: "session:s_demo", label: "Order cancellation", tone: "warn" },
  { open: "exec:x_imp2", label: "Implementer · phase 2", tone: "ok" },
  { open: "session:s_init", label: "Initialize web", tone: "accent" },
];
const CYCLE_MS = 6000;
const SRC = `console/index.html?path=${encodeURIComponent(`/w/${WORKSPACE}/s/s_refund`)}`;

type FromShot =
  | { type: "ostra-shot-ready" }
  | { type: "ostra-shot-nav"; open: string }
  | { type: "ostra-shot-theme"; theme: Theme };

/**
 * The real console, built with mock data, cycling through a few screens. Hovering pauses it; clicking a screen or
 * working inside the console stops it.
 */
export function ConsoleShot({ theme, setTheme }: { theme: Theme; setTheme: (t: Theme) => void }) {
  const frame = useRef<HTMLIFrameElement>(null);
  const [index, setIndex] = useState(0);
  const [stopped, setStopped] = useState(false);
  const [hover, setHover] = useState(false);
  const [ready, setReady] = useState(false);
  const sentTabs = useRef(false);

  const send = useCallback((open: string, t: Theme) => {
    const w = frame.current?.contentWindow;
    if (!w) return;
    const tabs = sentTabs.current ? undefined : SHOTS.map((s) => s.open);
    sentTabs.current = true;
    w.postMessage({ type: "ostra-shot", tabs, open, theme: t }, location.origin);
  }, []);

  useEffect(() => {
    const onMessage = (e: MessageEvent<FromShot>) => {
      if (e.source !== frame.current?.contentWindow || e.origin !== location.origin) return;
      const d = e.data;
      if (d?.type === "ostra-shot-ready") {
        sentTabs.current = false;
        setReady(true);
      } else if (d?.type === "ostra-shot-nav") {
        const i = SHOTS.findIndex((s) => s.open === d.open);
        if (i >= 0) setIndex(i);
        setStopped(true);
      } else if (d?.type === "ostra-shot-theme" && (d.theme === "light" || d.theme === "dark")) {
        setTheme(d.theme);
      }
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [setTheme]);

  useEffect(() => {
    if (ready) send(SHOTS[index].open, theme);
  }, [ready, index, theme, send]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: each step re-arms the timer, so `index` must stay.
  useEffect(() => {
    const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    if (stopped || hover || reduced) return;
    const t = setTimeout(() => setIndex((i) => (i + 1) % SHOTS.length), CYCLE_MS);
    return () => clearTimeout(t);
  }, [index, stopped, hover]);

  return (
    <section className="home-section home-section--shot">
      <div className="home-shot" onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)}>
        <iframe ref={frame} src={SRC} title="The Ostra console" loading="lazy" className="home-shot__frame" />
      </div>
      <div className="home-shot__steps">
        {SHOTS.map((s, i) => (
          <button
            key={s.open}
            type="button"
            aria-pressed={i === index}
            className="home-shot__step"
            onClick={() => {
              setIndex(i);
              setStopped(true);
            }}
          >
            <StatusDot tone={s.tone} />
            {s.label}
          </button>
        ))}
      </div>
    </section>
  );
}
