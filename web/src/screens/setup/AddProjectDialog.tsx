import { useState } from "react";
import { api, HttpError } from "../../api";
import type { WorkspaceDetail } from "../../api/types";
import { Banner, Button, Dialog } from "../../design";
import { useWorkspace } from "../../lib/nav";
import { useHome } from "./folders";
import { ImportForm } from "./ImportForm";
import { expandHome, importErrors, projectStart, type DraftProject, type ImportErrors } from "./wizard";

export type AddProjectDialogProps = {
  ws: string;
  onClose: () => void;
  /** The project was imported: the updated workspace and the new project's key. */
  onAdded: (detail: WorkspaceDetail, key: string) => void;
};

/** Add a project to the active workspace, like /add-dir: pick a folder, confirm the key and stack, import. */
export function AddProjectDialog({ ws, onClose, onAdded }: AddProjectDialogProps) {
  const { detail } = useWorkspace();
  const home = useHome();
  const existing = detail?.projects ?? [];
  const [draft, setDraft] = useState<DraftProject | null>(null);
  const [busy, setBusy] = useState(false);
  const [fields, setFields] = useState<ImportErrors>({});
  const [general, setGeneral] = useState<string | null>(null);

  const submit = () => {
    if (!draft) return;
    setBusy(true);
    setFields({});
    setGeneral(null);
    api.importProject(ws, { path: expandHome(draft.path, home), key: draft.key, stack: draft.stack || null }).then(
      (d) => onAdded(d, draft.key),
      (e: Error) => {
        setBusy(false);
        const placed = importErrors(e instanceof HttpError ? e.issues : [], e.message);
        setFields(placed.fields);
        setGeneral(placed.general);
      },
    );
  };

  return (
    <Dialog
      title="Add a project"
      subtitle={`to ${detail?.settings.name ?? ws}. Like /add-dir: the folder is referenced by path, never copied.`}
      onClose={onClose}
      width={640}
      footer={
        <>
          <span style={{ flex: 1 }} />
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" icon="folder-plus" disabled={!draft || busy} onClick={submit}>
            {busy ? "Importing…" : "Import project"}
          </Button>
        </>
      }
    >
      {general && <Banner tone="bad">{general}</Banner>}
      <ImportForm
        compact
        stacks={detail?.stacks ?? []}
        takenKeys={existing.map((p) => p.key)}
        takenPaths={Object.fromEntries(existing.map((p) => [p.path, p.key]))}
        initialPath={detail ? projectStart(detail.root) : "~/"}
        onDraft={(d) => {
          setDraft(d);
          setFields({});
          setGeneral(null);
        }}
        serverErrors={fields}
      />
    </Dialog>
  );
}
