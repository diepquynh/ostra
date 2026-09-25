import { useState } from "react";
import { api } from "../../api";
import type { SessionSummary } from "../../api/types";
import { LANES } from "../../content/stages";
import { Button, Chip, Dialog, IconButton, Input, StatusChip, Switch } from "../../design";
import { formatCost, humanize } from "../../lib/format";
import { startedLabel } from "./board";

type Props = { summary: SessionSummary; onSummary: (s: SessionSummary) => void; onChanged: () => void };

const ended = (s: SessionSummary) => s.status === "completed" || s.status === "failed";

/** Title, request, status chips, and the session controls: YOLO, change the request, stop. */
export function SessionHeader({ summary: s, onSummary, onChanged }: Props) {
  const [dialog, setDialog] = useState<"amend" | "stop" | null>(null);
  const [yoloError, setYoloError] = useState<string | null>(null);
  const toggleYolo = () => {
    setYoloError(null);
    api.setYolo(s.id, !s.yolo).then(onSummary, (e: Error) => setYoloError(e.message));
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
              {s.request}
            </p>
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
          {!ended(s) && s.kind.kind === "pipeline" && (
            <Button size="sm" icon="pencil-line" onClick={() => setDialog("amend")}>
              Change the request
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
      {yoloError && <div className="os-field__error">{yoloError}</div>}
      {dialog === "amend" && (
        <AmendDialog
          id={s.id}
          onClose={() => setDialog(null)}
          onDone={(next) => {
            onSummary(next);
            onChanged();
          }}
        />
      )}
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

function AmendDialog({
  id,
  onClose,
  onDone,
}: {
  id: string;
  onClose: () => void;
  onDone: (s: SessionSummary) => void;
}) {
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const send = () => {
    if (!text.trim()) return setError("Write the change first.");
    setBusy(true);
    api
      .amend(id, text.trim())
      .then((s) => {
        onDone(s);
        onClose();
      })
      .catch((e: Error) => setError(e.message))
      .finally(() => setBusy(false));
  };
  return (
    <Dialog
      title="Change the request"
      subtitle="Ostra routes the change through the pipeline"
      onClose={onClose}
      footer={
        <>
          <span style={{ flex: 1 }} />
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" disabled={busy || !text.trim()} onClick={send}>
            Send the change
          </Button>
        </>
      }
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
        <p
          style={{
            margin: 0,
            fontSize: "var(--text-sm)",
            color: "var(--text-secondary)",
            lineHeight: "var(--leading-normal)",
          }}
        >
          A requirement change goes through research for the new part, then the spec is rewritten and approved again. If
          a plan exists, a new plan is written from the updated spec.
        </p>
        <Input
          multiline
          rows={4}
          label="What changes"
          value={text}
          error={error}
          onChange={(e) => setText(e.target.value)}
        />
      </div>
    </Dialog>
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
