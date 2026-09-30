import { useCallback, useMemo, useSyncExternalStore } from "react";
import { isMac } from "./keys";

export type Action =
  | "palette"
  | "dock"
  | "sidebar"
  | "files"
  | "next-tab"
  | "prev-tab"
  | "close-tab"
  | "new-task"
  | "settings"
  | "theme"
  | "keyboard-lock";

/** One key press with its modifiers, as `ctrl+alt+shift+meta+<key>` with the modifiers in that order. */
export type Stroke = string;
/** One stroke, or two pressed one after the other. */
export type Keys = Stroke[];
/** A bound shortcut. `reserved` marks a default the console claims only while the browser lets it through. */
export type Bound = { keys: Keys; reserved: boolean };
export type Bindings = Record<Action, Bound | null>;
/** What the user changed; an action missing here keeps its default, and `null` unbinds it. */
export type Overrides = Partial<Record<Action, Keys | null>>;

export const ACTIONS: { id: Action; label: string }[] = [
  { id: "palette", label: "Open the command palette" },
  { id: "dock", label: "Toggle the quick-question dock" },
  { id: "sidebar", label: "Toggle the sidebar" },
  { id: "files", label: "Show the Files tab" },
  { id: "next-tab", label: "Next tab" },
  { id: "prev-tab", label: "Previous tab" },
  { id: "close-tab", label: "Close the focused tab" },
  { id: "new-task", label: "New task" },
  { id: "settings", label: "Open settings" },
  { id: "theme", label: "Toggle light and dark theme" },
  { id: "keyboard-lock", label: "Lock or release keyboard shortcuts in fullscreen" },
];

const ACTION_IDS = new Set<string>(ACTIONS.map((a) => a.id));

/** A second stroke must follow the first within this time to count as one sequence. */
export const SEQUENCE_MS = 1500;

export function defaults(mac = isMac): Bindings {
  const mod = mac ? "meta+" : "ctrl+";
  const b = (s: Stroke, reserved = false): Bound => ({ keys: [s], reserved });
  return {
    palette: b(`${mod}k`),
    dock: b(`${mod}/`),
    sidebar: b(`${mod}b`),
    files: b(mac ? "shift+meta+e" : "ctrl+shift+e"),
    "next-tab": b("ctrl+Tab"),
    "prev-tab": b("ctrl+shift+Tab"),
    // ⌘W belongs to the browser tab unless the keyboard is locked or Ostra runs as an installed app.
    "close-tab": b(`${mod}w`, true),
    "new-task": null,
    settings: null,
    theme: null,
    "keyboard-lock": null,
  };
}

export function resolve(overrides: Overrides, mac = isMac): Bindings {
  const out = defaults(mac);
  for (const [id, keys] of Object.entries(overrides) as [Action, Keys | null][])
    out[id] = keys ? { keys, reserved: false } : null;
  return out;
}

// ------------------------------------------------------------------------------------------------
// Strokes
// ------------------------------------------------------------------------------------------------

const MODIFIER_KEYS = new Set(["Control", "Shift", "Alt", "AltGraph", "Meta", "OS", "Hyper", "Super", "Fn"]);
const IGNORED_KEYS = new Set(["Dead", "Unidentified", "Process", "CapsLock", "NumLock", "ScrollLock", ""]);

const CODE_CHARS: Record<string, string> = {
  Slash: "/",
  Backslash: "\\",
  Period: ".",
  Comma: ",",
  Semicolon: ";",
  Quote: "'",
  BracketLeft: "[",
  BracketRight: "]",
  Minus: "-",
  Equal: "=",
  Backquote: "`",
};

function codeKey(code: string): string | null {
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) return letter[1].toLowerCase();
  const digit = /^Digit(\d)$/.exec(code);
  if (digit) return digit[1];
  return CODE_CHARS[code] ?? null;
}

type KeyEventLike = Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey"> & { code?: string };

/**
 * The stroke a key event is, or null for a lone modifier. Shift and Alt change the character a key types (Alt+K
 * types ˚ on a Mac), so with either held, or for a character outside ASCII, the physical key names it instead.
 */
export function strokeOf(e: KeyEventLike): Stroke | null {
  if (MODIFIER_KEYS.has(e.key)) return null;
  let key: string | null;
  if (e.key === " ") key = "Space";
  else if (e.key.length === 1) {
    const ascii = /^[\x21-\x7e]$/.test(e.key);
    key =
      ascii && !e.altKey && !e.shiftKey
        ? e.key.toLowerCase()
        : (codeKey(e.code ?? "") ?? (ascii ? e.key.toLowerCase() : null));
  } else key = IGNORED_KEYS.has(e.key) ? codeKey(e.code ?? "") : e.key;
  if (!key) return null;
  const mods = [e.ctrlKey && "ctrl", e.altKey && "alt", e.shiftKey && "shift", e.metaKey && "meta"].filter(Boolean);
  return [...mods, key].join("+");
}

