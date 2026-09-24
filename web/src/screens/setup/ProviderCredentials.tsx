import { useState } from "react";
import { api, HttpError } from "../../api";
import type { ProviderCredentialsEdit, ProviderStatus, ValidationIssue } from "../../api/types";
import { Banner, Button, Input, Panel, Select } from "../../design";
import { keySource, PROVIDER_LABEL } from "./wizard";

type KeyKind = "api_key" | "auth_token";

const KEY_KINDS = [
  { value: "api_key", label: "API key" },
  { value: "auth_token", label: "Auth token (Bearer)" },
];

const urlSource = (p: ProviderStatus) => (p.base_url_source === "default" ? "provider default" : p.base_url_source.replace(/^env:/, "env "));

/**
 * Base URL and key per native provider, saved on the server. Environment variables still win, so a saved
 * value that is shadowed says so. Secrets are write-only: the server reports only whether one is saved.
 */
export function ProviderCredentials({ providers, onSaved }: { providers: ProviderStatus[]; onSaved: (p: ProviderStatus) => void }) {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
      {providers
        .filter((p) => p.name in PROVIDER_LABEL)
        .map((p) => (
          <ProviderForm key={p.name} provider={p} onSaved={onSaved} />
        ))}
    </div>
  );
}

function ProviderForm({ provider: p, onSaved }: { provider: ProviderStatus; onSaved: (p: ProviderStatus) => void }) {
  const [baseUrl, setBaseUrl] = useState(p.saved.base_url ?? "");
  const [kind, setKind] = useState<KeyKind>(p.saved.has_auth_token && !p.saved.has_api_key ? "auth_token" : "api_key");
  const [secret, setSecret] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [issues, setIssues] = useState<ValidationIssue[]>([]);
  const issue = (path: string) => issues.filter((i) => i.path === path).map((i) => i.message).join(" ") || null;
  const savedSecret = p.saved.has_api_key || p.saved.has_auth_token;
  const shadowedKey = savedSecret && p.source.startsWith("env:");
  const shadowedUrl = p.saved.base_url && p.base_url_source !== "saved";

  const send = async (edit: ProviderCredentialsEdit) => {
    setBusy(true);
    setError(null);
    setIssues([]);
    try {
      const next = await api.saveProvider(p.name, edit);
      setSecret("");
      setBaseUrl(next.saved.base_url ?? "");
      onSaved(next);
    } catch (e) {
      if (e instanceof HttpError) setIssues(e.issues);
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const save = () => {
    const edit: ProviderCredentialsEdit = { base_url: baseUrl };
    if (secret.trim()) {
      edit[kind] = secret;
      edit[kind === "api_key" ? "auth_token" : "api_key"] = "";
    }
    void send(edit);
  };

  const label = PROVIDER_LABEL[p.name] ?? p.name;
  return (
    <Panel
      title={`${label} endpoint and key`}
      subtitle={p.has_key ? `key from ${keySource(p.source)}, URL from ${urlSource(p)}` : "no key"}
      actions={
        savedSecret || p.saved.base_url ? (
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => void send({ base_url: "", api_key: "", auth_token: "" })}>
            Clear saved
          </Button>
        ) : undefined
      }
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
        {shadowedKey && <Banner tone="info">{`${p.source.replace(/^env:/, "")} is set in the environment, so it is used instead of the saved key.`}</Banner>}
        {shadowedUrl && <Banner tone="info">{`The base URL comes from ${urlSource(p)}, which takes precedence over the saved URL.`}</Banner>}
        <Input
          size="sm"
          mono
          label="Base URL"
          aria-label={`Base URL for ${label}`}
          placeholder={p.name === "anthropic" ? "https://api.anthropic.com" : "https://api.openai.com"}
          hint="Leave empty for the provider's own endpoint."
          value={baseUrl}
          error={issue("base_url")}
          onChange={(e) => setBaseUrl(e.target.value)}
        />
        <div style={{ display: "flex", gap: 8, alignItems: "flex-start", flexWrap: "wrap" }}>
          <Select size="sm" aria-label={`Key type for ${label}`} value={kind} onChange={(e) => setKind(e.target.value as KeyKind)} options={KEY_KINDS} />
          <Input
            size="sm"
            mono
            type="password"
            autoComplete="off"
            aria-label={`${kind === "api_key" ? "API key" : "Auth token"} for ${label}`}
            placeholder={savedSecret ? "Saved. Type a new value to replace it." : "Paste the key"}
            value={secret}
            error={issue(kind)}
            onChange={(e) => setSecret(e.target.value)}
            style={{ flex: 1, minWidth: 220 }}
          />
          <Button size="sm" variant="primary" disabled={busy} onClick={save}>
            {busy ? "Applying…" : "Apply"}
          </Button>
        </div>
        {error && issues.length === 0 && <Banner tone="bad">{error}</Banner>}
      </div>
    </Panel>
  );
}
