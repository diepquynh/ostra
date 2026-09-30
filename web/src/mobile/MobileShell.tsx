import { Icon, LiveMark, REST, Spinner, StatusDot } from "@ostra/design";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router";
import { api, isMock } from "../api";
import type { TreeSession } from "../api/nav";
import type { SessionStatus } from "../api/types";
import { ARTIFACTS_ROOT } from "../features/context/tags";
import { basename, formatCost, humanize } from "../lib/format";
import { useAsync, useChannel } from "../lib/hooks";
import { useActivity, useSocketState, useWorkspaceTree } from "../lib/live";
import { ConsoleContext, type ConsoleContextValue, type OpenOptions, type Theme } from "../lib/nav";
import { parseResource, resourceFromPath, resourcePath } from "../lib/resource";
import { applyTheme, lastTheme, resolveTheme } from "../lib/theme";
import { sessionLabel } from "../shell/meta";
import { HeaderContext, type HeaderOverride } from "./header";
import { AskSheet, MenuSheet, SearchSheet } from "./Sheets";
import { MobileScreenFor, mobileFills } from "./screens";
import { MSetup, type SetupMode } from "./screens/MSetup";
import "./mobile.css";

type Sheet = "menu" | "search" | "ask" | null;

const STATUS: Record<SessionStatus, string> = {
  running: "Running",
  waiting: "Waiting for you",
  paused: "Paused",
  failed: "Failed",
  stalled: "Stalled",
  completed: "Completed",
};

const PAGE_TITLE: Record<string, string> = {
  overview: "Overview",
  cost: "Cost",
  settings: "Settings",
  memory: "Memory",
  skills: "Skills",
  docs: "Documentation",
};

/** The session a resource belongs to, for the quick question's context. */
export function sessionOf(id: string, sessions: TreeSession[]): TreeSession | null {
  const r = parseResource(id);
  if (r?.type === "session") return sessions.find((s) => s.id === r.id) ?? null;
  if (r?.type === "exec") return sessions.find((s) => s.groups.some((g) => g.runs.some((x) => x.id === r.id))) ?? null;
  if (r?.type === "artifact") return sessions.find((s) => s.artifacts.some((a) => a.path === r.path)) ?? null;
  return null;
}

/** Header title and mono subtitle of a resource. */
export function mobileTitle(
  id: string,
  ctx: { wsName: string; root: string; sessions: TreeSession[]; projectPath: (key: string) => string | undefined },
): { title: string; sub: string } {
  const r = parseResource(id);
  if (!r) return { title: ctx.wsName, sub: ctx.root };
  switch (r.type) {
    case "ws":
      return r.page === "overview"
        ? { title: ctx.wsName, sub: ctx.root }
        : { title: PAGE_TITLE[r.page], sub: ctx.wsName };
    case "book":
      return { title: r.id, sub: "Documentation book" };
    case "session": {
      const s = ctx.sessions.find((x) => x.id === r.id);
      return s ? { title: sessionLabel(s), sub: `Session · ${STATUS[s.status]}` } : { title: "Session", sub: r.id };
    }
    case "exec": {
      for (const s of ctx.sessions)
        for (const g of s.groups) {
          const run = g.runs.find((x) => x.id === r.id);
          if (run) return { title: `${humanize(g.agent)} · ${run.run_label}`, sub: `${g.project} · ${run.status}` };
        }
      return { title: "Execution", sub: r.id };
    }
    case "artifact": {
      for (const s of ctx.sessions) {
        const a = s.artifacts.find((x) => x.path === r.path);
        if (a) return { title: a.label, sub: r.path };
      }
      return { title: basename(r.path), sub: r.path };
    }
    case "project":
      return r.key === ARTIFACTS_ROOT
        ? { title: "Artifacts", sub: `${ctx.root}/.ostra/artifacts` }
        : { title: r.key, sub: ctx.projectPath(r.key) ?? "" };
    case "file":
      return {
        title: basename(r.path),
        sub: `${r.key === ARTIFACTS_ROOT ? "artifacts" : r.key}/${r.path}`,
      };
    case "dep":
      return { title: basename(r.uri), sub: r.key };
  }
}

