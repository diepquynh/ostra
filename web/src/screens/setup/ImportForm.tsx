import { useEffect, useState } from "react";
import { Button, FolderPicker, Input, Select } from "../../design";
import { listFolders, useFolderInfo } from "./folders";
import { basename, isChosen, keyError, stackOptions, suggestKey, type DraftProject, type ImportErrors } from "./wizard";

export type ImportFormProps = {
  /** Keys already used in the workspace (or the wizard's list). */
  takenKeys: string[];
  /** Paths already added, so the same folder is not added twice. Keyed by path, valued by key. */
  takenPaths?: Record<string, string>;
  /** With onAdd the form shows an "Add project" button and resets after each add. */
  onAdd?: (p: DraftProject) => void;
  /** Called with the project the form describes whenever it is complete and valid, else null. */
  onDraft?: (p: DraftProject | null) => void;
  /** Errors from the server for the draft's fields. */
  serverErrors?: ImportErrors;
  /** Stacks the server has a seed reference for. */
  stacks: string[];
  initialPath?: string;
  compact?: boolean;
};

/** Folder, key and stack for one project import. The key follows the folder name until the user edits it. */
export function ImportForm({ takenKeys, takenPaths = {}, onAdd, onDraft, serverErrors = {}, stacks, initialPath = "~/", compact }: ImportFormProps) {
  const [path, setPath] = useState(initialPath);
  const [keyEdit, setKeyEdit] = useState<string | null>(null);
  const [stack, setStack] = useState("");
  const info = useFolderInfo(path);
  const chosen = isChosen(path);
  const key = keyEdit ?? (chosen ? suggestKey(basename(path)) : "");
  const kErr = key || keyEdit !== null ? keyError(key, takenKeys) : null;
  const dupOf = takenPaths[path.replace(/\/+$/, "")];
  const pErr =
    info.state === "missing"
      ? `No folder at ${path}. Create the folder first, then import it.`
      : dupOf
        ? `This folder is already added as ${dupOf}.`
        : null;
  const ok = chosen && info.state === "exists" && !pErr && !kErr && key !== "";
  const draft: DraftProject | null =
    ok && info.state === "exists" ? { key, path: path.replace(/\/+$/, ""), stack, isGit: info.isGit, isOstraProject: info.isOstraProject } : null;

  const draftSig = draft ? JSON.stringify(draft) : "";
  useEffect(() => {
    onDraft?.(draft);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draftSig]);

  const add = () => {
    if (!draft) return;
    onAdd?.(draft);
    setPath(path.slice(0, path.replace(/\/+$/, "").lastIndexOf("/") + 1));
    setKeyEdit(null);
    setStack("");
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
      <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
        <FolderPicker value={path} onChange={setPath} list={listFolders} height={compact ? 150 : 170} />
        {(pErr || serverErrors.path) && (
          <span className="os-field__error" role="alert">
            {pErr ?? serverErrors.path}
          </span>
        )}
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(220px, 1fr))", gap: 12 }}>
        <Input
          label="Project key"
          mono
          value={key}
          onChange={(e) => setKeyEdit(e.target.value)}
          placeholder="backend"
          error={kErr ?? serverErrors.key}
          hint="Names the project in every stage and session folder. It cannot change later."
          onKeyDown={(e) => {
            if (e.key === "Enter" && onAdd) add();
          }}
        />
        <Select
          label="Stack (optional)"
          value={stack}
          onChange={(e) => setStack(e.target.value)}
          options={stackOptions(stacks, stack)}
          hint="For an empty folder, the stack seeds the first skills."
        />
      </div>
      {serverErrors.stack && (
        <span className="os-field__error" role="alert">
          {serverErrors.stack}
        </span>
      )}
      {onAdd && (
        <div>
          <Button icon="plus" disabled={!draft} onClick={add}>
            Add project
          </Button>
        </div>
      )}
    </div>
  );
}
