import { createContext, useContext } from "react";
import type { WorkspaceDetail } from "../api/types";
import { resourcePath } from "./resource";
import type { Tab } from "./tabs";

export type OpenOptions = {
  /** Open in the italic preview tab, which the next preview open replaces. Default: a normal tab. */
  preview?: boolean;
  /** Element id to scroll to, sent as the URL hash (for example `gate-<id>`). */
  anchor?: string;
  /** Put a new tab right after the active one instead of at the end. */
  beside?: boolean;
};

/** Tab navigation inside the workspace shell. Every screen opens resources through this. */
export interface Nav {
  ws: string;
  activeId: string | null;
  tabs: Tab[];
  /** Open a resource id (`session:<id>`, `file:<key>:<path>`, ...) and focus its tab. */
  open: (id: string, opts?: OpenOptions) => void;
  close: (id: string) => void;
  /** Turn the preview tab into a normal tab. */
  keep: (id: string) => void;
  /** The URL of a resource, for real links (middle-click, copy link). */
  href: (id: string) => string;
}

export type Theme = "light" | "dark";

/** Shell actions a screen may trigger: the dock, dialogs, the Files tab, the theme. */
export interface ShellActions {
  /** Open the quick-question dock, optionally with a question typed in. */
  openDock: (question?: string) => void;
  closeDock: () => void;
  /** Text for the New task form, set by "Turn into task". Screens clear it once used. */
  taskDraft: string | null;
  setTaskDraft: (text: string | null) => void;
  newWorkspace: () => void;
  addProject: () => void;
  /** Show the first-run setup again. */
  runSetup: () => void;
  /** Switch the left dock to the Files tab on a project. */
  browseFiles: (key: string) => void;
  theme: Theme;
  toggleTheme: () => void;
}

export interface WorkspaceData {
  /** The workspace detail, refreshed on `workspace_updated`. Null while loading. */
  detail: WorkspaceDetail | null;
  reload: () => void;
}

export type ConsoleContextValue = { nav: Nav; shell: ShellActions; workspace: WorkspaceData };

const noop = () => {};

export const ConsoleContext = createContext<ConsoleContextValue>({
  nav: { ws: "", activeId: null, tabs: [], open: noop, close: noop, keep: noop, href: (id) => resourcePath("", id) },
  shell: {
    openDock: noop,
    closeDock: noop,
    taskDraft: null,
    setTaskDraft: noop,
    newWorkspace: noop,
    addProject: noop,
    runSetup: noop,
    browseFiles: noop,
    theme: "dark",
    toggleTheme: noop,
  },
  workspace: { detail: null, reload: noop },
});

export const useNav = (): Nav => useContext(ConsoleContext).nav;
export const useShell = (): ShellActions => useContext(ConsoleContext).shell;
export const useWorkspace = (): WorkspaceData => useContext(ConsoleContext).workspace;
