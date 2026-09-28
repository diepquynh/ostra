import { Banner, Button, Panel, Tabs } from "@ostra/design";
import { useState } from "react";
import { useNavigate } from "react-router";
import { api } from "../api";
import type { WorkspaceDetail } from "../api/types";
import { useShell, useWorkspace } from "../lib/nav";
import { GitCredentials } from "./setup/GitCredentials";
import { ProviderCredentials } from "./setup/ProviderCredentials";
import { SignInSessions } from "./setup/SignInSessions";
import { McpSection } from "./workspace/McpSection";
import { Loading, Page } from "./workspace/Page";
import { PendingCommandsBanner } from "./workspace/PendingCommands";
import {
  GeneralSection,
  InstructionsSection,
  NotificationsSection,
  PermissionsSection,
  ProjectsSection,
  RoutingSection,
} from "./workspace/SettingsSections";
import { SETTINGS_TABS, type SettingsTab } from "./workspace/settingsForm";
import { useSettingsEditor } from "./workspace/useSettingsEditor";

export type SettingsScreenProps = { ws: string };

/**
 * Resource `ws:settings`: workspace settings as forms, validated as you edit and again on save. A
 * `#setting:<dotted key>` anchor (from ⌘K) opens that field's tab and rings the field.
 */
export function SettingsScreen({ ws }: SettingsScreenProps) {
  const { detail, reload } = useWorkspace();
  if (!detail) {
    return (
      <Page title="Settings">
        <Loading>Reading the settings…</Loading>
      </Page>
    );
  }
  return <SettingsEditor ws={ws} detail={detail} onSaved={reload} />;
}

function SettingsEditor({ ws, detail, onSaved }: { ws: string; detail: WorkspaceDetail; onSaved: () => void }) {
  const { addProject } = useShell();
  const {
    base,
    tab,
    setTab,
    map,
    dirty,
    checking,
    saving,
    saved,
    error,
    external,
    elsewhere,
    status,
    props,
    go,
    discard,
    save,
    issues,
    fixFor,
    applyFix,
  } = useSettingsEditor(ws, detail, onSaved);
  return (
    <Page
      title="Settings"
      actions={
        <div className="wp-row" style={{ alignSelf: "center" }}>
          <span style={{ fontSize: "var(--text-sm)", color: status.color }} role="status">
            {status.text}
          </span>
          <Button size="sm" variant="ghost" disabled={!dirty || saving} onClick={discard}>
            Discard
          </Button>
          <Button
            variant="primary"
            size="sm"
            icon="check"
            disabled={!dirty || saving || checking || issues.length > 0}
            onClick={() => void save()}
          >
            {saving ? "Saving…" : "Save"}
          </Button>
        </div>
      }
    >
      <PendingCommandsBanner ws={ws} pending={detail.pending_commands} onApproved={onSaved} />
      {external && dirty && (
        <Banner
          tone="info"
          actions={
            <Button size="sm" onClick={discard}>
              Load the saved settings
            </Button>
          }
        >
          The settings file changed since you started editing. Saving writes your version over it.
        </Banner>
      )}
      {saved && <Banner tone="info">The next execution uses these settings; no restart is needed.</Banner>}
      {error && <Banner tone="bad">{error}</Banner>}
      {elsewhere.length > 0 && (
        <Banner tone="bad" title="Problems to fix before saving">
          <ul className="wp-issues" style={{ paddingLeft: 16 }}>
            {elsewhere.map((i, n) => {
              const fix = fixFor(i);
              return (
                <li key={n}>
                  <button type="button" onClick={() => go(i.path)} style={linkButton} title="Show this field">
                    <code>{i.path}</code>
                  </button>
                  : {i.message}
                  {fix && (
                    <>
                      {" "}
                      <Button size="sm" icon="wrench" onClick={() => applyFix(fix)}>
                        {fix.label}
                      </Button>
                    </>
                  )}
                </li>
              );
            })}
          </ul>
        </Banner>
      )}
      <Tabs
        label="Settings sections"
        value={tab}
        onChange={(id) => setTab(id as SettingsTab)}
        tabs={SETTINGS_TABS.map((t) => ({ ...t, count: map.byTab[t.id] || undefined }))}
      />
      {tab === "general" && (
        <>
          <GeneralSection {...props} />
          <DeleteWorkspace ws={ws} root={detail.root} />
        </>
      )}
      {tab === "projects" && <ProjectsSection {...props} onAdd={addProject} stacks={detail.stacks} />}
      {tab === "git" && <GitCredentials />}
      {tab === "routing" && (
        <>
          <RoutingSection {...props} harnesses={detail.harnesses} agentInfo={detail.agents} />
          <Panel
            title="Native providers"
            subtitle="saved for every workspace on this machine; environment variables take precedence"
          >
            <ProviderCredentials providers={detail.providers} onSaved={onSaved} />
          </Panel>
        </>
      )}
      {tab === "mcp" && <McpSection {...props} ws={ws} saved={base.mcp_servers} agents={detail.agents} />}
      {tab === "permissions" && (
        <PermissionsSection
          {...props}
          global={detail.global_permissions}
          sandbox={detail.sandbox}
          savedSandbox={detail.settings.sandbox_mode}
          globalSandbox={detail.global_sandbox}
        />
      )}
      {tab === "instructions" && (
        <InstructionsSection {...props} ws={ws} projects={detail.projects.map((p) => p.key)} />
      )}
      {tab === "notifications" && <NotificationsSection {...props} />}
      {tab === "signin" && <SignInSessions />}
    </Page>
  );
}

function DeleteWorkspace({ ws, root }: { ws: string; root: string }) {
  const navigate = useNavigate();
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const remove = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.deleteWorkspace(ws);
      navigate("/");
    } catch (e) {
      setError((e as Error).message);
      setBusy(false);
    }
  };
  return (
    <Panel title="Delete workspace" tone="bad" subtitle="removes this workspace from Ostra">
      <p style={{ margin: 0, color: "var(--text-secondary)", lineHeight: 1.55 }}>
        Deleting removes the workspace from the list and deletes <code>.ostra/workspace.toml</code> and{" "}
        <code>.ostra/workspace.db</code> in <code>{root}</code>, so its settings and session history are gone. Project
        folders, session folders under <code>.ostra/sessions</code>, and each project's <code>.ostra</code> files stay
        on disk.
      </p>
      {error && <Banner tone="bad">{error}</Banner>}
      <div className="wp-row" style={{ marginTop: 12 }}>
        {confirming ? (
          <>
            <Button variant="danger" size="sm" icon="trash-2" disabled={busy} onClick={() => void remove()}>
              {busy ? "Deleting…" : "Delete this workspace"}
            </Button>
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => setConfirming(false)}>
              Cancel
            </Button>
          </>
        ) : (
          <Button variant="danger" size="sm" icon="trash-2" onClick={() => setConfirming(true)}>
            Delete workspace…
          </Button>
        )}
      </div>
    </Panel>
  );
}

const linkButton = {
  background: "none",
  border: 0,
  padding: 0,
  color: "inherit",
  font: "inherit",
  cursor: "pointer",
  textDecoration: "underline",
} as const;