/** The console on a phone: one stack of screens under a header, with the menu, search and questions as sheets. */
export function MobileShell({ ws }: { ws: string }) {
  const location = useLocation();
  const navigate = useNavigate();
  const active = useMemo(
    () => resourceFromPath(location.pathname, location.search)?.id ?? "ws:overview",
    [location.pathname, location.search],
  );

  const detail = useAsync(() => api.workspace(ws), [ws]);
  useChannel(`workspace:${ws}`, (m) => {
    if (m.type === "workspace_updated") detail.reload();
  });
  const tree = useWorkspaceTree(ws);
  const { activity } = useActivity(ws);
  const wsName = detail.data?.settings.name ?? ws;
  const root = detail.data?.root ?? "";

  const [theme, setTheme] = useState<Theme>(() => resolveTheme(lastTheme()));
  useEffect(() => applyTheme(theme), [theme]);
  const [sheet, setSheet] = useState<Sheet>(null);
  const [setup, setSetup] = useState<SetupMode | null>(null);
  const [taskDraft, setTaskDraft] = useState<string | null>(null);
  const [header, setHeader] = useState<HeaderOverride | null>(null);
  const [askSeed, setAskSeed] = useState<{ text: string; n: number } | null>(null);

  const scroller = useRef<HTMLDivElement>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: every screen change starts at the top.
  useEffect(() => {
    if (!location.hash) scroller.current?.scrollTo?.({ top: 0 });
  }, [active, setup]);

  // Each pushed screen records its depth in the history entry, so Back knows whether there is a screen of this
  // workspace to return to or whether the stack started here (a deep link, a push notification).
  const depth = (location.state as { depth?: number } | null)?.depth ?? 0;
  const open = useCallback(
    (id: string, opts: OpenOptions = {}) => {
      if (!parseResource(id)) return;
      setSheet(null);
      setSetup(null);
      const to = resourcePath(ws, id, opts.anchor);
      if (id === active && !opts.push) navigate(to, { replace: true, state: { depth } });
      else navigate(to, { state: { depth: id === "ws:overview" && !opts.push ? 0 : depth + 1 } });
    },
    [ws, navigate, active, depth],
  );
  const canBack = !!setup || active !== "ws:overview";
  const back = () => {
    if (setup) setSetup(null);
    else if (depth > 0) navigate(-1);
    else navigate(resourcePath(ws, "ws:overview"), { replace: true });
  };

  const ctx: ConsoleContextValue = useMemo(
    () => ({
      nav: {
        ws,
        activeId: active,
        tabs: [],
        open,
        close: () => back(),
        keep: () => {},
        href: (id) => resourcePath(ws, id),
      },
      shell: {
        openDock: (question) => {
          if (question !== undefined) setAskSeed((s) => ({ text: question, n: (s?.n ?? 0) + 1 }));
          setSheet("ask");
        },
        closeDock: () => setSheet(null),
        taskDraft,
        setTaskDraft,
        newWorkspace: () => setSetup("new"),
        addProject: () => setSetup("add"),
        runSetup: () => setSetup("onboard"),
        browseFiles: (key) => open(`project:${key}`),
        theme,
        toggleTheme: () => setTheme((t) => (t === "dark" ? "light" : "dark")),
      },
      workspace: { detail: detail.data, reload: detail.reload },
    }),
    // biome-ignore lint/correctness/useExhaustiveDependencies: `back` reads the latest state through the closure.
    [ws, active, open, taskDraft, theme, detail.data, detail.reload],
  );

  const projects = detail.data?.projects ?? [];
  const base = mobileTitle(active, {
    wsName,
    root,
    sessions: tree.sessions,
    projectPath: (k) => projects.find((p) => p.key === k)?.path,
  });
  const head = setup
    ? {
        title: { onboard: "Set up Ostra", new: "New workspace", add: "Add project" }[setup],
        sub: setup === "add" ? wsName : "Setup",
      }
    : header
      ? { title: header.title, sub: header.sub ?? base.sub }
      : base;
  const isHome = !setup && active === "ws:overview";
  const pending = detail.data?.pending_commands ?? [];
  const pendingCount = pending.reduce((n, p) => n + p.items.length, 0);
  const showPending = pendingCount > 0 && !setup && active !== "ws:settings";
  const askSession = sessionOf(active, tree.sessions);

  return (
    <ConsoleContext.Provider value={ctx}>
      <div className="m-shell">
        <header className="m-header">
          {canBack && (
            <button type="button" className="m-hbtn" aria-label="Back" onClick={back}>
              <Icon name="arrow-left" size={20} />
            </button>
          )}
          {isHome && (
            <button type="button" className="m-hbtn" aria-label="Workspaces" onClick={() => setSheet("menu")}>
              <LiveMark state={REST} size={26} />
            </button>
          )}
          <button
            type="button"
            className="m-title"
            onClick={() => (isHome ? setSheet("menu") : scroller.current?.scrollTo?.({ top: 0, behavior: "smooth" }))}
          >
            <span className="m-title-main">
              {head.title}
              {isHome && <Icon name="chevron-down" size={14} style={{ color: "var(--text-muted)" }} />}
            </span>
            <span className="m-title-sub">{head.sub}</span>
          </button>
          <button type="button" className="m-hbtn" aria-label="Search" onClick={() => setSheet("search")}>
            <Icon name="search" size={19} />
          </button>
          <button type="button" className="m-hbtn" aria-label="Ask a quick question" onClick={() => setSheet("ask")}>
            <Icon name="message-square" size={19} />
          </button>
          <button type="button" className="m-hbtn" aria-label="Menu" onClick={() => setSheet("menu")}>
            <Icon name="menu" size={20} />
          </button>
        </header>
        {showPending && (
          <button type="button" className="m-notice" onClick={() => open("ws:settings")}>
            <Icon name="triangle-alert" size={15} style={{ color: "var(--warn)", flex: "none" }} />
            <span className="m-notice-text">
              {pendingCount === 1 ? "1 command waits" : `${pendingCount} commands wait`} for approval in{" "}
              {pending.map((p) => p.file).join(", ")}.
            </span>
            <span className="m-link">Review</span>
          </button>
        )}
        <div ref={scroller} className={`m-scroll${!setup && mobileFills(active) ? " m-fill" : ""}`}>
          {detail.error && !detail.data ? (
            <Missing ws={ws} message={detail.error.message} />
          ) : setup ? (
            <MSetup
              ws={ws}
              mode={setup}
              onDone={(created, addedKey) => {
                setSetup(null);
                if (created && created !== ws) navigate(`/w/${encodeURIComponent(created)}`);
                else if (addedKey) {
                  detail.reload();
                  open(`project:${addedKey}`);
                } else detail.reload();
              }}
            />
          ) : (
            <HeaderContext.Provider value={setHeader}>
              <MobileScreenFor ws={ws} id={active} />
            </HeaderContext.Provider>
          )}
        </div>
        <Footer ws={ws} open={open} />
        {sheet && (
          <>
            <div className="m-scrim" onClick={() => setSheet(null)} aria-hidden />
            <div className="m-sheet" role="dialog" aria-modal="true" aria-label={SHEET_LABEL[sheet]}>
              <div className="m-grip" />
              {sheet === "menu" && (
                <MenuSheet
                  ws={ws}
                  wsName={wsName}
                  theme={theme}
                  spend={activity ? formatCost(activity.spend_week_usd) : ""}
                  open={open}
                  onClose={() => setSheet(null)}
                  onSearch={() => setSheet("search")}
                  onSetup={(m) => {
                    setSheet(null);
                    setSetup(m);
                  }}
                  toggleTheme={ctx.shell.toggleTheme}
                />
              )}
              {sheet === "search" && (
                <SearchSheet
                  ws={ws}
                  sessions={tree.sessions}
                  projects={projects}
                  open={open}
                  onAsk={() => setSheet("ask")}
                  onSetup={(m) => {
                    setSheet(null);
                    setSetup(m);
                  }}
                  toggleTheme={() => {
                    ctx.shell.toggleTheme();
                    setSheet(null);
                  }}
                />
              )}
              {sheet === "ask" && (
                <AskSheet
                  ws={ws}
                  session={askSession ? { id: askSession.id, label: sessionLabel(askSession) } : null}
                  seed={askSeed}
                  onTurnIntoTask={(q) => {
                    setTaskDraft(q);
                    open("ws:overview");
                  }}
                />
              )}
            </div>
          </>
        )}
      </div>
    </ConsoleContext.Provider>
  );
}

