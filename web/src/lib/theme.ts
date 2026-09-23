import type { Theme } from "./nav";

const KEY = "ostra.theme";

/** Null follows the system setting; the design's default is dark. */
export function resolveTheme(stored: string | null | undefined): Theme {
  if (stored === "light" || stored === "dark") return stored;
  try {
    return window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
  } catch {
    return "dark";
  }
}

export function lastTheme(): string | null {
  try {
    return localStorage.getItem(KEY);
  } catch {
    return null;
  }
}

/** Set `data-theme` on <html> and remember it for pages outside a workspace. */
export function applyTheme(theme: Theme, remember = true): void {
  document.documentElement.dataset.theme = theme;
  if (!remember) return;
  try {
    localStorage.setItem(KEY, theme);
  } catch {
    // Storage may be unavailable; the theme then lasts for this page only.
  }
}
