import { useCallback, useEffect, useState } from "react";

type KeyboardLockApi = { lock(keys?: string[]): Promise<void>; unlock(): void };

const keyboard = (): KeyboardLockApi | undefined =>
  typeof navigator === "undefined" ? undefined : (navigator as { keyboard?: KeyboardLockApi }).keyboard;

/** True where the browser has the Keyboard Lock API (Chromium), which works only in fullscreen. */
export const keyboardLockSupported = () =>
  typeof keyboard()?.lock === "function" &&
  typeof document !== "undefined" &&
  typeof document.documentElement.requestFullscreen === "function";

/**
 * Fullscreen plus `navigator.keyboard.lock()`, as vscode.dev does it: while locked, combos the browser reserves
 * (Ctrl+Tab, Ctrl+W, Ctrl+N) reach the console first. Holding Escape or leaving fullscreen releases the lock.
 */
export function useKeyboardLock() {
  const [locked, setLocked] = useState(false);

  useEffect(() => {
    const onChange = () => {
      if (document.fullscreenElement) return;
      keyboard()?.unlock();
      setLocked(false);
    };
    document.addEventListener("fullscreenchange", onChange);
    return () => document.removeEventListener("fullscreenchange", onChange);
  }, []);

  const lock = useCallback(async () => {
    const kb = keyboard();
    if (!kb || !keyboardLockSupported()) return;
    const entered = !document.fullscreenElement;
    try {
      if (entered) await document.documentElement.requestFullscreen({ navigationUI: "hide" });
      await kb.lock();
      setLocked(true);
    } catch {
      if (entered && document.fullscreenElement) await document.exitFullscreen().catch(() => {});
    }
  }, []);

  const unlock = useCallback(() => {
    keyboard()?.unlock();
    setLocked(false);
    if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  }, []);

  const toggle = useCallback(() => (locked ? unlock() : lock()), [locked, lock, unlock]);

  return { supported: keyboardLockSupported(), locked, lock, unlock, toggle };
}