const SHEET_LABEL: Record<Exclude<Sheet, null>, string> = {
  menu: "Menu",
  search: "Search",
  ask: "Ask a quick question",
};

function Footer({ ws, open }: { ws: string; open: (id: string, opts?: OpenOptions) => void }) {
  const state = useSocketState();
  const { activity } = useActivity(ws);
  const running = activity?.running.length ?? 0;
  const gates = activity?.open_gates ?? [];
  const [tone, label] = isMock
    ? (["info", "Mock data"] as const)
    : state === "open"
      ? (["ok", "Live"] as const)
      : state === "connecting"
        ? (["warn", "Connecting"] as const)
        : (["bad", "Offline"] as const);
  const g = gates[0];
  return (
    <footer className="m-footer">
      <span className="m-footer-item">
        <StatusDot tone={tone} size={6} pulse={state === "connecting"} />
        {label}
      </span>
      {running > 0 && (
        <span className="m-footer-item">
          <Spinner size={10} style={{ color: "var(--accent)" }} />
          {running} running
        </span>
      )}
      {g && (
        <button
          type="button"
          className="m-footer-item"
          style={{ color: "var(--warn)" }}
          onClick={() => open(`session:${g.session}`, g.id.startsWith("session:") ? {} : { anchor: `gate-${g.id}` })}
        >
          <StatusDot tone="warn" size={6} />
          {gates.length === 1 ? "1 gate waiting" : `${gates.length} gates waiting`}
        </button>
      )}
      {activity && (
        <span style={{ marginLeft: "auto", fontFamily: "var(--font-mono)" }}>
          {formatCost(activity.spend_today_usd)} today
        </span>
      )}
    </footer>
  );
}

function Missing({ ws, message }: { ws: string; message: string }) {
  const navigate = useNavigate();
  return (
    <div className="m-page">
      <div className="m-section">
        <span style={{ fontSize: 16, fontWeight: 600 }}>Workspace not available</span>
        <span className="m-help" style={{ fontSize: 13, color: "var(--text-secondary)" }}>
          Ostra could not open <code>{ws}</code>: {message} Check that its folder still exists, then open it again from
          the workspace list.
        </span>
        <button type="button" className="m-btn" onClick={() => navigate("/")}>
          <Icon name="arrow-left" size={14} /> All workspaces
        </button>
      </div>
    </div>
  );
}
