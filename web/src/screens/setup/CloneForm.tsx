import { FolderPicker, Input, Select } from "@ostra/design";
import { useState } from "react";
import type { CloneProject, GitCredentialView } from "../../api/types";
import { listFolders, makeFolder } from "./folders";
import { StackInput } from "./StackInput";
import { keyError, repoName, suggestKey } from "./wizard";

export type CloneErrors = Partial<Record<"url" | "key" | "path" | "stack" | "branch" | "credential", string>>;

export type CloneFormProps = {
  takenKeys: string[];
  stacks: string[];
  root: string;
  credentials: GitCredentialView[] | null;
  serverErrors: CloneErrors;
  /** The request the form describes whenever the URL and key are filled in and valid, else null. */
  onDraft: (req: CloneProject | null) => void;
  disabled?: boolean;
};

/** URL, key, branch, credential, folder, and stack for cloning one repository into the workspace. */
export function CloneForm({ takenKeys, stacks, root, credentials, serverErrors, onDraft, disabled }: CloneFormProps) {
  const [url, setUrl] = useState("");
  const [keyEdit, setKeyEdit] = useState<string | null>(null);
  const [branch, setBranch] = useState("");
  const [credential, setCredential] = useState("");
  const [path, setPath] = useState("");
  const [stack, setStack] = useState("");
  const name = repoName(url);
  const key = keyEdit ?? (name ? suggestKey(name) : "");
  const kErr = key || keyEdit !== null ? keyError(key, takenKeys) : null;
  const dest = path.trim().replace(/(.)\/+$/, "$1") || `${root.replace(/\/+$/, "")}/${key || "<key>"}`;

  const emit = (
    next: Partial<{ url: string; key: string; branch: string; credential: string; path: string; stack: string }>,
  ) => {
    const v = { url, key, branch, credential, path, stack, ...next };
    const ok = v.url.trim() !== "" && v.key !== "" && !keyError(v.key, takenKeys);
    onDraft(
      ok
        ? {
            url: v.url.trim(),
            key: v.key,
            branch: v.branch.trim() || undefined,
            credential: v.credential || undefined,
            path: v.path.trim().replace(/(.)\/+$/, "$1") || undefined,
            stack: v.stack || undefined,
          }
        : null,
    );
  };

  const credOptions = [
    { value: "", label: "Match by host" },
    ...(credentials ?? []).map((c) => ({
      value: c.id,
      label: `${c.label} (${c.kind === "ssh" ? "SSH" : "token"}, ${c.host})`,
    })),
  ];

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
      <Input
        label="Repository URL"
        mono
        autoFocus
        disabled={disabled}
        placeholder="https://github.com/acme/shop.git or git@github.com:acme/shop.git"
        value={url}
        error={serverErrors.url}
        onChange={(e) => {
          setUrl(e.target.value);
          const k = keyEdit ?? (repoName(e.target.value) ? suggestKey(repoName(e.target.value)) : "");
          emit({ url: e.target.value, key: k });
        }}
      />
      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(220px, 1fr))", gap: 12 }}>
        <Input
          label="Project key"
          mono
          disabled={disabled}
          value={key}
          placeholder="shop"
          error={kErr ?? serverErrors.key}
          hint="Names the project in every stage and session folder. It cannot change later."
          onChange={(e) => {
            setKeyEdit(e.target.value);
            emit({ key: e.target.value });
          }}
        />
        <Input
          label="Branch (optional)"
          mono
          disabled={disabled}
          placeholder="the remote's default"
          value={branch}
          error={serverErrors.branch}
          onChange={(e) => {
            setBranch(e.target.value);
            emit({ branch: e.target.value });
          }}
        />
        <Select
          label="Credential"
          disabled={disabled}
          value={credential}
          options={credOptions}
          hint={
            serverErrors.credential ??
            "Match by host uses the saved credential for the URL's host, else this machine's own git setup."
          }
          onChange={(e) => {
            setCredential(e.target.value);
            emit({ credential: e.target.value });
          }}
        />
        <StackInput
          label="Stack (optional)"
          stacks={stacks}
          disabled={disabled}
          value={stack}
          onChange={(e) => {
            setStack(e.target.value);
            emit({ stack: e.target.value });
          }}
          error={serverErrors.stack}
        />
      </div>
      <div className="os-field">
        <span className="os-field__label">Folder (optional)</span>
        <span className="os-field__hint">{`The checkout goes to ${dest}. The folder must not exist yet, or be empty.`}</span>
        <fieldset
          disabled={disabled}
          style={{ border: 0, padding: 0, margin: 0, minWidth: 0, pointerEvents: disabled ? "none" : undefined }}
        >
          <FolderPicker
            value={path}
            onChange={(p) => {
              setPath(p);
              emit({ path: p });
            }}
            list={listFolders}
            mkdir={makeFolder}
            initialPath={root || "~"}
            height={150}
          />
        </fieldset>
        {serverErrors.path && <span className="os-field__error">{serverErrors.path}</span>}
      </div>
    </div>
  );
}
