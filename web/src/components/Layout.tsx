import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { Link, Outlet, useLocation } from "react-router";
import { isMock, socket } from "../api";
import type { SocketState } from "../api/extra";
import { AskPanel } from "../features/ask/AskPanel";

type Crumb = { label: string; to?: string };

type LayoutCtx = {
  setCrumbs: (c: Crumb[]) => void;
  /** Session the side panel should add as context, when on a session screen. */
  setAskSession: (id: string | null) => void;
  /** Prefill for the New task form, set by "Turn into task". */
  taskDraft: string | null;
  setTaskDraft: (text: string | null) => void;
};

const Ctx = createContext<LayoutCtx>({
  setCrumbs: () => {},
  setAskSession: () => {},
  taskDraft: null,
  setTaskDraft: () => {},
});

export const useLayout = () => useContext(Ctx);

/** Set the breadcrumb trail for the current screen. */
export function useCrumbs(crumbs: Crumb[]) {
  const { setCrumbs } = useLayout();
  const key = JSON.stringify(crumbs);
  useEffect(() => {
    setCrumbs(crumbs);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
}

function SocketDot() {
  const [state, setState] = useState<SocketState>(socket().state);
  useEffect(() => socket().onState(setState), []);
  if (isMock) return <span className="chip info">Mock data</span>;
  const tone = state === "open" ? "ok" : state === "connecting" ? "warn" : "bad";
  const label = state === "open" ? "Live" : state === "connecting" ? "Connecting" : "Offline, reconnecting";
  return (
    <span className="row small muted" title="Live updates from the Ostra server">
      <span className={`dot ${tone}`} /> {label}
    </span>
  );
}

export function Layout({ children }: { children?: ReactNode }) {
  const [crumbs, setCrumbs] = useState<Crumb[]>([]);
  const [askSession, setAskSession] = useState<string | null>(null);
  const [taskDraft, setTaskDraft] = useState<string | null>(null);
  const [panelOpen, setPanelOpen] = useState<boolean>(() => localStorage.getItem("ostra.askPanel") === "1");
  const { pathname } = useLocation();
  const ws = /^\/w\/([^/]+)/.exec(pathname)?.[1];

  const togglePanel = () => {
    setPanelOpen((v) => {
      localStorage.setItem("ostra.askPanel", v ? "0" : "1");
      return !v;
    });
  };

  const showPanel = Boolean(ws) && panelOpen;

  return (
    <Ctx.Provider value={{ setCrumbs, setAskSession, taskDraft, setTaskDraft }}>
      <div className="app">
        <header className="topbar">
          <Link to="/" className="brand">
            Ostra
          </Link>
          <nav className="crumbs">
            {crumbs.map((c, i) => (
              <span key={i} className="row">
                <span className="sep">/</span>
                {c.to ? <Link to={c.to}>{c.label}</Link> : <span>{c.label}</span>}
              </span>
            ))}
          </nav>
          <span className="spacer" />
          <SocketDot />
          {ws && (
            <button className={panelOpen ? "primary small" : "small"} onClick={togglePanel} title="Ask a quick question outside the pipeline">
              Quick question
            </button>
          )}
        </header>
        <div className={`body ${showPanel ? "with-panel" : ""}`}>
          <main className="main">{children ?? <Outlet />}</main>
          {showPanel && ws && (
            <aside className="side">
              <AskPanel ws={ws} session={askSession} onClose={togglePanel} />
            </aside>
          )}
        </div>
      </div>
    </Ctx.Provider>
  );
}
