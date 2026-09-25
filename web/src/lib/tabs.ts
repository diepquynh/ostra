import type { UiTab } from "../api/gen/UiTab";

/**
 * An editor tab. At most one tab is the italic preview tab, which the next preview open replaces. Pinned tabs sit
 * before the others, are never the preview tab, and survive the bulk closes.
 */
export type Tab = { id: string; preview: boolean; pinned: boolean };

/** Which tabs a bulk close removes, relative to the tab it was asked on. Pinned tabs always stay. */
export type CloseScope = "all" | "others" | "left" | "right";

export type TabsState = { tabs: Tab[]; active: string | null };

export type TabsAction =
  /**
   * Open a resource. `preview` reuses the preview tab; a plain open pins an existing preview tab. `beside` puts a new
   * tab right after the active one instead of at the end.
   */
  | { type: "open"; id: string; preview?: boolean; beside?: boolean }
  /** Turn a preview tab into a normal one. */
  | { type: "keep"; id: string }
  /** Pin moves the tab to the end of the pinned group; unpin moves it to the start of the others. */
  | { type: "setPinned"; id: string; pinned: boolean }
  /** Close a tab; when it was active, its right neighbour (else the left one) becomes active. */
  | { type: "close"; id: string }
  /** Close unpinned tabs; when the active tab goes, `id` (else the first remaining tab) becomes active. */
  | { type: "closeMany"; scope: CloseScope; id: string }
  /**
   * Move a tab so it sits at index `to` of the strip. It stays within its group, pinned or not, and a moved preview tab
   * becomes a normal one.
   */
  | { type: "move"; id: string; to: number }
  /** Focus a resource, adding it as a normal tab when it is not open (a deep link or history step). */
  | { type: "activate"; id: string }
  | { type: "restore"; state: TabsState };

export const emptyTabs: TabsState = { tabs: [], active: null };

const pinnedCount = (tabs: Tab[]) => tabs.filter((t) => t.pinned).length;

export function tabsReducer(state: TabsState, action: TabsAction): TabsState {
  switch (action.type) {
    case "open": {
      const { id } = action;
      const preview = !!action.preview;
      const existing = state.tabs.find((t) => t.id === id);
      if (existing) {
        const tabs =
          !preview && existing.preview
            ? state.tabs.map((t) => (t.id === id ? { ...t, preview: false } : t))
            : state.tabs;
        return tabs === state.tabs && state.active === id ? state : { tabs, active: id };
      }
      const slot = preview ? state.tabs.findIndex((t) => t.preview) : -1;
      const tab: Tab = { id, preview, pinned: false };
      if (slot >= 0) return { tabs: state.tabs.map((t, i) => (i === slot ? tab : t)), active: id };
      const at = action.beside
        ? Math.max(
            state.tabs.findIndex((t) => t.id === state.active),
            pinnedCount(state.tabs) - 1,
          )
        : -1;
      const tabs = at >= 0 ? [...state.tabs.slice(0, at + 1), tab, ...state.tabs.slice(at + 1)] : [...state.tabs, tab];
      return { tabs, active: id };
    }
    case "keep": {
      if (!state.tabs.some((t) => t.id === action.id && t.preview)) return state;
      return { ...state, tabs: state.tabs.map((t) => (t.id === action.id ? { ...t, preview: false } : t)) };
    }
    case "setPinned": {
      const tab = state.tabs.find((t) => t.id === action.id);
      if (!tab || tab.pinned === action.pinned) return state;
      const rest = state.tabs.filter((t) => t.id !== action.id);
      const at = pinnedCount(rest);
      const moved: Tab = { id: tab.id, preview: false, pinned: action.pinned };
      return { ...state, tabs: [...rest.slice(0, at), moved, ...rest.slice(at)] };
    }
    case "close": {
      const i = state.tabs.findIndex((t) => t.id === action.id);
      if (i < 0) return state;
      const tabs = state.tabs.filter((t) => t.id !== action.id);
      const active =
        state.active === action.id ? (tabs.length ? tabs[Math.min(i, tabs.length - 1)].id : null) : state.active;
      return { tabs, active };
    }
    case "closeMany": {
      const at = state.tabs.findIndex((t) => t.id === action.id);
      const { scope } = action;
      const tabs = state.tabs.filter(
        (t, i) =>
          t.pinned ||
          (scope === "others" && i === at) ||
          (scope === "left" && i >= at) ||
          (scope === "right" && i <= at),
      );
      if (tabs.length === state.tabs.length) return state;
      const open = (id: string | null) => id !== null && tabs.some((t) => t.id === id);
      const active = open(state.active) ? state.active : open(action.id) ? action.id : (tabs[0]?.id ?? null);
      return { tabs, active };
    }
    case "move": {
      const from = state.tabs.findIndex((t) => t.id === action.id);
      if (from < 0) return state;
      const tab = state.tabs[from];
      const rest = state.tabs.filter((_, i) => i !== from);
      const pins = pinnedCount(rest);
      const [lo, hi] = tab.pinned ? [0, pins] : [pins, rest.length];
      const to = Math.max(lo, Math.min(hi, action.to));
      if (to === from && !tab.preview) return state;
      const moved: Tab = { ...tab, preview: false };
      return { ...state, tabs: [...rest.slice(0, to), moved, ...rest.slice(to)] };
    }
    case "activate": {
      if (state.active === action.id && state.tabs.some((t) => t.id === action.id)) return state;
      const tabs = state.tabs.some((t) => t.id === action.id)
        ? state.tabs
        : [...state.tabs, { id: action.id, preview: false, pinned: false }];
      return { tabs, active: action.id };
    }
    case "restore":
      return normalizeTabs(action.state);
  }
}

/** Drop duplicates and extra preview tabs, put pinned tabs first, and point `active` at an open tab. */
export function normalizeTabs(state: TabsState): TabsState {
  const seen = new Set<string>();
  let previewSeen = false;
  const pinned: Tab[] = [];
  const others: Tab[] = [];
  for (const t of state.tabs) {
    if (!t || typeof t.id !== "string" || seen.has(t.id)) continue;
    seen.add(t.id);
    if (t.pinned) {
      pinned.push({ id: t.id, preview: false, pinned: true });
      continue;
    }
    const preview: boolean = !!t.preview && !previewSeen;
    previewSeen ||= preview;
    others.push({ id: t.id, preview, pinned: false });
  }
  const tabs = [...pinned, ...others];
  const active = state.active && seen.has(state.active) ? state.active : (tabs[0]?.id ?? null);
  return { tabs, active };
}

export const toUiTabs = (tabs: Tab[]): UiTab[] => tabs.map((t) => ({ id: t.id, preview: t.preview, pinned: t.pinned }));
export const fromUiTabs = (tabs: UiTab[]): Tab[] =>
  tabs.map((t) => ({ id: t.id, preview: !!t.preview, pinned: !!t.pinned }));