type Parsed = { ctrl: boolean; alt: boolean; shift: boolean; meta: boolean; key: string };

export function parseStroke(s: Stroke): Parsed | null {
  const p: Parsed = { ctrl: false, alt: false, shift: false, meta: false, key: "" };
  let rest = s;
  for (const m of ["ctrl", "alt", "shift", "meta"] as const)
    if (rest.startsWith(`${m}+`) && rest.length > m.length + 1) {
      p[m] = true;
      rest = rest.slice(m.length + 1);
    }
  if (!rest || MODIFIER_KEYS.has(rest) || rest.length > 24 || (rest.includes("+") && rest !== "+")) return null;
  p.key = rest;
  return p;
}

const isFunctionKey = (key: string) => /^F\d{1,2}$/.test(key);

/** True when a stroke would type text in a field: no Ctrl, Alt, or ⌘, and not a function key. */
export function typesText(s: Stroke): boolean {
  const p = parseStroke(s);
  return !!p && !p.ctrl && !p.alt && !p.meta && !isFunctionKey(p.key);
}

const KEY_LABELS: Record<string, string> = {
  ArrowLeft: "←",
  ArrowRight: "→",
  ArrowUp: "↑",
  ArrowDown: "↓",
  Enter: "↵",
  Escape: "Esc",
  Backspace: "⌫",
  Delete: "Del",
  PageUp: "PgUp",
  PageDown: "PgDn",
};

const keyLabel = (key: string) => KEY_LABELS[key] ?? (key.length === 1 ? key.toUpperCase() : key);

/** Key caps for `Kbd keys={...}`: `["⌘", "K"]` on a Mac, `["Ctrl", "K"]` elsewhere. */
export function strokeCaps(s: Stroke, mac = isMac): string[] {
  const p = parseStroke(s);
  if (!p) return [s];
  const mods = mac
    ? [p.ctrl && "⌃", p.alt && "⌥", p.shift && "⇧", p.meta && "⌘"]
    : [p.ctrl && "Ctrl", p.alt && "Alt", p.shift && "Shift", p.meta && "Meta"];
  return [...(mods.filter(Boolean) as string[]), keyLabel(p.key)];
}

/** A shortcut as one string for hints: `⇧⌘E` on a Mac, `Ctrl+Shift+E` elsewhere, sequences space-separated. */
export function keysHint(keys: Keys, mac = isMac): string {
  return keys.map((s) => strokeCaps(s, mac).join(mac ? "" : "+")).join(" ");
}

// ------------------------------------------------------------------------------------------------
// Matching
// ------------------------------------------------------------------------------------------------

export type Pending = { stroke: Stroke; at: number } | null;
export type Match = { action: Action | null; pending: Pending; consume: boolean };

/**
 * What one stroke does. `claimReserved` is true while the browser passes its own combos to the page (keyboard lock,
 * installed app). A stroke that starts a sequence waits for the next one; one that finishes it runs the action.
 */
export function matchStroke(
  bindings: Bindings,
  pending: Pending,
  stroke: Stroke,
  now: number,
  claimReserved = false,
): Match {
  const live = (Object.entries(bindings) as [Action, Bound | null][]).filter(
    (e): e is [Action, Bound] => !!e[1] && (!e[1].reserved || claimReserved),
  );
  if (pending && now - pending.at <= SEQUENCE_MS) {
    const hit = live.find(([, b]) => b.keys.length === 2 && b.keys[0] === pending.stroke && b.keys[1] === stroke);
    if (hit) return { action: hit[0], pending: null, consume: true };
  }
  if (live.some(([, b]) => b.keys.length === 2 && b.keys[0] === stroke))
    return { action: null, pending: { stroke, at: now }, consume: true };
  const one = live.find(([, b]) => b.keys.length === 1 && b.keys[0] === stroke);
  return { action: one?.[0] ?? null, pending: null, consume: !!one };
}

/** Actions a new binding for `action` would collide with: the same keys, or one's first stroke is the other. */
export function conflicts(bindings: Bindings, action: Action, keys: Keys): Action[] {
  const out: Action[] = [];
  for (const [id, b] of Object.entries(bindings) as [Action, Bound | null][]) {
    if (id === action || !b) continue;
    const same = b.keys.length === keys.length && b.keys.every((s, i) => s === keys[i]);
    const prefix =
      (b.keys.length === 2 && keys.length === 1 && b.keys[0] === keys[0]) ||
      (b.keys.length === 1 && keys.length === 2 && keys[0] === b.keys[0]);
    if (same || prefix) out.push(id);
  }
  return out;
}

