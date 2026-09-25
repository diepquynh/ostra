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
import { Button, Kbd, StatusDot, Tabs, type Tone } from "../design";
import { useAsync, useChannel } from "../lib/hooks";
import { modKeys, shortcutOf } from "../lib/keys";
import { useWorkspaceTree } from "../lib/live";
import { ConsoleContext, type ConsoleContextValue, type OpenOptions, type Theme } from "../lib/nav";
import { paletteTarget } from "../lib/palette";
import { parseResource, resourceFromPath, resourcePath } from "../lib/resource";
import { emptyTabs, normalizeTabs, type TabsState, tabsReducer } from "../lib/tabs";
import { applyTheme, resolveTheme } from "../lib/theme";
import { AddProjectDialog, NewWorkspaceDialog, Onboarding, selfScrolling } from "../screens";
import { resourceMeta } from "./meta";
import { Palette } from "./Palette";
import { QuickDock } from "./QuickDock";
import { ResizeHandle, usePanelWidths } from "./ResizeHandle";
import { Sidebar } from "./Sidebar";
import { StatusBar } from "./StatusBar";
import { TitleBar } from "./TitleBar";
import { DEFAULT_PREFS, fromServerUi, loadLocalUi, saveLocalUi, toServerUi, type UiPrefs } from "./uiState";
import "./shell.css";

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
  // Remount per workspace so tabs, stores and dialogs never leak across workspaces.
  return <Shell key={ws} ws={ws} />;
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

  const [local] = useState(() => loadLocalUi(ws));
  const [tabs, dispatch] = useReducer(tabsReducer, null, () => initialTabs(local?.tabs ?? null, routeId));
  const [prefs, setPrefs] = useState<UiPrefs>(() => local?.prefs ?? DEFAULT_PREFS);
  const [widths, setWidth] = usePanelWidths();
  const setPref = useCallback(<K extends keyof UiPrefs>(k: K, v: UiPrefs[K] | ((p: UiPrefs[K]) => UiPrefs[K])) => {
    setPrefs((p) => ({ ...p, [k]: typeof v === "function" ? (v as (x: UiPrefs[K]) => UiPrefs[K])(p[k]) : v }));
  }, []);

  // Server copy of the layout: restores it in a browser that has no local copy, then receives every change.
  const serverReady = useRef(false);
  useEffect(() => {
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
  const tabStrip = useCallback((el: HTMLDivElement | null) => {
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (e.deltaY === 0 || Math.abs(e.deltaX) > Math.abs(e.deltaY) || el.scrollWidth <= el.clientWidth) return;
      e.preventDefault();
      el.scrollLeft += e.deltaY;
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

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
      dispatch({ type: "open", id, preview: opts.preview });
    },
    [ws, navigate, setPref, tabs.active],
  );
  const pinActive = useCallback(() => tabs.active && dispatch({ type: "pin", id: tabs.active }), [tabs.active]);

  const ctx: ConsoleContextValue = useMemo(
    () => ({
      nav: {
        ws,
        activeId: tabs.active,
        tabs: tabs.tabs,
        open,
        close: (id) => dispatch({ type: "close", id }),
        pin: (id) => dispatch({ type: "pin", id }),
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
          setPrefs((p) => ({ ...p, sidebarOpen: true, leftTab: "files", filesProject: key }));
        },
        theme,
        toggleTheme: () => setPref("theme", theme === "dark" ? "light" : "dark"),
      },
      workspace: { detail: detail.data, reload: detail.reload },
    }),
    [ws, tabs, open, taskDraft, theme, detail.data, detail.reload, setPref],
  );

  const tabsRef = useRef(tabs);
  tabsRef.current = tabs;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = shortcutOf(e);
      if (!s) return;
      e.preventDefault();
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

  const runCommand = (command: string) => {
    if (command === "new-workspace") setDialog("new-workspace");
    else if (command === "add-project") setDialog("add-project");
    else if (command === "dock") setPref("dockOpen", true);
    else if (command === "theme") ctx.shell.toggleTheme();
  };

  const onTabsDoubleClick = (e: MouseEvent) => {
    const tab = (e.target as HTMLElement).closest('[role="tab"]');
    if (!tab?.parentElement) return;
    const i = Array.from(tab.parentElement.children).indexOf(tab);
    if (tabs.tabs[i]) dispatch({ type: "pin", id: tabs.tabs[i].id });
  };
  const onTabsAuxClick = (e: MouseEvent) => {
    if (e.button !== 1) return;
    const tab = (e.target as HTMLElement).closest('[role="tab"]');
    if (!tab?.parentElement) return;
    const i = Array.from(tab.parentElement.children).indexOf(tab);
    if (tabs.tabs[i]) dispatch({ type: "close", id: tabs.tabs[i].id });
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
                  tabs={tabs.tabs.map((t) => {
                    const m = resourceMeta(t.id, metaCtx);
                    return {
                      id: t.id,
                      label: m.label,
                      title: m.title ?? m.label,
                      icon: m.status ? null : m.icon,
                      italic: t.preview,
                      dot: m.status ? (
                        <StatusDot tone={TAB_TONE[m.status]} pulse={m.status === "running"} />
                      ) : undefined,
                    };
                  })}
                />
              </div>
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
        <StatusBar ws={ws} session={activeSession} open={open} theme={theme} toggleTheme={ctx.shell.toggleTheme} />
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
      />
    </ConsoleContext.Provider>
  );
}

function NothingOpen() {
  return (
    <div style={{ height: "100%", display: "grid", placeItems: "center", color: "var(--text-muted)" }}>
      <div style={{ display: "flex", flexDirection: "column", gap: 10, alignItems: "center" }}>
        <img src="/favicon.svg" width={36} height={36} style={{ opacity: 0.5 }} alt="" />
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
