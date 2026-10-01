/** True on macOS and iOS, where shortcuts use ⌘ instead of Ctrl. */
export const isMac =
  typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

/** True on Android, where Chrome keeps Ctrl+Tab and Ctrl+W for itself and an unhandled Esc goes back. */
export const isAndroid =
  typeof navigator !== "undefined" &&
  (/Android/.test(navigator.userAgent) ||
    (navigator as { userAgentData?: { platform?: string } }).userAgentData?.platform === "Android");

/** The modifier key cap: ⌘ on a Mac, Ctrl elsewhere. */
export const MOD = isMac ? "⌘" : "Ctrl";

/** Key caps for a shortcut, for `Kbd keys={...}`. */
export const modKeys = (...keys: string[]) => [MOD, ...keys];

/** A shortcut as one string, for hints: `⌘/` on a Mac, `Ctrl+/` elsewhere. */
export const modHint = (...keys: string[]) => (isMac ? [MOD, ...keys].join("") : [MOD, ...keys].join("+"));
