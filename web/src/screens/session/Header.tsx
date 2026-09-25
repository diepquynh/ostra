import { useState } from "react";
import { api } from "../../api";
import type { ContextAddition, ContextFile, SessionSummary, UploadedFile } from "../../api/types";
import { LANES } from "../../content/stages";
import { Button, Chip, Dialog, IconButton, StatusChip, Switch } from "../../design";
import { TaggedText, UntaggedFiles } from "../../features/context/TaggedText";
import { UploadChip } from "../../features/context/uploads";
import { formatCost, humanize } from "../../lib/format";
import { startedLabel } from "./board";

type Props = {
  summary: SessionSummary;
  /** Files attached to the request at the start. */
  files: ContextFile[];
  /** Files uploaded with the request at the start. */
  uploads: UploadedFile[];
  additions: ContextAddition[];
  onSummary: (s: SessionSummary) => void;
  onChanged: () => void;
};

const ended = (s: SessionSummary) => s.status === "completed" || s.status === "failed";
const hhmm = (iso: string) => new Date(iso).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });

/** Title, request with its tagged files, added context, status chips, and the controls: YOLO, pause, stop. */
export function SessionHeader({ summary: s, files, uploads, additions, onSummary, onChanged }: Props) {
  const [dialog, setDialog] = useState<"stop" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const toggleYolo = () => {
    setError(null);
    api.setYolo(s.id, !s.yolo).then(onSummary, (e: Error) => setError(e.message));
  };
  const paused = s.status === "paused";
  const togglePause = () => {
    setError(null);
    setBusy(true);
    (paused ? api.resumeSession(s.id) : api.pauseSession(s.id))
      .then(
        (next) => {
          onSummary(next);
          onChanged();
        },
        (e: Error) => setError(e.message),
      )
      .finally(() => setBusy(false));
  };
  const category = s.kind.kind === "init" ? `Init ${s.kind.project}` : s.category ? humanize(s.category) : null;
  return (
    <>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 16, alignItems: "flex-start" }}>
        <div style={{ flex: "1 1 420px", minWidth: 0 }}>
          <h1 style={{ margin: "0 0 6px", font: "var(--type-title)", textWrap: "pretty" }}>{s.title ?? "Untitled"}</h1>
          {s.request && (
            <p
              style={{
                margin: "0 0 12px",
                fontSize: "var(--text-sm)",
                color: "var(--text-secondary)",
                lineHeight: "var(--leading-normal)",
              }}
            >
              <TaggedText text={s.request} files={files} /> <UntaggedFiles text={s.request} files={files} />
              {uploads.map((u) => (
                <UploadChip key={u.path} upload={u} />
              ))}
            </p>
          )}
          {additions.length > 0 && (
            <ul className="ctx-additions" aria-label="Context you added">
              {additions.map((a) => (
                <li key={a.at}>
                  <span className="ctx-additions__meta">
                    {a.delivery === "now" ? "Sent now" : "Queued"} · {hhmm(a.at)}
                  </span>
                  <TaggedText text={a.text} files={a.files} /> <UntaggedFiles text={a.text} files={a.files} />
                  {a.uploads.map((u) => (
                    <UploadChip key={u.path} upload={u} />
                  ))}
                </li>
              ))}
            </ul>
          )}
          <div
            style={{
              display: "flex",
              flexWrap: "wrap",
              gap: 6,
              alignItems: "center",
              fontSize: "var(--text-sm)",
              color: "var(--text-muted)",
            }}
          >
            {category && <Chip tone={s.kind.kind === "init" ? "info" : "neutral"}>{category}</Chip>}
            <StatusChip status={s.status} />
            <Chip tone="accent">
              {LANES[s.lane].title}: {s.stage_label}
            </Chip>
            {s.projects.map((p) => (
              <Chip key={p} mono outline>
                {p}
              </Chip>
            ))}
            <span style={{ fontFamily: "var(--font-mono)" }}>{formatCost(s.cost_usd)}</span>
            <span>· {startedLabel(s.created_at)}</span>
          </div>
        </div>
        <div style={{ display: "flex", gap: 10, alignItems: "center", flex: "none" }}>
          <Switch
            label="YOLO"
            tone="warn"
            checked={s.yolo}
            disabled={ended(s)}
            onChange={toggleYolo}
            title="Ostra answers every gate and permission ask itself, from the next gate or tool call on"
          />
          {!ended(s) && (
            <Button
              size="sm"
              icon={paused ? "play" : "pause"}
              variant={paused ? "primary" : "default"}
              disabled={busy}
              onClick={togglePause}
              title={
                paused
                  ? "Start the pipeline again. Each paused agent continues from where it stopped."
                  : "Stop every running agent and start nothing new until you continue. Each agent resumes where it stopped."
              }
            >
              {paused ? "Continue" : "Pause"}
            </Button>
          )}
          {!ended(s) && (
            <IconButton
              size="sm"
              variant="default"
              icon="square"
              label="Stop the session"
              onClick={() => setDialog("stop")}
            />
          )}
        </div>
      </div>
      {error && <div className="os-field__error">{error}</div>}
      {dialog === "stop" && (
        <StopDialog
          id={s.id}
          onClose={() => setDialog(null)}
          onDone={(next) => {
            onSummary(next);
            onChanged();
          }}
        />
      )}
    </>
  );
}

function StopDialog({ id, onClose, onDone }: { id: string; onClose: () => void; onDone: (s: SessionSummary) => void }) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const stop = () => {
    setBusy(true);
    api
      .stopSession(id)
      .then((s) => {
        onDone(s);
        onClose();
      })
      .catch((e: Error) => setError(e.message))
      .finally(() => setBusy(false));
  };
  return (
    <Dialog
      title="Stop the session?"
      onClose={onClose}
      width={460}
      footer={
        <>
          <span style={{ flex: 1 }} />
          <Button onClick={onClose}>Keep it running</Button>
          <Button variant="danger" disabled={busy} onClick={stop}>
            Stop the session
          </Button>
        </>
      }
    >
      <p style={{ margin: 0, lineHeight: "var(--leading-normal)" }}>
        Ostra cancels every running execution, denies any waiting permission ask, and marks the session failed. Files
        the agents already wrote stay in the project.
      </p>
      {error && <div className="os-field__error">{error}</div>}
    </Dialog>
  );
}
