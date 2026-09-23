import type { WorkspaceUiState } from "../api/types";
import { fromUiTabs, normalizeTabs, toUiTabs, type TabsState } from "../lib/tabs";

export type LeftTab = "sessions" | "files";

/** Shell layout besides the tabs. */
export type UiPrefs = {
  leftTab: LeftTab;
  filesProject: string | null;
  sidebarOpen: boolean;
  dockOpen: boolean;
  /** `light`, `dark`, or null to follow the system. */
  theme: string | null;
};

export type ShellUi = { tabs: TabsState; prefs: UiPrefs };

export const DEFAULT_PREFS: UiPrefs = { leftTab: "sessions", filesProject: null, sidebarOpen: true, dockOpen: false, theme: null };

const key = (ws: string) => `ostra.ui.${ws}`;

export function toServerUi(ui: ShellUi): WorkspaceUiState {
  return {
    tabs: toUiTabs(ui.tabs.tabs),
    active: ui.tabs.active,
    left_tab: ui.prefs.leftTab,
    files_project: ui.prefs.filesProject,
    sidebar_open: ui.prefs.sidebarOpen,
    dock_open: ui.prefs.dockOpen,
    theme: ui.prefs.theme,
  };
}

/** Null when the stored state is empty (the server's default before the first PATCH). */
export function fromServerUi(s: WorkspaceUiState | null | undefined): ShellUi | null {
  if (!s || (s.tabs.length === 0 && s.active === null && s.left_tab === null && s.theme === null)) return null;
  return {
    tabs: normalizeTabs({ tabs: fromUiTabs(s.tabs ?? []), active: s.active }),
    prefs: {
      leftTab: s.left_tab === "files" ? "files" : "sessions",
      filesProject: s.files_project ?? null,
      sidebarOpen: s.sidebar_open ?? true,
      dockOpen: !!s.dock_open,
      theme: s.theme === "light" || s.theme === "dark" ? s.theme : null,
    },
  };
}

/** The layout this browser last used for a workspace. localStorage is the fast path; the server is the fallback. */
export function loadLocalUi(ws: string): ShellUi | null {
  try {
    const raw = localStorage.getItem(key(ws));
    return raw ? fromServerUi(JSON.parse(raw) as WorkspaceUiState) : null;
  } catch {
    return null;
  }
}

export function saveLocalUi(ws: string, ui: ShellUi): void {
  try {
    localStorage.setItem(key(ws), JSON.stringify(toServerUi(ui)));
  } catch {
    // Storage may be full or unavailable; the server copy still restores the layout.
  }
}

/** The tab a workspace last showed, so switching workspaces lands where the user left it. */
export function lastActive(ws: string): string | null {
  return loadLocalUi(ws)?.tabs.active ?? null;
}
