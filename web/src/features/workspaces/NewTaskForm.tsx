import { useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { api } from "../../api";
import type { ProjectView, SessionSummary } from "../../api/types";

export function NewTaskForm({
  ws,
  projects,
  yoloDefault,
  draft,
  onDraftUsed,
  onCreated,
}: {
  ws: string;
  projects: ProjectView[];
  yoloDefault: boolean;
  draft: string | null;
  onDraftUsed: () => void;
  onCreated: (s: SessionSummary) => void;
}) {
  const [request, setRequest] = useState("");
  const [tests, setTests] = useState(false);
  const [docs, setDocs] = useState(false);
  const [yolo, setYolo] = useState(yoloDefault);
  const [pins, setPins] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const navigate = useNavigate();

  useEffect(() => {
    if (draft) {
      setRequest(draft);
      onDraftUsed();
    }
  }, [draft, onDraftUsed]);

  const initialized = projects.filter((p) => p.init_status === "initialized");

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const s = await api.createSession(ws, { request: request.trim(), options: { tests, docs, yolo }, projects: pins });
      onCreated(s);
      setRequest("");
      navigate(`/w/${ws}/s/${s.id}`);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="card">
      <h2>New task</h2>
      <p className="muted small">
        Describe the work. Ostra classifies it, researches the code, writes a spec for your approval, plans phases, builds and reviews
        each one, and asks before any optional stage. For a quick question, use the side panel instead.
      </p>
      <textarea
        rows={4}
        value={request}
        onChange={(e) => setRequest(e.target.value)}
        placeholder="Add order cancellation: customers can cancel until the order ships."
      />
      <div className="row mt">
        <label className="check" title="Tests run after every phase passes review">
          <input type="checkbox" checked={tests} onChange={(e) => setTests(e.target.checked)} /> Write tests
        </label>
        <label className="check" title="Refresh the area references once every phase passes review">
          <input type="checkbox" checked={docs} onChange={(e) => setDocs(e.target.checked)} /> Update docs
        </label>
        <label className="check" title="Ostra answers every question and permission ask itself and lists each decision at the end">
          <input type="checkbox" checked={yolo} onChange={(e) => setYolo(e.target.checked)} /> YOLO
        </label>
        {initialized.length > 1 && (
          <>
            <span className="muted small">Projects:</span>
            {initialized.map((p) => (
              <label key={p.key} className="check small">
                <input
                  type="checkbox"
                  checked={pins.includes(p.key)}
                  onChange={(e) => setPins(e.target.checked ? [...pins, p.key] : pins.filter((k) => k !== p.key))}
                />
                {p.key}
              </label>
            ))}
            <span className="muted small">(none pinned lets Ostra choose)</span>
          </>
        )}
        <span className="spacer" />
        <button className="primary" disabled={busy || !request.trim() || initialized.length === 0} onClick={() => void submit()}>
          Start
        </button>
      </div>
      {yolo && (
        <div className="banner mt small">
          YOLO: every permission is granted and every gate is answered by Ostra. Guards, deny rules, the fact-check PASS requirement,
          and security blocks still apply. The completion report lists every decision made for you.
        </div>
      )}
      {error && <div className="form-error mt">{error}</div>}
    </div>
  );
}
