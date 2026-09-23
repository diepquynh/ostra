import { useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { api, socket } from "../../api";
import type { ExecutionStatus } from "../../api/types";
import { Markdown } from "../../components/Markdown";
import { useLayout } from "../../components/Layout";
import { applyDelta, emptyActivity, foldActivity, type ActivityState } from "../../lib/events";

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

function AnswerView({ execution }: { execution: string }) {
  const [state, setState] = useState<ActivityState>(emptyActivity);
  const [status, setStatus] = useState<ExecutionStatus>("running");

  useEffect(() => {
    let alive = true;
    api
      .activity(execution)
      .then((items) => alive && setState((s) => foldActivity(items, s)))
      .catch(() => {});
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
  const tools = state.entries.filter((e) => e.kind === "tool" && !e.call.tool.startsWith("submit_")).length;
  return (
    <div>
      {text ? <Markdown text={text} /> : <div className="muted small">Reading the projects…</div>}
      <div className="muted small">
        {status === "running" ? "Answering" : status === "ok" ? "Done" : status}
        {tools > 0 && ` · ${tools} lookup${tools === 1 ? "" : "s"}`}
      </div>
    </div>
  );
}

export function AskPanel({ ws, session, onClose }: { ws: string; session: string | null; onClose: () => void }) {
  const [turns, setTurns] = useState<Turn[]>([]);
  const [question, setQuestion] = useState("");
  const [busy, setBusy] = useState(false);
  const { setTaskDraft } = useLayout();
  const navigate = useNavigate();

  const ask = async () => {
    const q = question.trim();
    if (!q) return;
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

  const turnIntoTask = (q: string) => {
    setTaskDraft(q);
    navigate(`/w/${ws}`);
  };

  return (
    <div className="ask-panel">
      <div className="ask-head row between">
        <div>
          <strong>Quick question</strong>
          <div className="muted small">
            Read-only answers outside the pipeline. Nothing here changes code or session state.
            {session && " This session's artifacts are included."}
          </div>
        </div>
        <button className="ghost small" onClick={onClose} aria-label="Close">
          Close
        </button>
      </div>
      <div className="ask-log">
        {turns.length === 0 && (
          <div className="muted small">
            Ask how something works, where a behavior lives, or what a library does. For a change, use the New task form so it goes
            through research, spec, and review.
          </div>
        )}
        {turns.map((t, i) => (
          <div key={i} className="stack">
            <div className="ask-q">{t.question}</div>
            {t.error && <div className="form-error">{t.error}</div>}
            {t.execution && <AnswerView execution={t.execution} />}
            <div>
              <button className="small" onClick={() => turnIntoTask(t.question)}>
                Turn into task
              </button>
            </div>
          </div>
        ))}
      </div>
      <form
        className="ask-form"
        onSubmit={(e) => {
          e.preventDefault();
          void ask();
        }}
      >
        <textarea
          value={question}
          rows={3}
          placeholder="Where is the order status changed?"
          onChange={(e) => setQuestion(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              void ask();
            }
          }}
        />
        <div className="row between">
          <span className="muted small">Ctrl+Enter to send</span>
          <button className="primary" disabled={busy || !question.trim()}>
            Ask
          </button>
        </div>
      </form>
    </div>
  );
}
