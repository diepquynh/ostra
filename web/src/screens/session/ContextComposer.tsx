import { Button, Dialog, Panel } from "@ostra/design";
import { type KeyboardEvent, useState } from "react";
import { api } from "../../api";
import type { ContextDelivery, ContextFile, SessionSummary } from "../../api/types";
import { FileTagInput } from "../../features/context/FileTagInput";
import { useUploads } from "../../features/context/uploads";
import { isMac, modHint } from "../../lib/keys";

type Props = {
  ws: string;
  summary: SessionSummary;
  /** Projects whose files may be tagged: the session's projects. */
  projects: string[];
  /** Executions running now, which "Send now" interrupts. */
  running: number;
  onSent: (s: SessionSummary) => void;
};

/** Add text and tagged files to a running session, queued for the next step or sent now. */
export function ContextComposer({ ws, summary, projects, running, onSent }: Props) {
  const [text, setText] = useState("");
  const [files, setFiles] = useState<ContextFile[]>([]);
  const uploads = useUploads(ws);
  const [confirm, setConfirm] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const empty = !text.trim() && files.length === 0 && uploads.ids.length === 0;

  const send = (delivery: ContextDelivery) => {
    if (empty) return setError("Write the context first, tag a file with @, or upload one.");
    setBusy(true);
    setError(null);
    api
      .amend(summary.id, { text: text.trim(), files, uploads: uploads.ids, delivery })
      .then((s) => {
        setText("");
        uploads.clear();
        setConfirm(false);
        onSent(s);
      })
      .catch((e: Error) => setError(e.message))
      .finally(() => setBusy(false));
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Enter" && (isMac ? e.metaKey : e.ctrlKey)) {
      e.preventDefault();
      send("queue");
    }
  };

  return (
    <Panel title="Add context" subtitle="type @ to tag a file" icon="message-square">
      <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
        <FileTagInput
          ws={ws}
          projects={projects}
          label="Context"
          rows={3}
          value={text}
          onChange={setText}
          onFiles={setFiles}
          uploads={uploads.items}
          onUploadFiles={uploads.add}
          onRemoveUpload={uploads.remove}
          onKeyDown={onKeyDown}
          disabled={busy}
          placeholder="Refunds follow the same rule. See @backend/src/payments/refund.ts"
        />
        <div style={{ display: "flex", flexWrap: "wrap", gap: 10, alignItems: "center" }}>
          <span
            style={{
              flex: "1 1 280px",
              fontSize: "var(--text-sm)",
              color: "var(--text-muted)",
              lineHeight: "var(--leading-normal)",
            }}
          >
            Queued context reaches the agents at the next step, and running agents finish first. A requirement change
            goes through research, then the spec is rewritten and approved again.
          </span>
          <Button
            icon="octagon-alert"
            disabled={busy || uploads.busy || empty || running === 0}
            title={running === 0 ? "Nothing is running, so queued context is used right away" : undefined}
            onClick={() => setConfirm(true)}
          >
            Send now
          </Button>
          <Button
            variant="primary"
            icon="plus"
            kbd={modHint("↵")}
            disabled={busy || uploads.busy || empty}
            onClick={() => send("queue")}
          >
            Queue for the next step
          </Button>
        </div>
        {error && <div className="os-field__error">{error}</div>}
      </div>
      {confirm && (
        <Dialog
          title="Interrupt running work?"
          onClose={() => setConfirm(false)}
          width={480}
          footer={
            <>
              <span style={{ flex: 1 }} />
              <Button onClick={() => setConfirm(false)}>Keep them running</Button>
              <Button variant="danger" disabled={busy} onClick={() => send("now")}>
                Interrupt and send
              </Button>
            </>
          }
        >
          <p style={{ margin: 0, lineHeight: "var(--leading-normal)" }}>
            Ostra stops the {running} running execution{running === 1 ? "" : "s"} now. Each one starts again from the
            beginning with your context, so the progress it made in this run is lost. Files it already wrote stay in the
            project, and what it spent stays on the bill.
          </p>
          {error && <div className="os-field__error">{error}</div>}
        </Dialog>
      )}
    </Panel>
  );
}
