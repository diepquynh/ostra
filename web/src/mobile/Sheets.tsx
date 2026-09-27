import { Banner, Chip, Icon, type IconName, Spinner } from "@ostra/design";
import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { api } from "../api";
import type { TreeSession } from "../api/nav";
import type { ProjectView } from "../api/types";
import { ARTIFACTS_ROOT } from "../features/context/tags";
import { useSearch, useWorkspaces } from "../lib/live";
import type { OpenOptions, Theme } from "../lib/nav";
import { localItems, mergePalette, paletteCommands, paletteTarget } from "../lib/palette";
import { Answer } from "../shell/QuickDock";
import type { SetupMode } from "./screens/MSetup";

type Open = (id: string, opts?: OpenOptions) => void;

type MenuSheetProps = {
  ws: string;
  wsName: string;
  theme: Theme;
  /** This week's spend, the hint on the Cost row. */
  spend: string;
  open: Open;
  onClose: () => void;
  onSearch: () => void;
  onSetup: (mode: SetupMode) => void;
  toggleTheme: () => void;
};

/** Workspaces, then this workspace's pages and actions. */
export function MenuSheet({ ws, wsName, theme, spend, open, onClose, onSearch, onSetup, toggleTheme }: MenuSheetProps) {
  const { workspaces } = useWorkspaces();
  const navigate = useNavigate();
  const items: { icon: IconName; label: string; hint?: string; onTap: () => void }[] = [
    { icon: "layout-dashboard", label: "Overview", onTap: () => open("ws:overview") },
    { icon: "package", label: "Artifacts", hint: "workspace files", onTap: () => open(`project:${ARTIFACTS_ROOT}`) },
    { icon: "coins", label: "Cost", hint: spend ? `${spend} this week` : undefined, onTap: () => open("ws:cost") },
    { icon: "book-open", label: "Skills", hint: "every project", onTap: () => open("ws:skills") },
    { icon: "brain", label: "Memory", hint: "lessons", onTap: () => open("ws:memory") },
    { icon: "settings", label: "Settings", onTap: () => open("ws:settings") },
    { icon: "search", label: "Search", onTap: onSearch },
    { icon: "box", label: "New workspace", onTap: () => onSetup("new") },
    { icon: "folder-plus", label: "Add project", onTap: () => onSetup("add") },
    { icon: "compass", label: "Run the setup guide again", onTap: () => onSetup("onboard") },
    {
      icon: "sun-moon",
      label: theme === "dark" ? "Switch to light theme" : "Switch to dark theme",
      onTap: () => {
        toggleTheme();
        onClose();
      },
    },
  ];
  return (
    <div style={{ overflowY: "auto", padding: "0 0 12px" }}>
      <div className="m-label m-sheet-label">Workspaces</div>
      {workspaces.map((w) => (
        <button
          type="button"
          key={w.id}
          className="m-sheet-row"
          style={{ minHeight: 52 }}
          disabled={!w.available}
          onClick={() => {
            onClose();
            if (w.id !== ws) navigate(`/w/${encodeURIComponent(w.id)}`);
          }}
        >
          <Icon name="box" size={16} style={{ color: "var(--text-muted)" }} />
          <span className="m-row-body">
            <span style={{ fontSize: 14, fontWeight: 500 }}>{w.name}</span>
            <span className="m-mono-sub">
              {w.root} · {w.projects} {w.projects === 1 ? "project" : "projects"}
            </span>
          </span>
          {w.id === ws && <Icon name="check" size={16} style={{ color: "var(--accent)" }} />}
          {!w.available && <Chip tone="bad">Folder missing</Chip>}
        </button>
      ))}
      <div className="m-divider" />
      <div className="m-label m-sheet-label">{wsName}</div>
      {items.map((m) => (
        <button type="button" key={m.label} className="m-sheet-row" onClick={m.onTap}>
          <Icon name={m.icon} size={16} style={{ color: "var(--text-muted)" }} />
          <span style={{ flex: 1 }}>{m.label}</span>
          {m.hint && <span style={{ fontSize: 12, color: "var(--text-muted)" }}>{m.hint}</span>}
        </button>
      ))}
    </div>
  );
}

type SearchSheetProps = {
  ws: string;
  sessions: TreeSession[];
  projects: ProjectView[];
  open: Open;
  onAsk: () => void;
  onSetup: (mode: SetupMode) => void;
  toggleTheme: () => void;
};

