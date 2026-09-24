import { useEffect, useRef, useState, type ReactNode } from "react";
import { api, socket } from "../api";
import type { ExecutionStatus } from "../api/types";
import { Markdown } from "../components/Markdown";
import { Banner, Button, Icon, IconButton, Input, Kbd, Spinner } from "../design";
import { applyDelta, emptyActivity, foldActivity, type ActivityState } from "../lib/events";
import { modKeys } from "../lib/keys";

type Turn = { question: string; execution: string | null; error: string | null };

/** The answer text: the submitted answer when present, else the streamed text so far. */
export function answerText(state: ActivityState): string {
  for (let i = state.entries.length - 1; i >= 0; i--) {
    const e = state.entries[i];
    if (e.kind === "tool" && e.call.tool === "submit_quick_answer") {
      const input = e.call.input as { answer?: unknown };
      if (typeof input.answer === "string") return input.answer;
    }
  }
  return state.entries
    .filter((e) => e.kind === "text")
    .map((e) => (e.kind === "text" ? e.text : ""))
    .join("\n\n");
}

function Answer({ execution, question, onTurnIntoTask }: { execution: string; question: string; onTurnIntoTask: (q: string) => void }) {
  const [state, setState] = useState<ActivityState>(emptyActivity);
  const [status, setStatus] = useState<ExecutionStatus>("running");

  useEffect(() => {
    let alive = true;
    api.activity(execution).then(
      (items) => alive && setState((s) => foldActivity(items, s)),
      () => {},
    );
    const unsub = socket().subscribe(`execution:${execution}`, (m) => {
      if (m.type === "execution_delta") setState((s) => applyDelta(s, m.seq, m.delta));
      if (m.type === "execution_status") setStatus(m.status);
    });
    return () => {
      alive = false;
      unsub();
    };
  }, [execution]);

  const text = answerText(state);
  const lookups = state.entries.filter((e) => e.kind === "tool" && !e.call.tool.startsWith("submit_")).length;
  const done = status !== "running";
  return (
    <>
      {text ? (
        <Markdown text={text} className="os-prose" />
      ) : (
        <div style={{ display: "flex", gap: 8, alignItems: "center", color: "var(--text-muted)" }}>
          <Spinner size={11} /> Reading the projects…
        </div>
      )}
      <div style={{ display: "flex", alignItems: "center", gap: 8, fontSize: "var(--text-xs)", color: "var(--text-muted)" }}>
        <span>
          {done ? (status === "ok" ? "Done" : `Ended: ${status}`) : "Answering"}
          {lookups > 0 && ` · ${lookups} lookup${lookups === 1 ? "" : "s"}`}
        </span>
        <span style={{ flex: 1 }} />
        <Button size="sm" variant="ghost" icon="corner-down-right" onClick={() => onTurnIntoTask(question)}>
          Turn into task
        </Button>
      </div>
    </>
  );
}

export type QuickDockProps = {
  ws: string;
  /** The drag handle on the left edge. */
  resizer?: ReactNode;
  /** Session whose artifacts the answer may read: the active tab's session. */
  session: string | null;
  /** Text to put in the question box when the dock opens (from "Ask about this file"). */
  seed: { text: string; n: number } | null;
  onClose: () => void;
  onTurnIntoTask: (question: string) => void;
};

/** The quick-question dock (⌘/): read-only answers from the `quick-answer` agent, outside the pipeline. */
export function QuickDock({ ws, resizer, session, seed, onClose, onTurnIntoTask }: QuickDockProps) {
  const [turns, setTurns] = useState<Turn[]>([]);
  const [question, setQuestion] = useState("");
  const [busy, setBusy] = useState(false);
  const input = useRef<HTMLInputElement & HTMLTextAreaElement>(null);
  const log = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (seed) setQuestion(seed.text);
    input.current?.focus();
  }, [seed]);
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
      const started = await api.ask(ws, { question: q, session });
      setTurns((t) => t.map((turn, i) => (i === idx ? { ...turn, execution: started.execution } : turn)));
    } catch (e) {
      setTurns((t) => t.map((turn, i) => (i === idx ? { ...turn, error: (e as Error).message } : turn)));
    } finally {
      setBusy(false);
    }
  };

  return (
    <aside
      aria-label="Quick question"
      style={{ position: "relative", width: "var(--dock-w)", flex: "none", background: "var(--surface-panel)", borderLeft: "1px solid var(--border-default)", display: "flex", flexDirection: "column", minHeight: 0 }}
    >
      <div style={{ height: "var(--tabbar-h)", display: "flex", alignItems: "center", gap: 8, padding: "0 6px 0 12px", borderBottom: "1px solid var(--border-default)", background: "var(--surface-chrome)", flex: "none" }}>
        <Icon name="message-square" size={14} style={{ color: "var(--text-muted)" }} />
        <span style={{ fontWeight: 600, flex: 1 }}>Quick question</span>
        <IconButton size="sm" icon="x" label="Close" onClick={onClose} />
      </div>
      <div style={{ padding: "8px 12px", fontSize: "var(--text-sm)", color: "var(--text-muted)", borderBottom: "1px solid var(--border-subtle)", lineHeight: 1.45, flex: "none" }}>
        Read-only answers outside the pipeline. Nothing here changes code or session state.{session && " This session's artifacts are included."}
      </div>
      <div ref={log} style={{ flex: 1, overflow: "auto", padding: 12, display: "flex", flexDirection: "column", gap: 16 }}>
        {turns.length === 0 && (
          <div style={{ color: "var(--text-muted)", fontSize: "var(--text-sm)", lineHeight: 1.5 }}>
            Ask how something works, where a behavior lives, or what a library does. For a change, use the New task form so it goes through research, spec, and review.
          </div>
        )}
        {turns.map((t, i) => (
          <div key={i} style={{ display: "flex", flexDirection: "column", gap: 8 }}>
            <div style={{ alignSelf: "flex-end", maxWidth: "88%", background: "var(--surface-active)", padding: "6px 10px", borderRadius: 6, whiteSpace: "pre-wrap" }}>{t.question}</div>
            {t.error && (
              <Banner tone="bad" style={{ margin: 0 }}>
                {t.error}
              </Banner>
            )}
            {t.execution && <Answer execution={t.execution} question={t.question} onTurnIntoTask={onTurnIntoTask} />}
            {!t.execution && !t.error && (
              <div style={{ display: "flex", gap: 8, alignItems: "center", color: "var(--text-muted)" }}>
                <Spinner size={11} /> Starting…
              </div>
            )}
          </div>
        ))}
      </div>
      <form
        style={{ padding: 10, borderTop: "1px solid var(--border-default)", display: "flex", flexDirection: "column", gap: 8, flex: "none" }}
        onSubmit={(e) => {
          e.preventDefault();
          void ask();
        }}
      >
        <Input
          ref={input}
          multiline
          rows={2}
          aria-label="Question"
          placeholder="Where is the order status changed?"
          value={question}
          onChange={(e) => setQuestion(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              void ask();
            }
          }}
        />
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
          <span style={{ fontSize: "var(--text-xs)", color: "var(--text-muted)", display: "flex", gap: 5, alignItems: "center" }}>
            <Kbd keys={modKeys("↵")} /> to send
          </span>
          <Button type="submit" size="sm" variant="primary" disabled={busy || !question.trim()}>
            Ask
          </Button>
        </div>
      </form>
      {resizer}
    </aside>
  );
}
