import { useCallback, useEffect, useState } from "react";
import { api, HttpError } from "../../../api";
import type { ProjectFile } from "../../../api/types";
import type { Async } from "../../../lib/hooks";

/** An edit in progress. `base` is the hash the edit started from; `saved` is the text at that hash. */
type Draft = { draft: string; saved: string; base: string | null };

export type FileEdit = {
  editing: boolean;
  draft: string;
  dirty: boolean;
  saving: boolean;
  /** The file on disk no longer matches the hash the edit started from. */
  conflict: boolean;
  error: string | null;
  /** Discard was pressed once with unsaved changes; the next press discards. */
  confirmDiscard: boolean;
  start: () => void;
  change: (text: string) => void;
  save: () => void;
  /** Leave edit mode. With unsaved changes the first call only asks for confirmation. */
  discard: () => void;
  /** Take the disk version and drop the draft. */
  reload: () => void;
  /** Save the draft over the disk version. */
  overwrite: () => void;
};

const isHashConflict = (e: unknown) => e instanceof HttpError && e.status === 409 && e.issues.some((i) => i.path === "base_hash");

/**
 * Edit state for one project file. Every save carries the hash the edit started from, and the server refuses it
 * when the file changed since. The file also reloads on `project_fs_changed`, so a changed hash shows the conflict
 * before the user saves. A clean draft follows the disk version silently.
 */
export function useFileEdit(ws: string, projectKey: string, path: string, file: Async<ProjectFile>): FileEdit {
  const [edit, setEdit] = useState<Draft | null>(null);
  const [saving, setSaving] = useState(false);
  const [conflict, setConflict] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  const disk = file.data;
  const dirty = !!edit && edit.draft !== edit.saved;

  useEffect(() => {
    if (!edit || !disk || saving) return;
    if (disk.hash === edit.base) {
      setConflict(false);
    } else if (edit.draft === edit.saved && disk.hash !== null) {
      setEdit({ draft: disk.content ?? "", saved: disk.content ?? "", base: disk.hash });
      setConflict(false);
    } else {
      setConflict(true);
    }
    // The draft text is not a trigger: typing must not re-run the comparison.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [disk, edit?.base, saving]);

  useEffect(() => {
    if (!dirty) return;
    const warn = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [dirty]);

  const take = (f: ProjectFile) => {
    file.set(f);
    setEdit({ draft: f.content ?? "", saved: f.content ?? "", base: f.hash });
    setConflict(false);
    setError(null);
  };

  const send = useCallback(
    async (content: string, baseHash: string | null) => {
      setSaving(true);
      setError(null);
      try {
        const f = await api.saveProjectFile(ws, projectKey, { path, content, base_hash: baseHash });
        file.set(f);
        setEdit((e) => (e ? { ...e, saved: content, base: f.hash } : e));
        setConflict(false);
      } catch (e) {
        if (isHashConflict(e)) {
          setConflict(true);
          file.reload();
        } else {
          setError(e instanceof Error ? e.message : String(e));
        }
      } finally {
        setSaving(false);
      }
    },
    [ws, projectKey, path, file],
  );

  return {
    editing: !!edit,
    draft: edit?.draft ?? "",
    dirty,
    saving,
    conflict,
    error,
    confirmDiscard,
    start: () => {
      if (!disk || disk.hash === null) return;
      setEdit({ draft: disk.content ?? "", saved: disk.content ?? "", base: disk.hash });
      setConflict(false);
      setError(null);
      setConfirmDiscard(false);
    },
    change: (text) => {
      setEdit((e) => (e ? { ...e, draft: text } : e));
      setConfirmDiscard(false);
    },
    save: () => {
      if (edit && dirty && !saving && !conflict) void send(edit.draft, edit.base);
    },
    discard: () => {
      if (dirty && !confirmDiscard) return setConfirmDiscard(true);
      setEdit(null);
      setConflict(false);
      setError(null);
      setConfirmDiscard(false);
    },
    reload: () => {
      void api.projectFile(ws, projectKey, path).then(take, (e: unknown) => setError(e instanceof Error ? e.message : String(e)));
    },
    overwrite: () => {
      if (!edit) return;
      const content = edit.draft;
      void api.projectFile(ws, projectKey, path).then(
        (f) => {
          file.set(f);
          void send(content, f.hash);
        },
        (e: unknown) => setError(e instanceof Error ? e.message : String(e)),
      );
    },
  };
}