const BROWSER_MAC = ["meta+w", "meta+t", "meta+n", "meta+q", "meta+shift+t", "meta+shift+n", "meta+shift+w"];
const BROWSER_OTHER = ["ctrl+w", "ctrl+t", "ctrl+n", "ctrl+shift+t", "ctrl+shift+n", "ctrl+shift+w"];
const BROWSER_ANY = ["ctrl+Tab", "ctrl+shift+Tab", "ctrl+PageUp", "ctrl+PageDown"];
const SYSTEM_MAC = ["meta+Tab", "meta+shift+Tab", "meta+Space", "ctrl+Space", "meta+h", "meta+m", "ctrl+ArrowUp"];
const SYSTEM_OTHER = [
  "alt+Tab",
  "alt+shift+Tab",
  "alt+F4",
  "ctrl+alt+Delete",
  "ctrl+alt+ArrowLeft",
  "ctrl+alt+ArrowRight",
];

/** Who likely handles a stroke before the page does, so the console may never see it. */
export function reservedBy(s: Stroke, mac = isMac): "browser" | "system" | null {
  if ((mac ? SYSTEM_MAC : SYSTEM_OTHER).includes(s)) return "system";
  if (!mac && parseStroke(s)?.meta) return "system";
  if ((mac ? BROWSER_MAC : BROWSER_OTHER).includes(s) || BROWSER_ANY.includes(s)) return "browser";
  return null;
}

// ------------------------------------------------------------------------------------------------
// Storage: one entry per workspace, in this browser only
// ------------------------------------------------------------------------------------------------

const storageKey = (ws: string) => `ostra.shortcuts.${ws}`;
const CHANGED = "ostra-shortcuts";

/** The overrides stored for a workspace. The value is user-editable, so anything malformed is dropped. */
export function parseOverrides(raw: string | null): Overrides {
  if (!raw) return {};
  let data: unknown;
  try {
    data = JSON.parse(raw);
  } catch {
    return {};
  }
  const bindings = (data as { bindings?: unknown } | null)?.bindings;
  if (!bindings || typeof bindings !== "object") return {};
  const out: Overrides = {};
  for (const [id, keys] of Object.entries(bindings)) {
    if (!ACTION_IDS.has(id)) continue;
    if (keys === null) out[id as Action] = null;
    else if (
      Array.isArray(keys) &&
      keys.length >= 1 &&
      keys.length <= 2 &&
      keys.every((s) => typeof s === "string" && parseStroke(s))
    )
      out[id as Action] = keys as Keys;
  }
  return out;
}

const memory = new Map<string, string>();

function read(ws: string): string | null {
  const kept = memory.get(ws);
  if (kept !== undefined) return kept;
  try {
    return localStorage.getItem(storageKey(ws));
  } catch {
    return null;
  }
}

export function saveOverrides(ws: string, overrides: Overrides): void {
  try {
    if (Object.keys(overrides).length === 0) localStorage.removeItem(storageKey(ws));
    else localStorage.setItem(storageKey(ws), JSON.stringify({ v: 1, bindings: overrides }));
  } catch {
    // Storage may be unavailable; the change then lasts until the page reloads.
    memory.set(ws, JSON.stringify({ v: 1, bindings: overrides }));
  }
  window.dispatchEvent(new CustomEvent(CHANGED, { detail: ws }));
}

function subscribe(onChange: () => void) {
  window.addEventListener(CHANGED, onChange);
  // Another browser tab changed them.
  window.addEventListener("storage", onChange);
  return () => {
    window.removeEventListener(CHANGED, onChange);
    window.removeEventListener("storage", onChange);
  };
}

/** The workspace's shortcuts, live across components and browser tabs. */
export function useShortcuts(ws: string) {
  const raw = useSyncExternalStore(subscribe, () => read(ws));
  const overrides = useMemo(() => parseOverrides(raw), [raw]);
  const bindings = useMemo(() => resolve(overrides), [overrides]);
  /** Merges changes into the stored overrides; `undefined` puts an action back to its default. */
  const set = useCallback(
    (changes: Overrides) => {
      const next = { ...parseOverrides(read(ws)), ...changes };
      for (const id of Object.keys(next) as Action[]) if (next[id] === undefined) delete next[id];
      saveOverrides(ws, next);
    },
    [ws],
  );
  const reset = useCallback(
    (action?: Action) => (action ? set({ [action]: undefined }) : saveOverrides(ws, {})),
    [ws, set],
  );
  return { bindings, overrides, set, reset };
}

/** Hint strings for bound actions, for menus and the palette. */
export function hints(bindings: Bindings, mac = isMac): Partial<Record<Action, string>> {
  const out: Partial<Record<Action, string>> = {};
  for (const [id, b] of Object.entries(bindings) as [Action, Bound | null][]) if (b) out[id] = keysHint(b.keys, mac);
  return out;
}
