import { lazy, Suspense, useEffect, useState } from "react";
import { useParams, useSearchParams } from "react-router";
import { api } from "../../api";
import type { DiffFile } from "../../api/extra";
import { useCrumbs } from "../../components/Layout";
import { Markdown } from "../../components/Markdown";
import { Chip, ErrorBox, Loading } from "../../components/Status";
import { EARS_NOTE } from "../../content/stages";
import { basename } from "../../lib/format";
import { useAsync, useStoredFlag } from "../../lib/hooks";
import { ledgerLoop, parseLedger } from "../../lib/ledger";

const DiffView = lazy(() => import("./DiffView"));

export type ArtifactKind = "spec" | "plan" | "phase" | "research" | "ledger" | "report";

export function artifactKind(path: string): ArtifactKind {
  const name = basename(path);
  if (/^ostra-review-ledger/.test(name)) return "ledger";
  if (/^ostra-spec-/.test(name)) return "spec";
  if (/^ostra-plan-.*-phase-\d+/.test(name)) return "phase";
  if (/^ostra-plan-/.test(name)) return "plan";
  if (/^ostra-research-/.test(name)) return "research";
  return "report";
}

/** Session id from a path under `.ostra/sessions/<id>/`. */
export function sessionFromPath(path: string): string | null {
  return /\/\.ostra\/sessions\/([^/]+)\//.exec(path)?.[1] ?? null;
}

function EarsNote() {
  const [seen, setSeen] = useStoredFlag("ostra.earsNoteSeen", false);
  const [open, setOpen] = useState(!seen);
  useEffect(() => {
    if (!seen) setSeen(true);
  }, [seen, setSeen]);
  return (
    <details className="card" open={open} onToggle={(e) => setOpen((e.target as HTMLDetailsElement).open)}>
      <summary>
        <strong>{EARS_NOTE.title}</strong>
      </summary>
      <Markdown text={EARS_NOTE.body} />
    </details>
  );
}

function LedgerView({ path, content }: { path: string; content: string }) {
  const findings = parseLedger(content);
  const loop = ledgerLoop(path);
  const session = sessionFromPath(path);
  const [diff, setDiff] = useState<DiffFile[] | null>(null);
  const [diffMissing, setDiffMissing] = useState(false);
  useEffect(() => {
    if (!session || !loop?.project) {
      setDiffMissing(true);
      return;
    }
    api
      .diff(session, loop.project, loop.phase)
      .then(setDiff)
      .catch(() => setDiffMissing(true));
  }, [session, loop?.project, loop?.phase]);

  return (
    <div className="stack">
      <p className="small muted">
        The reviewer appends one iteration per pass; the fix agent records FIXED or WONTFIX against each finding. The loop stops when a
        pass comes back clean, or asks you after 3 passes.
      </p>
      {diff && diff.length > 0 ? (
        <Suspense fallback={<Loading label="Loading diff viewer" />}>
          {diff.map((f) => (
            <div key={f.path} className="stack">
              <div className="row">
                <span className="mono">{f.path}</span>
                {findings.filter((x) => f.path.endsWith(x.file) || x.file.endsWith(f.path)).map((x, i) => (
                  <Chip key={i} tone={x.severity === "HIGH" ? "bad" : x.severity === "MEDIUM" ? "warn" : ""}>
                    {x.id || x.rule} {x.severity}
                  </Chip>
                ))}
              </div>
              <DiffView file={f} findings={findings.filter((x) => f.path.endsWith(x.file) || x.file.endsWith(f.path))} />
            </div>
          ))}
        </Suspense>
      ) : (
        diffMissing && <div className="small muted">No diff is available for this loop, so the ledger is shown as written.</div>
      )}
      <div className="card">
        <Markdown text={content} />
      </div>
    </div>
  );
}

export function ArtifactPage() {
  const { ws = "" } = useParams();
  const [params] = useSearchParams();
  const path = params.get("path") ?? "";
  const art = useAsync(() => api.artifact(path), [path]);
  const kind = artifactKind(path);
  const session = sessionFromPath(path);
  useCrumbs([
    { label: "Workspace", to: `/w/${ws}` },
    ...(session ? [{ label: "Session", to: `/w/${ws}/s/${session}` }] : []),
    { label: basename(path) },
  ]);

  if (!path) return <div className="empty">No artifact path given.</div>;
  if (art.error) return <ErrorBox error={art.error} onRetry={art.reload} />;
  if (!art.data) return <Loading />;

  return (
    <div className="page narrow" style={{ maxWidth: 1000 }}>
      <div className="row between mb">
        <div>
          <Chip tone="accent">{kind}</Chip> <span className="mono small muted">{path}</span>
        </div>
        <button className="small" onClick={art.reload}>
          Reload
        </button>
      </div>
      {(kind === "spec" || kind === "plan" || kind === "phase") && <EarsNote />}
      {kind === "ledger" ? (
        <LedgerView path={path} content={art.data.content} />
      ) : (
        <div className="card">
          <Markdown text={art.data.content} />
        </div>
      )}
    </div>
  );
}
