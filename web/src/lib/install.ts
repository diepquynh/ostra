import { useSyncExternalStore } from "react";

type InstallPromptEvent = Event & { prompt(): Promise<void>; userChoice: Promise<{ outcome: string }> };

/** `installed`: running in its own window. `ready`: the browser offered an install prompt. `manual`: neither. */
export type InstallState = "installed" | "ready" | "manual";

let deferred: InstallPromptEvent | null = null;
let installed = false;
const listeners = new Set<() => void>();
const emit = () => {
  for (const l of listeners) l();
};

export const runningAsApp = () =>
  typeof window !== "undefined" &&
  (window.matchMedia?.("(display-mode: standalone)").matches ||
    window.matchMedia?.("(display-mode: window-controls-overlay)").matches ||
    (navigator as { standalone?: boolean }).standalone === true);

/** Call once at startup, because the browser fires `beforeinstallprompt` only once per page load. */
export function watchInstall(): void {
  window.addEventListener("beforeinstallprompt", (e) => {
    e.preventDefault();
    deferred = e as InstallPromptEvent;
    emit();
  });
  window.addEventListener("appinstalled", () => {
    deferred = null;
    installed = true;
    emit();
  });
}

const state = (): InstallState => (installed || runningAsApp() ? "installed" : deferred ? "ready" : "manual");

export function useInstall() {
  const s = useSyncExternalStore((l) => {
    listeners.add(l);
    return () => listeners.delete(l);
  }, state);
  return {
    state: s,
    /** Shows the browser's install dialog; false when there is none to show. */
    install: async (): Promise<boolean> => {
      if (!deferred) return false;
      const e = deferred;
      deferred = null;
      await e.prompt();
      const { outcome } = await e.userChoice;
      if (outcome !== "accepted") deferred = e;
      emit();
      return true;
    },
  };
}

/** How to install where the browser gives the page no prompt. */
export function manualInstallHint(): string {
  if (typeof window !== "undefined" && !window.isSecureContext)
    return "Browsers install apps only from an HTTPS address or localhost. Open Ostra through one of those to install it.";
  const ua = navigator.userAgent;
  if (/Firefox\//.test(ua))
    return "Firefox does not install web apps. Open Ostra in Chrome, Edge, or Safari to install it.";
  if (/Safari\//.test(ua) && !/Chrome\/|Chromium\/|Edg\//.test(ua))
    return /iPhone|iPad/.test(ua)
      ? "In Safari, tap Share, then Add to Home Screen."
      : "In Safari, choose File, then Add to Dock.";
  return "Use the install icon at the right of the address bar, or the browser menu's Install Ostra item. If neither shows, Ostra may already be installed in this browser.";
}
