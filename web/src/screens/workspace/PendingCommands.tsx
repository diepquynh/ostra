import { Banner, Button } from "@ostra/design";
import { useState } from "react";
import { api } from "../../api";
import type { PendingCommand, PendingCommands, PendingKind } from "../../api/types";

const KIND: Record<PendingKind, string> = {
  mcpServer: "MCP server",
  languageServer: "Language server",
  codeProvider: "Code provider",
  formatCommand: "Format command",
  allowRule: "Allow rule",
  projectOutside: "Project outside the workspace",
};

function describe(c: PendingCommand): string {
  const parts = [
    c.command ? `runs ${c.command}` : null,
    c.url ? `connects to ${c.url}` : null,
    c.path ? `lets agents write in ${c.path}` : null,
  ];
  if (c.env.length) parts.push(`sets ${c.env.join(", ")}`);
  if (c.headers.length) parts.push(`sends headers ${c.headers.join(", ")}`);
  if (c.variables.length) parts.push(`reads ${c.variables.map((v) => `$${v}`).join(", ")} from the server`);
  if (!c.enabled) parts.push("disabled");
  return parts.filter(Boolean).join("; ");
}

/**
 * One line above every workspace screen but Settings while folder-file commands wait for approval,
 * because a command an init agent or a pull wrote would otherwise wait unseen.
 */
export function PendingCommandsNotice({ pending, onReview }: { pending: PendingCommands[]; onReview: () => void }) {
  const count = pending.reduce((n, p) => n + p.items.length, 0);
  if (!count) return null;
  const files = pending.map((p) => p.file).join(", ");
  return (
    <div style={{ padding: "8px 16px 0" }}>
      <Banner
        tone="warn"
        actions={
          <Button size="sm" onClick={onReview}>
            Review in Settings
          </Button>
        }
      >
        {count === 1 ? "1 command waits" : `${count} commands wait`} for approval in {files}. Ostra runs none of them
        until you approve them, because they changed outside Ostra.
      </Banner>
    </div>
  );
}

/** Folder files whose commands changed outside Ostra. Nothing in them runs until approved. */
export function PendingCommandsBanner({
  ws,
  pending,
  onApproved,
}: {
  ws: string;
  pending: PendingCommands[];
  onApproved: () => void;
}) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const approve = async (p: PendingCommands) => {
    setBusy(p.hash);
    setError(null);
    try {
      await api.approveCommands(ws, { project: p.project ?? undefined, hash: p.hash });
      onApproved();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
    }
  };
  if (!pending.length) return null;
  return (
    <>
      {pending.map((p) => (
        <Banner
          key={p.hash}
          tone="bad"
          title={`Commands in ${p.file} wait for approval`}
          actions={
            <Button size="sm" variant="primary" disabled={busy !== null} onClick={() => void approve(p)}>
              {busy === p.hash ? "Approving…" : "Approve"}
            </Button>
          }
        >
          Ostra runs none of these until you approve them, because the file changed outside Ostra.
          <ul style={{ paddingLeft: 16 }}>
            {p.items.map((c, n) => (
              <li key={n}>
                {KIND[c.kind]} <code>{c.name}</code>: {describe(c) || "no command"}
              </li>
            ))}
          </ul>
        </Banner>
      ))}
      {error && <Banner tone="bad">{error}</Banner>}
    </>
  );
}