/** The ⌘K palette as a sheet: sessions, executions, artifacts, projects and files, then commands. */
export function SearchSheet({ ws, sessions, projects, open, onAsk, onSetup, toggleTheme }: SearchSheetProps) {
  const [query, setQuery] = useState("");
  const search = useSearch(ws, query);
  const local = useMemo(() => localItems(sessions, projects), [sessions, projects]);
  const commands = useMemo(() => paletteCommands("").filter((c) => c.id !== "cmd:keyboard-lock"), []);
  const hits = query.trim() && !search.loading && !search.unavailable ? search.items : null;
  const rows = mergePalette(query, hits, local, commands);

  const select = (id: string) => {
    const t = paletteTarget(id);
    if (t.kind === "open") return open(t.id, { anchor: t.anchor });
    if (t.command === "dock") onAsk();
    else if (t.command === "theme") toggleTheme();
    else if (t.command === "new-workspace") onSetup("new");
    else if (t.command === "add-project") onSetup("add");
  };

  return (
    <>
      <div style={{ flex: "none", padding: "4px 12px 10px", borderBottom: "1px solid var(--border-subtle)" }}>
        {/* biome-ignore lint/a11y/noAutofocus: the sheet opens to type a query. */}
        <input
          autoFocus
          className="m-input"
          type="search"
          aria-label="Search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && rows[0]) select(rows[0].id);
          }}
          placeholder="Go to a session, execution, artifact, file…"
        />
      </div>
      <div style={{ flex: 1, minHeight: 200, overflowY: "auto", padding: "4px 0 12px" }}>
        {rows.map((r, i) => (
          <div key={r.id}>
            {(i === 0 || rows[i - 1].group !== r.group) && (
              <div className="m-label" style={{ padding: "10px 16px 4px" }}>
                {r.group}
              </div>
            )}
            <button type="button" className="m-sheet-row" onClick={() => select(r.id)}>
              <Icon name={r.icon ?? "search"} size={16} style={{ color: "var(--text-muted)", flex: "none" }} />
              <span
                style={{ flex: 1, minWidth: 0, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}
              >
                {r.label}
              </span>
              {r.hint && (
                <span className="m-mono-sub" style={{ maxWidth: "40%", fontSize: 11 }}>
                  {r.hint}
                </span>
              )}
            </button>
          </div>
        ))}
        {query.trim() && search.loading && rows.length === 0 && (
          <span className="m-muted" style={{ display: "flex", gap: 8, padding: 16 }}>
            <Spinner size={11} /> Searching…
          </span>
        )}
        {rows.length === 0 && !search.loading && (
          <span className="m-muted" style={{ display: "block", padding: 16 }}>
            Nothing matches.
          </span>
        )}
      </div>
    </>
  );
}

type Turn = { question: string; execution: string | null; error: string | null };

type AskSheetProps = {
  ws: string;
  /** The session of the open screen, whose artifacts the answer may read. */
  session: { id: string; label: string } | null;
  seed: { text: string; n: number } | null;
  onTurnIntoTask: (question: string) => void;
};

/** The quick-question dock as a sheet: read-only answers from the `quick-answer` agent. */
export function AskSheet({ ws, session, seed, onTurnIntoTask }: AskSheetProps) {
  const [turns, setTurns] = useState<Turn[]>([]);
  const [question, setQuestion] = useState(seed?.text ?? "");
  const [busy, setBusy] = useState(false);
  const log = useRef<HTMLDivElement>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: scroll to the newest turn.
  useEffect(() => {
    log.current?.scrollTo?.({ top: log.current.scrollHeight });
  }, [turns.length]);

  const ask = async () => {
    const q = question.trim();
    if (!q || busy) return;
    setBusy(true);
    setQuestion("");
    const idx = turns.length;
    setTurns((t) => [...t, { question: q, execution: null, error: null }]);
    try {
      const started = await api.ask(ws, { question: q, session: session?.id ?? null });
      setTurns((t) => t.map((turn, i) => (i === idx ? { ...turn, execution: started.execution } : turn)));
    } catch (e) {
      setTurns((t) => t.map((turn, i) => (i === idx ? { ...turn, error: (e as Error).message } : turn)));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div
        style={{
          flex: "none",
          padding: "4px 16px 10px",
          display: "flex",
          flexDirection: "column",
          gap: 4,
          borderBottom: "1px solid var(--border-subtle)",
        }}
      >
        <span style={{ fontSize: 16, fontWeight: 600 }}>Ask a quick question</span>
        <span style={{ fontSize: 12, color: "var(--text-secondary)", textWrap: "pretty" }}>
          Answers are read-only. Ostra reads the code but changes nothing. Real work goes through a session.
        </span>
        {session && (
          <span style={{ display: "flex", gap: 6, alignItems: "center", fontSize: 12, color: "var(--text-muted)" }}>
            Context <Chip icon="git-pull-request">{session.label}</Chip>
          </span>
        )}
      </div>
      <div
        ref={log}
        style={{
          flex: 1,
          minHeight: 140,
          overflowY: "auto",
          padding: "12px 16px",
          display: "flex",
          flexDirection: "column",
          gap: 12,
        }}
      >
        {turns.length === 0 && (
          <span className="m-muted">No questions yet. Ask about the code, a stage or a decision.</span>
        )}
        {turns.map((t, i) => (
          <div key={i} style={{ display: "flex", flexDirection: "column", gap: 8, fontSize: 14, lineHeight: 1.6 }}>
            <div
              style={{
                alignSelf: "flex-end",
                maxWidth: "85%",
                padding: "8px 12px",
                borderRadius: 6,
                background: "var(--surface-selected)",
                lineHeight: 1.5,
                whiteSpace: "pre-wrap",
              }}
            >
              {t.question}
            </div>
            {t.error && (
              <Banner tone="bad" style={{ margin: 0 }}>
                {t.error}
              </Banner>
            )}
            {t.execution && <Answer execution={t.execution} question={t.question} onTurnIntoTask={onTurnIntoTask} />}
            {!t.execution && !t.error && (
              <span className="m-muted" style={{ display: "flex", gap: 8, alignItems: "center" }}>
                <Spinner size={12} /> Reading the code…
              </span>
            )}
          </div>
        ))}
      </div>
      <form
        style={{
          flex: "none",
          display: "flex",
          gap: 8,
          padding: "10px 12px 12px",
          borderTop: "1px solid var(--border-subtle)",
        }}
        onSubmit={(e) => {
          e.preventDefault();
          void ask();
        }}
      >
        <input
          className="m-input"
          aria-label="Question"
          value={question}
          onChange={(e) => setQuestion(e.target.value)}
          placeholder="Where is cancellation checked?"
        />
        <button
          type="submit"
          className="m-btn m-btn-primary"
          aria-label="Ask"
          disabled={busy || !question.trim()}
          style={{ width: 44, padding: 0, flex: "none" }}
        >
          <Icon name="arrow-up" size={18} />
        </button>
      </form>
    </>
  );
}
