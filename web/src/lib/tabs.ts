import type { UiTab } from "../api/gen/UiTab";

/** An editor tab. At most one tab is the italic preview tab, which the next preview open replaces. */
export type Tab = { id: string; preview: boolean };

export type TabsState = { tabs: Tab[]; active: string | null };

export type TabsAction =
  /**
   * Open a resource. `preview` reuses the preview tab; a plain open pins an existing preview tab. `beside` puts a new
   * tab right after the active one instead of at the end.
   */
  | { type: "open"; id: string; preview?: boolean; beside?: boolean }
  /** Turn a preview tab into a normal one. */
  | { type: "pin"; id: string }
  /** Close a tab; when it was active, its right neighbour (else the left one) becomes active. */
  | { type: "close"; id: string }
  /** Focus a resource, adding it as a normal tab when it is not open (a deep link or history step). */
  | { type: "activate"; id: string }
  | { type: "restore"; state: TabsState };

export const emptyTabs: TabsState = { tabs: [], active: null };

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
      if (slot >= 0) return { tabs: state.tabs.map((t, i) => (i === slot ? { id, preview: true } : t)), active: id };
      const at = action.beside ? state.tabs.findIndex((t) => t.id === state.active) : -1;
      const tabs =
        at >= 0
          ? [...state.tabs.slice(0, at + 1), { id, preview }, ...state.tabs.slice(at + 1)]
          : [...state.tabs, { id, preview }];
      return { tabs, active: id };
    }
    case "pin": {
      if (!state.tabs.some((t) => t.id === action.id && t.preview)) return state;
      return { ...state, tabs: state.tabs.map((t) => (t.id === action.id ? { ...t, preview: false } : t)) };
    }
    case "close": {
      const i = state.tabs.findIndex((t) => t.id === action.id);
      if (i < 0) return state;
      const tabs = state.tabs.filter((t) => t.id !== action.id);
      const active =
        state.active === action.id ? (tabs.length ? tabs[Math.min(i, tabs.length - 1)].id : null) : state.active;
      return { tabs, active };
    }
    case "activate": {
      if (state.active === action.id && state.tabs.some((t) => t.id === action.id)) return state;
      const tabs = state.tabs.some((t) => t.id === action.id)
        ? state.tabs
        : [...state.tabs, { id: action.id, preview: false }];
      return { tabs, active: action.id };
    }
    case "restore":
      return normalizeTabs(action.state);
  }
}

/** Drop duplicates and extra preview tabs, and point `active` at an open tab. */
export function normalizeTabs(state: TabsState): TabsState {
  const seen = new Set<string>();
  let previewSeen = false;
  const tabs: Tab[] = [];
  for (const t of state.tabs) {
    if (!t || typeof t.id !== "string" || seen.has(t.id)) continue;
    seen.add(t.id);
    const preview: boolean = !!t.preview && !previewSeen;
    previewSeen ||= preview;
    tabs.push({ id: t.id, preview });
  }
  const active = state.active && seen.has(state.active) ? state.active : (tabs[0]?.id ?? null);
  return { tabs, active };
}

export const toUiTabs = (tabs: Tab[]): UiTab[] =>
  tabs.map((t) => ({ id: t.id, preview: t.preview, pinned: !t.preview }));
export const fromUiTabs = (tabs: UiTab[]): Tab[] => tabs.map((t) => ({ id: t.id, preview: !!t.preview }));
