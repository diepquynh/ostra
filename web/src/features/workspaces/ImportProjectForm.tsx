import { useState } from "react";
import { api } from "../../api";
import type { ProjectView, WorkspaceDetail } from "../../api/types";
import { FolderPicker } from "../../components/FolderPicker";
import { basename } from "../../lib/format";

export const STACKS = ["", "go", "java-spring", "python", "typescript-node"];

/** Same slug rule as the server: lowercase letters, digits, and dashes. */
export function suggestKey(folder: string): string {
  const slug = folder
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug || "project";
}

export const isProjectKey = (k: string) => /^[a-z0-9][a-z0-9-]*$/.test(k);

export function ImportProjectForm({
  ws,
  existing,
  onImported,
}: {
  ws: string;
  existing: ProjectView[];
  onImported: (d: WorkspaceDetail) => void;
}) {
  const [path, setPath] = useState("");
  const [key, setKey] = useState("");
  const [keyTouched, setKeyTouched] = useState(false);
  const [stack, setStack] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const effectiveKey = keyTouched ? key : path ? suggestKey(basename(path)) : "";
  const keyError = !effectiveKey
    ? null
    : !isProjectKey(effectiveKey)
      ? "Use lowercase letters, digits, and dashes, starting with a letter or digit."
      : existing.some((p) => p.key === effectiveKey)
        ? "This key is already used in the workspace."
        : null;

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const d = await api.importProject(ws, { path: path.trim(), key: effectiveKey, stack: stack || null });
      onImported(d);
      setPath("");
      setKey("");
      setKeyTouched(false);
      setStack("");
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="stack">
      <label className="field">
        <span className="label">Project folder</span>
      </label>
      <FolderPicker value={path} onChange={setPath} />
      <div className="grid-2">
        <label className="field">
          <span className="label">Project key</span>
          <input
            value={effectiveKey}
            onChange={(e) => {
              setKeyTouched(true);
              setKey(e.target.value);
            }}
            placeholder="backend"
          />
          <span>Names the project in every stage and in session folders. It cannot change later.</span>
          {keyError && <span className="form-error">{keyError}</span>}
        </label>
        <label className="field">
          <span className="label">Stack (optional)</span>
          <select value={stack} onChange={(e) => setStack(e.target.value)}>
            {STACKS.map((s) => (
              <option key={s} value={s}>
                {s || "Detect from the code"}
              </option>
            ))}
          </select>
          <span>For an empty folder, the stack seeds the first skills.</span>
        </label>
      </div>
      {error && <div className="form-error">{error}</div>}
      <div>
        <button className="primary" disabled={busy || !path.trim().startsWith("/") || !effectiveKey || Boolean(keyError)} onClick={() => void submit()}>
          Import project
        </button>
      </div>
    </div>
  );
}
