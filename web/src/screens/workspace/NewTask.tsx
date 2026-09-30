import { Banner, Button, Checkbox, Panel, Select } from "@ostra/design";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import type { BookSummary } from "../../api/gen/BookSummary";
import type { ContextFile, ProjectView, SessionSummary, Track } from "../../api/types";
import { FileTagInput } from "../../features/context/FileTagInput";
import { useUploads } from "../../features/context/uploads";
import { useAsync } from "../../lib/hooks";
import { isMac, modHint } from "../../lib/keys";
import { useShell } from "../../lib/nav";

export type NewTaskProps = {
  ws: string;
  projects: ProjectView[];
  yoloDefault: boolean;
  onCreated: (s: SessionSummary) => void;
};

/** Request text with `@` file tags, the tests, docs and YOLO toggles, and optional pinned projects. */
export function NewTask({ ws, projects, yoloDefault, onCreated }: NewTaskProps) {
  const { taskDraft, setTaskDraft } = useShell();
  const [request, setRequest] = useState("");
  const [tests, setTests] = useState(false);
  const [docs, setDocs] = useState(false);
  const [book, setBook] = useState("");
  const books = useAsync<BookSummary[]>(() => (docs ? api.books(ws) : Promise.resolve([])), [ws, docs]);
  const [yolo, setYolo] = useState(yoloDefault);
  const [track, setTrack] = useState<Track | null>(null);
  const [pins, setPins] = useState<string[]>([]);
  const [files, setFiles] = useState<ContextFile[]>([]);
  const uploads = useUploads(ws);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const field = useRef<HTMLDivElement>(null);

  // "Turn into task" from the quick dock: take the text once, then clear it so a later visit starts empty.
  useEffect(() => {
    if (taskDraft === null) return;
    setRequest(taskDraft);
    setTaskDraft(null);
    field.current?.focus();
  }, [taskDraft, setTaskDraft]);

  useEffect(() => setYolo(yoloDefault), [yoloDefault]);

  const initialized = projects.filter((p) => p.init_status === "initialized");
  const canStart = !busy && !uploads.busy && request.trim() !== "" && initialized.length > 0;

  const submit = async () => {
    if (!canStart) return;
    setBusy(true);
    setError(null);
    try {
      const s = await api.createSession(ws, {
        request: request.trim(),
        options: track ? { tests, docs, yolo, track } : { tests, docs, yolo },
        projects: pins,
        files,
        uploads: uploads.ids,
        ...(docs && book ? { docs_book: book } : {}),
      });
      setRequest("");
      uploads.clear();
      setPins([]);
      onCreated(s);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Enter" && (isMac ? e.metaKey : e.ctrlKey)) {
      e.preventDefault();
      void submit();
    }
  };

  const pin = (k: string) => setPins((p) => (p.includes(k) ? p.filter((x) => x !== k) : [...p, k]));

  return (
    <Panel title="New task" icon="plus">
      <div className="wp-stack" style={{ gap: 10 }}>
        <div className="wp-muted" style={{ color: "var(--text-secondary)", lineHeight: 1.45 }}>
          Describe the work. Ostra classifies it, researches the code, and builds and reviews it. A change that needs
          settled requirements gets a spec and a plan for your approval first. You try the result and send feedback
          until you accept it.
        </div>
        <FileTagInput
          ref={field}
          ws={ws}
          projects={initialized.map((p) => p.key)}
          rows={3}
          label="Request"
          value={request}
          onChange={setRequest}
          onFiles={setFiles}
          uploads={uploads.items}
          onUploadFiles={uploads.add}
          onRemoveUpload={uploads.remove}
          onKeyDown={onKeyDown}
          placeholder="Add order cancellation: customers can cancel until the order ships. Type @ to tag a file."
        />
        <div className="wp-row" style={{ gap: 14 }}>
          <Checkbox
            label="Write tests"
            title="Tests are written after every phase passes review"
            checked={tests}
            onChange={(e) => setTests(e.target.checked)}
          />
          <Checkbox
            label="Write docs"
            title="A documentation book for the changed projects is written or updated once every phase passes review"
            checked={docs}
            onChange={(e) => setDocs(e.target.checked)}
          />
          {docs && (
            <Select
              size="sm"
              aria-label="Documentation book"
              title="The book this session writes into"
              value={book}
              onChange={(e) => setBook(e.target.value)}
              options={[
                { value: "", label: "Book: new or matching the projects" },
                ...(books.data ?? []).map((b) => ({ value: b.id, label: `Book: ${b.id}` })),
              ]}
            />
          )}
          <Checkbox
            label="YOLO"
            title="Ostra answers every question and permission ask itself and lists each decision at the end"
            checked={yolo}
            onChange={(e) => setYolo(e.target.checked)}
          />
          <span className="wp-divider" />
          <span className="wp-muted">Track</span>
          {([null, "light", "full"] as const).map((t) => (
            <Button
              key={t ?? "auto"}
              size="sm"
              variant={track === t ? "default" : "ghost"}
              active={track === t}
              title={
                t === null
                  ? "Ostra picks after research: light unless the change needs a spec and a plan"
                  : t === "light"
                    ? "Research, then build and review, with no spec or plan"
                    : "Spec, fact-check, and plan, each approved by you, before building"
              }
              onClick={() => setTrack(t)}
            >
              {t === null ? "Auto" : t === "light" ? "Light" : "Full"}
            </Button>
          ))}
          {initialized.length > 1 && (
            <>
              <span className="wp-divider" />
              <span className="wp-muted">Projects</span>
              {initialized.map((p) => (
                <Button
                  key={p.key}
                  size="sm"
                  variant={pins.includes(p.key) ? "default" : "ghost"}
                  active={pins.includes(p.key)}
                  onClick={() => pin(p.key)}
                  style={{ fontFamily: "var(--font-mono)", fontWeight: 400 }}
                >
                  {p.key}
                </Button>
              ))}
              {pins.length === 0 && (
                <span className="wp-muted" style={{ fontSize: "var(--text-xs)" }}>
                  none pinned lets Ostra choose
                </span>
              )}
            </>
          )}
          <span className="wp-spacer" />
          <Button variant="primary" icon="play" kbd={modHint("↵")} disabled={!canStart} onClick={() => void submit()}>
            {busy ? "Starting…" : "Start"}
          </Button>
        </div>
        {yolo && (
          <Banner tone="warn">
            YOLO: every permission is granted and every gate is answered by Ostra. Guards, deny rules, the fact-check
            PASS requirement, and security blocks still apply. The completion report lists every decision made for you.
          </Banner>
        )}
        {error && <Banner tone="bad">{error}</Banner>}
      </div>
    </Panel>
  );
}
