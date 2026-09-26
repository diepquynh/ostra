import { useEffect, useRef } from "react";
import type { Theme } from "../lib/nav";

/**
 * The console built for the site's homepage (`VITE_SHOT=1`, always with `VITE_MOCK=1`). It runs inside an iframe on
 * the same origin, and the page drives which tab is open and the theme.
 */
export const isShot = import.meta.env.VITE_SHOT === "1";

/** The route the memory router starts on: the iframe's `?path=`, limited to workspace routes. */
export function shotEntry(): string {
  const path = new URLSearchParams(location.search).get("path");
  return path?.startsWith("/w/") ? path : "/";
}

type FromPage = { type: "ostra-shot"; tabs?: string[]; open?: string; theme?: Theme };

const toPage = (msg: object) => window.parent.postMessage(msg, location.origin);

/**
 * Opens what the page asks for and reports what the visitor does inside: a tab they switch to, a theme they pick.
 * Nothing is reported before the page's first message, so the shell's own startup does not stop the page's cycle.
 */
export function useShotBridge(
  activeId: string | null,
  open: (id: string) => void,
  theme: Theme,
  setTheme: (t: Theme) => void,
) {
  const asked = useRef<{ open: string | null; theme: Theme | null } | null>(null);
  const openRef = useRef(open);
  openRef.current = open;
  const themeRef = useRef(setTheme);
  themeRef.current = setTheme;

  useEffect(() => {
    if (!isShot || window.parent === window) return;
    const onMessage = (e: MessageEvent<FromPage>) => {
      if (e.source !== window.parent || e.origin !== location.origin || e.data?.type !== "ostra-shot") return;
      const d = e.data;
      asked.current ??= { open: null, theme: null };
      for (const id of d.tabs ?? []) openRef.current(id);
      if (d.open) {
        asked.current.open = d.open;
        openRef.current(d.open);
      }
      if (d.theme === "light" || d.theme === "dark") {
        asked.current.theme = d.theme;
        themeRef.current(d.theme);
      }
    };
    window.addEventListener("message", onMessage);
    toPage({ type: "ostra-shot-ready" });
    return () => window.removeEventListener("message", onMessage);
  }, []);

  useEffect(() => {
    const a = asked.current;
    if (!a || !activeId || activeId === a.open) return;
    a.open = activeId;
    toPage({ type: "ostra-shot-nav", open: activeId });
  }, [activeId]);

  useEffect(() => {
    const a = asked.current;
    if (!a || theme === a.theme) return;
    a.theme = theme;
    toPage({ type: "ostra-shot-theme", theme });
  }, [theme]);
}
