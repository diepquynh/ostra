import { createContext, useContext, useEffect } from "react";

export type HeaderOverride = { title: string; sub?: string };

export const HeaderContext = createContext<(h: HeaderOverride | null) => void>(() => {});

/** Show `title` (and `sub`) in the mobile header while the calling screen is mounted; null keeps the default. */
export function useMobileHeader(title: string | null | undefined, sub?: string) {
  const set = useContext(HeaderContext);
  useEffect(() => {
    set(title ? { title, sub } : null);
    return () => set(null);
  }, [set, title, sub]);
}
