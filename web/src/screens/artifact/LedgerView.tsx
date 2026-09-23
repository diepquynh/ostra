import { lazy, Suspense, useEffect, useState } from "react";
import { api } from "../../api";
import type { DiffFile } from "../../api/types";
import { Chip, Panel, Spinner, Table, type Tone } from "../../design";
import { ledgerLoop, parseLedger, type LedgerFinding } from "../../lib/ledger";
import type { Theme } from "../../lib/nav";
import { inlineCode } from "../execution/inline";

const MonacoDiff = lazy(() => import("./MonacoDiff"));

const SEVERITY_TONE: Record<string, Tone> = { BLOCKER: "bad", HIGH: "bad", MEDIUM: "warn", LOW: "neutral" };

const sameFile = (a: string, b: string) => a.endsWith(b) || b.endsWith(a);

/** Session id from a path under `.ostra/sessions/<id>/`. */
export function sessionFromPath(path: string): string | null {
  return /\/\.ostra\/sessions\/([^/]+)\//.exec(path)?.[1] ?? null;
}

type Row = LedgerFinding & { key: string };

/** A review ledger: its findings as a table and as comments on the loop's Monaco diff. */
export function LedgerView({ path, content, session, theme }: { path: string; content: string; session: string | null; theme: Theme }) {
  const findings: Row[] = parseLedger(content).map((f, i) => ({ ...f, key: `${f.iteration}:${f.id || i}` }));
  const loop = ledgerLoop(path);
  const sid = session ?? sessionFromPath(path);
  const [diff, setDiff] = useState<DiffFile[] | null>(null);
  const [missing, setMissing] = useState(false);

  useEffect(() => {
    setDiff(null);
    setMissing(false);
    if (!sid || !loop?.project) {
      setMissing(true);
      return;
    }
    let alive = true;
    api.diff(sid, loop.project, loop.phase).then(
      (d) => alive && setDiff(d),
      () => alive && setMissing(true),
    );
    return () => {
      alive = false;
    };
  }, [sid, loop?.project, loop?.phase]);

  return (
    <div className="art-ledger">
      <p className="art-note">
        The reviewer appends one iteration per pass, and the fix agent records FIXED or WONTFIX against each finding. The loop stops when a pass
        comes back clean, or asks you after 3 passes.
      </p>
      {findings.length > 0 && (
        <Panel title="Findings" subtitle={findings.length} bodyFlush>
          <Table<Row>
            dense
            rowKey="key"
            columns={[
              { key: "id", label: "ID", width: 44, render: (r) => <span className="art-mono-strong">{r.id}</span> },
              { key: "iteration", label: "Pass", num: true, width: 44 },
              { key: "severity", label: "Severity", width: 84, render: (r) => <Chip tone={SEVERITY_TONE[r.severity] ?? "neutral"}>{r.severity}</Chip> },
              {
                key: "file",
                label: "Where",
                width: 200,
                render: (r) => (
                  <span className="art-where" title={r.file}>
                    <span className="art-mono">{r.file.split("/").pop()}</span>
                    <code style={{ alignSelf: "flex-start" }}>{r.rule}</code>
                  </span>
                ),
              },
              {
                key: "description",
                label: "Finding",
                render: (r) => (
                  <span className="art-finding">
                    <span>{inlineCode(r.description)}</span>
                    {r.fix && <span className="art-finding__fix">Fix: {inlineCode(r.fix)}</span>}
                  </span>
                ),
              },
            ]}
            rows={findings}
          />
        </Panel>
      )}
      {diff === null && !missing && (
        <div className="art-note">
          <Spinner size={11} /> Loading the diff for this loop…
        </div>
      )}
      {diff && diff.length > 0 && (
        <Suspense
          fallback={
            <div className="art-note">
              <Spinner size={11} /> Loading the diff viewer…
            </div>
          }
        >
          {diff.map((f) => {
            const mine = findings.filter((x) => sameFile(f.path, x.file));
            return (
              <Panel
                key={f.path}
                icon="file-diff"
                title={<span className="art-mono">{f.path}</span>}
                actions={
                  <span style={{ display: "flex", gap: 4 }}>
                    {mine.map((x) => (
                      <Chip key={x.key} tone={SEVERITY_TONE[x.severity] ?? "neutral"}>
                        {x.id || x.rule} {x.severity}
                      </Chip>
                    ))}
                  </span>
                }
                bodyFlush
              >
                <MonacoDiff file={f} findings={mine} theme={theme} />
              </Panel>
            );
          })}
        </Suspense>
      )}
      {(missing || (diff && diff.length === 0)) && <div className="art-note">No diff is available for this loop, so the ledger is shown as written.</div>}
    </div>
  );
}
