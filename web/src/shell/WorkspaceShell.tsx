import { Button, Kbd, LiveMark, Menu, type MenuItem, REST, StatusDot, Tabs, type Tone } from "@ostra/design";
import {
  type CSSProperties,
  type MouseEvent,
  useCallback,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from "react";
import { Outlet, useLocation, useNavigate, useParams } from "react-router";
import { api } from "../api";
import type { SessionStatus } from "../api/types";
import { useAsync, useChannel } from "../lib/hooks";
import { useKeyboardLock } from "../lib/keyboardLock";
import { isMac, modKeys, shortcutOf } from "../lib/keys";
import { useSessionSummaries, useWorkspaceTree } from "../lib/live";
import { ConsoleContext, type ConsoleContextValue, type OpenOptions, type Theme } from "../lib/nav";
import { paletteTarget } from "../lib/palette";
import { parseResource, resourceFromPath, resourcePath } from "../lib/resource";
import { type CloseScope, emptyTabs, normalizeTabs, type TabsState, tabsReducer } from "../lib/tabs";
import { applyTheme, resolveTheme } from "../lib/theme";
import { AddProjectDialog, NewWorkspaceDialog, Onboarding, selfScrolling } from "../screens";
import { PendingCommandsNotice } from "../screens/workspace/PendingCommands";
import { markState, useLiveFavicon } from "./markState";
import { resourceMeta } from "./meta";
import { Palette } from "./Palette";
import { QuickDock } from "./QuickDock";
import { ResizeHandle, usePanelWidths } from "./ResizeHandle";
import { Sidebar } from "./Sidebar";
import { StatusBar } from "./StatusBar";
import { isShot, useShotBridge } from "./shot";
import { TitleBar } from "./TitleBar";
import { DEFAULT_PREFS, fromServerUi, loadLocalUi, saveLocalUi, toServerUi, type UiPrefs } from "./uiState";
import "./shell.css";
import { ARTIFACTS_ROOT } from "../features/context/tags";
import { MobileShell } from "../mobile/MobileShell";
import { useIsMobile } from "../mobile/useMobile";

const TAB_TONE: Record<string, Tone> = {
  running: "accent",
  waiting: "warn",
  completed: "ok",
  stalled: "bad",
  failed: "bad",
};
const PATCH_DELAY_MS = 800;

/** The console for one workspace: title bar, left dock, editor tabs, quick-question dock, status bar. */
export function WorkspaceShell() {
  const { ws = "" } = useParams();
  const mobile = useIsMobile();
  // Remount per workspace so tabs, stores and dialogs never leak across workspaces.
  return mobile ? <MobileShell key={ws} ws={ws} /> : <Shell key={ws} ws={ws} />;
}

function initialTabs(local: TabsState | null, routeId: string | null): TabsState {
  const s = normalizeTabs(local ?? emptyTabs);
  return routeId ? tabsReducer(s, { type: "activate", id: routeId }) : s;
}

function Shell({ ws }: { ws: string }) {
  const location = useLocation();
  const navigate = useNavigate();
  const routeId = useMemo(
    () => resourceFromPath(location.pathname, location.search)?.id ?? null,
    [location.pathname, location.search],
  );

  // The homepage shot starts from a clean strip; its page opens the tabs.
  const [local] = useState(() => (isShot ? null : loadLocalUi(ws)));
  const [tabs, dispatch] = useReducer(tabsReducer, null, () => initialTabs(local?.tabs ?? null, routeId));
  const [prefs, setPrefs] = useState<UiPrefs>(() => local?.prefs ?? DEFAULT_PREFS);
  const [widths, setWidth] = usePanelWidths();
  const setPref = useCallback(<K extends keyof UiPrefs>(k: K, v: UiPrefs[K] | ((p: UiPrefs[K]) => UiPrefs[K])) => {
    setPrefs((p) => ({ ...p, [k]: typeof v === "function" ? (v as (x: UiPrefs[K]) => UiPrefs[K])(p[k]) : v }));
  }, []);

  // Server copy of the layout: restores it in a browser that has no local copy, then receives every change.
  const serverReady = useRef(false);
  useEffect(() => {
    if (isShot) return;
    let alive = true;
    api.uiState(ws).then(
      (s) => {
        if (!alive) return;
        const remote = fromServerUi(s);
        if (!local && remote) {
          dispatch({ type: "restore", state: remote.tabs });
          if (routeId) dispatch({ type: "activate", id: routeId });
          setPrefs(remote.prefs);
        }
        serverReady.current = true;
      },
      () => {
        serverReady.current = true;
      },
    );
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ws]);
  useEffect(() => {
    const ui = { tabs, prefs };
    saveLocalUi(ws, ui);
    if (!serverReady.current) return;
    const t = setTimeout(() => void api.patchUiState(ws, toServerUi(ui)).catch(() => {}), PATCH_DELAY_MS);
    return () => clearTimeout(t);
  }, [ws, tabs, prefs]);

  // Vertical wheels scroll the tab strip sideways. Native listener because React's onWheel is passive.
  const stripEl = useRef<HTMLDivElement | null>(null);
  const tabStrip = useCallback((el: HTMLDivElement | null) => {
    stripEl.current = el;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (e.deltaY === 0 || Math.abs(e.deltaX) > Math.abs(e.deltaY) || el.scrollWidth <= el.clientWidth) return;
      e.preventDefault();
      el.scrollLeft += e.deltaY;
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  // Keep the active tab on screen, so a tab opened past the edge of the strip does not need scrolling to.
  useEffect(() => {
    if (!tabs.active) return;
    const id = requestAnimationFrame(() =>
      stripEl.current
        ?.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')
        ?.scrollIntoView?.({ block: "nearest", inline: "nearest" }),
    );
    return () => cancelAnimationFrame(id);
  }, [tabs]);

  // URL and active tab follow each other. `shellNav` marks a URL change the shell made itself.
  const shellNav = useRef<string | null>(null);
  const pendingAnchor = useRef<string | null>(null);
  useEffect(() => {
    if (!routeId) return;
    if (shellNav.current === routeId) {
      shellNav.current = null;
      return;
    }
    dispatch({ type: "activate", id: routeId });
  }, [routeId]);
  useEffect(() => {
    const target = tabs.active ?? "ws:overview";
    const anchor = pendingAnchor.current;
    pendingAnchor.current = null;
    if (target === routeId && !anchor) return;
    shellNav.current = target;
    navigate(resourcePath(ws, target, anchor));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabs.active]);

  const detail = useAsync(() => api.workspace(ws), [ws]);
  useChannel(`workspace:${ws}`, (m) => {
    if (m.type === "workspace_updated") detail.reload();
  });
  const tree = useWorkspaceTree(ws);
  const wsName = detail.data?.settings.name ?? ws;
  const projects = useMemo(() => detail.data?.projects ?? [], [detail.data]);

  const [palette, setPalette] = useState(false);
  const [dialog, setDialog] = useState<"new-workspace" | "add-project" | "setup" | null>(null);
  const [taskDraft, setTaskDraft] = useState<string | null>(null);
  const [dockSeed, setDockSeed] = useState<{ text: string; n: number } | null>(null);
  const theme: Theme = resolveTheme(prefs.theme);
  useEffect(() => applyTheme(theme), [theme]);

  const open = useCallback(
    (id: string, opts: OpenOptions = {}) => {
      const r = parseResource(id);
      if (!r) return;
      if (r.type === "file") setPref("filesProject", r.key);
      if (opts.anchor) pendingAnchor.current = opts.anchor;
      if (opts.anchor && tabs.active === id) {
        pendingAnchor.current = null;
        shellNav.current = id;
        navigate(resourcePath(ws, id, opts.anchor));
      }
      dispatch({ type: "open", id, preview: opts.preview, beside: opts.beside });
    },
    [ws, navigate, setPref, tabs.active],
  );
  useShotBridge(tabs.active, open, theme, (t) => setPref("theme", t));
  const pinActive = useCallback(() => tabs.active && dispatch({ type: "keep", id: tabs.active }), [tabs.active]);
  const [tabMenu, setTabMenu] = useState<{ id: string; x: number; y: number } | null>(null);

  const ctx: ConsoleContextValue = useMemo(
    () => ({
      nav: {
        ws,
        activeId: tabs.active,
        tabs: tabs.tabs,
        open,
        close: (id) => dispatch({ type: "close", id }),
        keep: (id) => dispatch({ type: "keep", id }),
        href: (id) => resourcePath(ws, id),
      },
      shell: {
        openDock: (question) => {
          setPref("dockOpen", true);
          if (question !== undefined) setDockSeed((s) => ({ text: question, n: (s?.n ?? 0) + 1 }));
        },
        closeDock: () => setPref("dockOpen", false),
        taskDraft,
        setTaskDraft,
        newWorkspace: () => setDialog("new-workspace"),
        addProject: () => setDialog("add-project"),
        runSetup: () => setDialog("setup"),
        browseFiles: (key) => {
          setPrefs((p) =>
            key === ARTIFACTS_ROOT
              ? { ...p, sidebarOpen: true, leftTab: "artifacts" }
              : { ...p, sidebarOpen: true, leftTab: "files", filesProject: key },
          );
        },
        theme,
        toggleTheme: () => setPref("theme", theme === "dark" ? "light" : "dark"),
      },
      workspace: { detail: detail.data, reload: detail.reload },
    }),
    [ws, tabs, open, taskDraft, theme, detail.data, detail.reload, setPref],
  );

  const keyLock = useKeyboardLock();
  const tabsRef = useRef(tabs);
  tabsRef.current = tabs;
  const lockedRef = useRef(keyLock.locked);
  lockedRef.current = keyLock.locked;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = shortcutOf(e, isMac, lockedRef.current);
      if (!s) return;
      // Ctrl+W deletes a word in a shell, so the terminal keeps it.
      if (s === "close-tab" && (e.target as HTMLElement | null)?.closest?.(".ex-xterm")) return;
      e.preventDefault();
      if (s === "close-tab") {
        const { active } = tabsRef.current;
        if (active) dispatch({ type: "close", id: active });
        return;
      }
      if (s === "next-tab" || s === "prev-tab") {
        // Strip order, wrapping at the ends, like VS Code with its MRU switcher turned off.
        const { tabs: list, active } = tabsRef.current;
        if (list.length < 2) return;
        const i = list.findIndex((t) => t.id === active);
        const next = list[(i + (s === "next-tab" ? 1 : -1) + list.length) % list.length];
        dispatch({ type: "activate", id: next.id });
        return;
      }
      if (s === "palette") setPalette((p) => !p);
      else if (s === "dock") setPref("dockOpen", (d) => !d);
      else if (s === "sidebar") setPref("sidebarOpen", (v) => !v);
      else if (s === "files")
        setPrefs((p) => ({
          ...p,
          sidebarOpen: true,
          leftTab: p.leftTab === "files" && p.sidebarOpen ? "sessions" : "files",
        }));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [setPref]);

  const metaCtx = { wsName, sessions: tree.sessions };
  const active = tabs.active;
  const activeMeta = active ? resourceMeta(active, metaCtx) : { crumbs: [{ label: wsName }] };
  const activeSession = useMemo(() => {
    const r = active ? parseResource(active) : null;
    if (r?.type === "session") return r.id;
    if (r?.type === "exec")
      return tree.sessions.find((s) => s.groups.some((g) => g.runs.some((x) => x.id === r.id)))?.id ?? null;
    if (r?.type === "artifact")
      return tree.sessions.find((s) => s.artifacts.some((a) => a.path === r.path))?.id ?? null;
    return null;
  }, [active, tree.sessions]);

  const { sessions: summaries } = useSessionSummaries(ws);
  const markSession = activeSession ? (summaries.find((s) => s.id === activeSession) ?? null) : null;
  const mark = markState(markSession);
  const markLabel = markSession ? MARK_LABEL[markSession.status] : "No session open";
  useLiveFavicon(mark, theme);

  const runCommand = (command: string) => {
    if (command === "new-workspace") setDialog("new-workspace");
    else if (command === "add-project") setDialog("add-project");
    else if (command === "dock") setPref("dockOpen", true);
    else if (command === "theme") ctx.shell.toggleTheme();
    else if (command === "keyboard-lock") keyLock.toggle();
  };

  const onTabsDoubleClick = (e: MouseEvent) => {
    const tab = (e.target as HTMLElement).closest('[role="tab"]');
    if (!tab?.parentElement) return;
    const i = Array.from(tab.parentElement.children).indexOf(tab);
    if (tabs.tabs[i]) dispatch({ type: "keep", id: tabs.tabs[i].id });
  };
  const onTabsAuxClick = (e: MouseEvent) => {
    if (e.button !== 1) return;
    const tab = (e.target as HTMLElement).closest('[role="tab"]');
    if (!tab?.parentElement) return;
    const i = Array.from(tab.parentElement.children).indexOf(tab);
    if (tabs.tabs[i]) dispatch({ type: "close", id: tabs.tabs[i].id });
  };

  const tabMenuItems = (id: string): MenuItem[] => {
    const at = tabs.tabs.findIndex((t) => t.id === id);
    const tab = tabs.tabs[at];
    if (!tab) return [];
    // A bulk close is offered only when it would close an unpinned tab.
    const bulk = (label: string, scope: CloseScope, stays: (j: number) => boolean): MenuItem[] =>
      tabs.tabs.some((t, j) => !t.pinned && !stays(j))
        ? [{ label, onSelect: () => dispatch({ type: "closeMany", scope, id }) }]
        : [];
    // Moves stay within the tab's group, pinned or not.
    const pins = tabs.tabs.filter((t) => t.pinned).length;
    const moves = (i: number, pinned: boolean): MenuItem[] => {
      const [lo, hi] = pinned ? [0, pins - 1] : [pins, tabs.tabs.length - 1];
      return [
        ...(i > lo
          ? [
              {
                label: "Move left",
                hint: isMac ? "⇧⌘←" : "Ctrl+Shift+←",
                onSelect: () => dispatch({ type: "move", id, to: i - 1 }),
              },
            ]
          : []),
        ...(i < hi
          ? [
              {
                label: "Move right",
                hint: isMac ? "⇧⌘→" : "Ctrl+Shift+→",
                onSelect: () => dispatch({ type: "move", id, to: i + 1 }),
              },
            ]
          : []),
      ];
    };
    return [
      { label: "Close", onSelect: () => dispatch({ type: "close", id }) },
      ...bulk("Close others", "others", (j) => j === at),
      ...bulk("Close to the left", "left", (j) => j >= at),
      ...bulk("Close to the right", "right", (j) => j <= at),
      ...bulk("Close all", "all", () => false).map((it) =>
        tabs.tabs.some((t) => t.pinned) ? { ...it, hint: "Keeps pinned" } : it,
      ),
      { type: "divider" },
      ...moves(at, tab.pinned),
      {
        label: tab.pinned ? "Unpin" : "Pin",
        icon: tab.pinned ? "pin-off" : "pin",
        onSelect: () => dispatch({ type: "setPinned", id, pinned: !tab.pinned }),
      },
    ];
  };

  return (
    <ConsoleContext.Provider value={ctx}>
      <div
        style={{
          height: "100%",
          display: "flex",
          flexDirection: "column",
          background: "var(--surface-editor)",
          ...({ "--sidebar-w": `${widths.sidebar}px`, "--dock-w": `${widths.dock}px` } as CSSProperties),
        }}
      >
        <TitleBar
          ws={ws}
          wsName={wsName}
          activeId={active}
          mark={mark}
          markLabel={markLabel}
          go={(id) => open(id)}
          crumbs={activeMeta.crumbs}
          onPalette={() => setPalette(true)}
          sidebar={prefs.sidebarOpen}
          toggleSidebar={() => setPref("sidebarOpen", (v) => !v)}
          dock={prefs.dockOpen}
          toggleDock={() => setPref("dockOpen", (v) => !v)}
          onNewWorkspace={() => setDialog("new-workspace")}
          onAddProject={() => setDialog("add-project")}
          onSetup={() => setDialog("setup")}
        />
        <div style={{ flex: 1, display: "flex", minHeight: 0 }}>
          {prefs.sidebarOpen && (
            <Sidebar
              ws={ws}
              resizer={<ResizeHandle side="sidebar" edge="right" width={widths.sidebar} onResize={setWidth} />}
              tab={prefs.leftTab}
              setTab={(t) => setPref("leftTab", t)}
              sessions={tree.sessions}
              loading={tree.loading}
              error={tree.error}
              projects={projects}
              activeId={active}
              open={open}
              pinActive={pinActive}
              filesProject={prefs.filesProject}
              setFilesProject={(k) => setPref("filesProject", k)}
              onAddProject={() => setDialog("add-project")}
            />
          )}
          <main
            style={{
              flex: 1,
              minWidth: 0,
              display: "flex",
              flexDirection: "column",
              background: "var(--surface-editor)",
            }}
          >
            {tabs.tabs.length > 0 && (
              <div ref={tabStrip} className="shell-tabs" onDoubleClick={onTabsDoubleClick} onAuxClick={onTabsAuxClick}>
                <Tabs
                  variant="bar"
                  label="Open tabs"
                  value={active ?? ""}
                  onChange={(id) => dispatch({ type: "activate", id })}
                  onClose={(id) => dispatch({ type: "close", id })}
                  onUnpin={(id) => dispatch({ type: "setPinned", id, pinned: false })}
                  onMove={(id, to) => dispatch({ type: "move", id, to })}
                  onContextMenu={(id, e) => {
                    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
                    const x = e.clientX || r.left;
                    const y = e.clientY || r.bottom;
                    setTabMenu({ id, x: Math.min(x, window.innerWidth - 240), y });
                  }}
                  tabs={tabs.tabs.map((t) => {
                    const m = resourceMeta(t.id, metaCtx);
                    return {
                      id: t.id,
                      label: m.label,
                      title: m.title ?? m.label,
                      icon: m.status ? null : m.icon,
                      italic: t.preview,
                      pinned: t.pinned,
                      dot: m.status ? (
                        <StatusDot tone={TAB_TONE[m.status]} pulse={m.status === "running"} />
                      ) : undefined,
                    };
                  })}
                />
                <Menu
                  open={!!tabMenu}
                  onClose={() => setTabMenu(null)}
                  items={tabMenu ? tabMenuItems(tabMenu.id) : []}
                  width={220}
                  label="Tab actions"
                  style={tabMenu ? { position: "fixed", left: tabMenu.x, top: tabMenu.y } : undefined}
                />
              </div>
            )}
            {active !== "ws:settings" && detail.data && (
              <PendingCommandsNotice pending={detail.data.pending_commands} onReview={() => open("ws:settings")} />
            )}
            <div
              onDoubleClick={pinActive}
              style={{
                flex: 1,
                overflowY: active && selfScrolling(active) ? "hidden" : "auto",
                overflowX: "hidden",
                minHeight: 0,
              }}
            >
              {detail.error && !detail.data ? (
                <WorkspaceMissing ws={ws} message={detail.error.message} />
              ) : active ? (
                <Outlet />
              ) : (
                <NothingOpen />
              )}
            </div>
          </main>
          {prefs.dockOpen && (
            <QuickDock
              ws={ws}
              resizer={<ResizeHandle side="dock" edge="left" width={widths.dock} onResize={setWidth} />}
              session={activeSession}
              seed={dockSeed}
              onClose={() => setPref("dockOpen", false)}
              onTurnIntoTask={(q) => {
                setTaskDraft(q);
                open("ws:overview");
                setPref("dockOpen", false);
              }}
            />
          )}
        </div>
        <StatusBar
          ws={ws}
          session={activeSession}
          open={open}
          theme={theme}
          toggleTheme={ctx.shell.toggleTheme}
          keysLocked={keyLock.locked}
          unlockKeys={keyLock.unlock}
        />
      </div>
      {dialog === "new-workspace" && <NewWorkspaceDialog onClose={() => setDialog(null)} />}
      {dialog === "add-project" && (
        <AddProjectDialog
          ws={ws}
          onClose={() => setDialog(null)}
          onAdded={(_, key) => {
            setDialog(null);
            detail.reload();
            if (key) open(`project:${key}`);
          }}
        />
      )}
      {dialog === "setup" && (
        <Onboarding
          onFinish={(created) => {
            setDialog(null);
            if (created && created !== ws) navigate(`/w/${encodeURIComponent(created)}`);
          }}
        />
      )}
      <Palette
        ws={ws}
        open={palette}
        onClose={() => setPalette(false)}
        sessions={tree.sessions}
        projects={projects}
        filesProject={prefs.filesProject ?? projects[0]?.key ?? null}
        onSelect={(id) => {
          const t = paletteTarget(id);
          if (t.kind === "command") runCommand(t.command);
          else open(t.id, { anchor: t.anchor });
        }}
        lock={!keyLock.supported ? "unsupported" : keyLock.locked ? "locked" : "unlocked"}
      />
    </ConsoleContext.Provider>
  );
}

const MARK_LABEL: Record<SessionStatus, string> = {
  running: "Running",
  waiting: "Waiting for you",
  paused: "Paused",
  failed: "Failed",
  stalled: "Stalled",
  completed: "Completed",
};

function NothingOpen() {
  return (
    <div style={{ height: "100%", display: "grid", placeItems: "center", color: "var(--text-muted)" }}>
      <div style={{ display: "flex", flexDirection: "column", gap: 10, alignItems: "center" }}>
        <span style={{ opacity: 0.5 }}>
          <LiveMark state={REST} size={36} />
        </span>
        <span style={{ display: "inline-flex", gap: 6, alignItems: "center" }}>
          Nothing open. Press <Kbd keys={modKeys("K")} /> to go to a session, execution or artifact.
        </span>
      </div>
    </div>
  );
}

function WorkspaceMissing({ ws, message }: { ws: string; message: string }) {
  const navigate = useNavigate();
  return (
    <div
      style={{
        maxWidth: 560,
        margin: "80px auto",
        padding: "0 24px",
        display: "flex",
        flexDirection: "column",
        gap: 12,
      }}
    >
      <h1 style={{ margin: 0, font: "var(--type-title)" }}>Workspace not available</h1>
      <p style={{ margin: 0, color: "var(--text-secondary)", lineHeight: 1.55 }}>
        Ostra could not open <code>{ws}</code>: {message} Check that its folder still exists, then open it again from
        the workspace list.
      </p>
      <div>
        <Button icon="arrow-left" onClick={() => navigate("/")}>
          All workspaces
        </Button>
      </div>
    </div>
  );
}
