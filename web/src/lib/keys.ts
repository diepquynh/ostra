/** True on macOS and iOS, where shortcuts use ⌘ instead of Ctrl. */
export const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

/** The modifier key cap: ⌘ on a Mac, Ctrl elsewhere. */
export const MOD = isMac ? "⌘" : "Ctrl";

/** Key caps for a shortcut, for `Kbd keys={...}`. */
export const modKeys = (...keys: string[]) => [MOD, ...keys];

/** A shortcut as one string, for hints: `⌘/` on a Mac, `Ctrl+/` elsewhere. */
export const modHint = (...keys: string[]) => (isMac ? [MOD, ...keys].join("") : [MOD, ...keys].join("+"));

export type Shortcut = "palette" | "dock" | "sidebar" | "files";

/** Which console shortcut a key event is: ⌘K, ⌘/, ⌘B, ⇧⌘E (Ctrl on other systems). */
export function shortcutOf(e: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey">, mac = isMac): Shortcut | null {
  const mod = mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!mod || e.altKey) return null;
  const key = e.key.toLowerCase();
  if (e.shiftKey) return key === "e" ? "files" : null;
  if (key === "k") return "palette";
  if (key === "/") return "dock";
  if (key === "b") return "sidebar";
  return null;
}
