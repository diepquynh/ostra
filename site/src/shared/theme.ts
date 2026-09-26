import { markSvg, REST } from "@ostra/design";
import { useCallback, useEffect, useState } from "react";

export type Theme = "light" | "dark";

const KEY = "ostra-home-theme";

function initialTheme(): Theme {
  try {
    const t = localStorage.getItem(KEY);
    if (t === "light" || t === "dark") return t;
  } catch {
    // Storage may be blocked; fall back to the system setting.
  }
  return window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

/** The page theme, shared by the homepage and the docs. It sets `data-theme` and a favicon in the same colors. */
export function useSiteTheme(): [Theme, (t: Theme) => void] {
  const [theme, setThemeState] = useState<Theme>(initialTheme);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    let icon = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
    if (!icon) {
      icon = Object.assign(document.createElement("link"), { rel: "icon", type: "image/svg+xml" });
      document.head.append(icon);
    }
    icon.href = `data:image/svg+xml,${encodeURIComponent(markSvg(REST, theme))}`;
  }, [theme]);
  const setTheme = useCallback((t: Theme) => {
    setThemeState(t);
    try {
      localStorage.setItem(KEY, t);
    } catch {
      // The choice then lasts for this page only.
    }
  }, []);
  return [theme, setTheme];
}
