import { useEffect, useRef, useState } from "react";
import { api, HttpError } from "../../api";
import type { GitCredentialEdit, GitCredentialKind, GitCredentialView, ValidationIssue } from "../../api/types";
import { Banner, Button, Input, Panel, Select, Table } from "../../design";

/** A private key file is a few kilobytes; anything much larger is the wrong file. */
const MAX_KEY_BYTES = 64 * 1024;

const KINDS = [
  { value: "https", label: "HTTPS token" },
  { value: "ssh", label: "SSH private key" },
];

/** The saved git credentials, loaded once per mount. */
export function useGitCredentials() {
  const [list, setList] = useState<GitCredentialView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api.gitCredentials().then(setList, (e: Error) => setError(e.message));
  }, []);
  return { list, setList, error };
}

/**
 * Git credentials for cloning and pulling, saved on the server for every workspace on this machine. Secrets are
 * write-only: the server reports only that one is saved. Git gets them per command, so they never reach a
 * project's .git/config or an agent's environment.
 */
export function GitCredentials() {
  const { list, setList, error: loadError } = useGitCredentials();
  const [kind, setKind] = useState<GitCredentialKind>("https");
  const [label, setLabel] = useState("");
  const [host, setHost] = useState("");
  const [username, setUsername] = useState("");
  const [secret, setSecret] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [issues, setIssues] = useState<ValidationIssue[]>([]);
  const keyFile = useRef<HTMLInputElement>(null);
  const importKey = async (file: File | undefined) => {
    if (!file) return;
    setError(null);
    setIssues([]);
    if (file.size > MAX_KEY_BYTES) {
      setError(`${file.name} is too large for a private key. Choose the key file itself, for example ~/.ssh/id_ed25519.`);
      return;
    }
    setSecret(await file.text());
  };
  const issue = (path: string) => issues.filter((i) => i.path === path).map((i) => i.message).join(" ") || null;

  const reset = () => {
    setEditing(null);
    setLabel("");
    setHost("");
    setUsername("");
    setSecret("");
    setKind("https");
    setIssues([]);
    setError(null);
  };

  const run = async (call: Promise<GitCredentialView[]>, after?: () => void) => {
    setBusy(true);
    setError(null);
    setIssues([]);
    try {
      setList(await call);
      after?.();
    } catch (e) {
      if (e instanceof HttpError) setIssues(e.issues);
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const save = () => {
    const edit: GitCredentialEdit = { label, host, kind, username };
    if (secret.trim()) edit.secret = secret;
    void run(editing ? api.updateGitCredential(editing, edit) : api.createGitCredential(edit), reset);
  };

  const edit = (c: GitCredentialView) => {
    setEditing(c.id);
    setLabel(c.label);
    setHost(c.host);
    setKind(c.kind);
    setUsername(c.username ?? "");
    setSecret("");
    setIssues([]);
    setError(null);
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
      <p className="wp-lead" style={{ margin: 0 }}>
        Credentials stay on the machine that runs Ostra, in its data folder (under Docker, the data volume). The browser never
        receives a saved token or key, and Ostra gives one to git only for a clone or pull that uses it.
      </p>
      <Panel title="Saved git credentials" subtitle="for every workspace on this machine; the longest matching host and path wins" bodyFlush>
        {loadError && <Banner tone="bad">{loadError}</Banner>}
        <Table<GitCredentialView>
          dense
          rows={list ?? []}
          empty={list ? "No git credentials yet. Without one, git uses this machine's own SSH keys and credential helpers." : "Loading…"}
          columns={[
            { key: "label", label: "Name", render: (c) => c.label },
            { key: "host", label: "Host", render: (c) => <span className="wp-mono">{c.host}</span> },
            { key: "kind", label: "Type", width: 130, render: (c) => (c.kind === "ssh" ? "SSH key" : `Token${c.username ? ` as ${c.username}` : ""}`) },
            {
              key: "actions",
              label: "",
              width: 150,
              render: (c) => (
                <div style={{ display: "flex", gap: 4 }}>
                  <Button size="sm" variant="ghost" onClick={() => edit(c)}>
                    Edit
                  </Button>
                  <Button size="sm" variant="ghost" icon="trash-2" disabled={busy} title={`Delete ${c.label}`} onClick={() => void run(api.deleteGitCredential(c.id), () => editing === c.id && reset())}>
                    Delete
                  </Button>
                </div>
              ),
            },
          ]}
        />
      </Panel>
      <Panel
        title={editing ? "Edit git credential" : "Add a git credential"}
        actions={
          editing ? (
            <Button size="sm" variant="ghost" onClick={reset}>
              Cancel
            </Button>
          ) : undefined
        }
      >
        <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
          <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(200px, 1fr))", gap: 10 }}>
            <Select size="sm" label="Type" value={kind} onChange={(e) => setKind(e.target.value as GitCredentialKind)} options={KINDS} />
            <Input size="sm" mono label="Host" placeholder="github.com or github.com/acme" value={host} error={issue("host")} onChange={(e) => setHost(e.target.value)} />
            <Input size="sm" label="Name (optional)" placeholder="Work GitHub" value={label} onChange={(e) => setLabel(e.target.value)} />
            {kind === "https" && (
              <Input
                size="sm"
                mono
                label="User name (optional)"
                placeholder="x-access-token"
                hint="GitHub and GitLab accept any name with a token. Bitbucket needs your user name."
                value={username}
                error={issue("username")}
                onChange={(e) => setUsername(e.target.value)}
              />
            )}
          </div>
          <Input
            size={kind === "ssh" ? "md" : "sm"}
            mono
            multiline={kind === "ssh"}
            rows={4}
            type={kind === "ssh" ? undefined : "password"}
            autoComplete="off"
            label={kind === "ssh" ? "Private key" : "Token"}
            placeholder={editing ? "Saved. Paste a new value to replace it." : kind === "ssh" ? "-----BEGIN OPENSSH PRIVATE KEY-----" : "Paste a personal access token"}
            hint={
              kind === "ssh"
                ? "Paste the key or import its file, for example ~/.ssh/id_ed25519. The file is read in this browser and saved on the Ostra machine. Use a deploy key without a passphrase, with only the access the projects need, because agents run in the same environment."
                : "Give the token only the access the projects need, for example read-only contents, because agents run in the same environment."
            }
            value={secret}
            error={issue("secret") ?? issue("kind")}
            onChange={(e) => setSecret(e.target.value)}
          />
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            {kind === "ssh" && (
              <>
                <input
                  ref={keyFile}
                  type="file"
                  hidden
                  aria-label="Private key file"
                  onChange={(e) => {
                    void importKey(e.target.files?.[0]);
                    e.target.value = "";
                  }}
                />
                <Button size="sm" disabled={busy} onClick={() => keyFile.current?.click()}>
                  Import key file…
                </Button>
              </>
            )}
            <Button size="sm" variant="primary" icon="check" disabled={busy} onClick={save}>
              {busy ? "Saving…" : editing ? "Save changes" : "Add credential"}
            </Button>
          </div>
          {error && issues.length === 0 && <Banner tone="bad">{error}</Banner>}
        </div>
      </Panel>
    </div>
  );
}
