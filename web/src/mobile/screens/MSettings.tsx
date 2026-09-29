import { Banner, Icon, Panel } from "@ostra/design";
import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { api } from "../../api";
import type { WorkspaceDetail } from "../../api/types";
import { useShell, useWorkspace } from "../../lib/nav";
import { GitCredentials } from "../../screens/setup/GitCredentials";
import { ProviderCredentials } from "../../screens/setup/ProviderCredentials";
import { SignInSessions } from "../../screens/setup/SignInSessions";
import { McpSection } from "../../screens/workspace/McpSection";
import { PendingCommandsBanner } from "../../screens/workspace/PendingCommands";
import {
  GeneralSection,
  InstructionsSection,
  NotificationsSection,
  PermissionsSection,
  ProjectsSection,
  RoutingSection,
} from "../../screens/workspace/SettingsSections";
import { SETTINGS_TABS, type SettingsTab } from "../../screens/workspace/settingsForm";
import { useSettingsEditor } from "../../screens/workspace/useSettingsEditor";
import "../../screens/workspace/workspace.css";
import "./MSettings.css";

/** `ws:settings`. */
export function MSettings({ ws }: { ws: string }) {
  const { detail, reload } = useWorkspace();
  if (!detail) return <div className="m-page m-muted">Reading the settings…</div>;
  return <Editor ws={ws} detail={detail} onSaved={reload} />;
}

function Editor({ ws, detail, onSaved }: { ws: string; detail: WorkspaceDetail; onSaved: () => void }) {
  const { addProject } = useShell();
  const e = useSettingsEditor(ws, detail, onSaved);
  const { props, tab } = e;
  const canSave = e.dirty && !e.saving && !e.checking && e.issues.length === 0;
  const strip = useRef<HTMLDivElement>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: keep the selected tab on screen when it changes.
  useEffect(() => {
    strip.current
      ?.querySelector<HTMLElement>('[aria-selected="true"]')
      ?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [tab]);

  return (
    <div className="ms-root">
      <div ref={strip} className="ms-tabs" role="tablist" aria-label="Settings sections">
        {SETTINGS_TABS.map((t) => {
          const n = e.map.byTab[t.id];
          return (
            <button
              type="button"
              key={t.id}
              role="tab"
              aria-selected={tab === t.id}
              className="ms-tab"
              onClick={() => e.setTab(t.id as SettingsTab)}
            >
              {t.label}
              {n ? <span className="ms-tab-count">{n}</span> : null}
            </button>
          );
        })}
      </div>
      <div className="ms-body">
        <div className="ms-savebar">
          <span className="ms-status" role="status" style={{ color: e.status.color }}>
            {e.saved && <Icon name="check" size={14} />}
            {e.status.text}
          </span>
          {e.dirty && (
            <button type="button" className="m-btn m-btn-quiet ms-save" disabled={e.saving} onClick={e.discard}>
              Discard
            </button>
          )}
          <button
            type="button"
            className="m-btn m-btn-primary ms-save"
            disabled={!canSave}
            onClick={() => void e.save()}
          >
            <Icon name="check" size={15} />
            {e.saving ? "Saving…" : "Save"}
          </button>
        </div>
        <PendingCommandsBanner ws={ws} pending={detail.pending_commands} onApproved={onSaved} />
        {e.external && e.dirty && (
          <Banner tone="info">
            The settings file changed since you started editing. Saving writes your version over it.{" "}
            <button type="button" className="ms-link" onClick={e.discard}>
              Load the saved settings
            </button>
          </Banner>
        )}
        {e.saved && <Banner tone="info">The next execution uses these settings; no restart is needed.</Banner>}
        {e.error && <Banner tone="bad">{e.error}</Banner>}
        {e.elsewhere.length > 0 && (
          <Banner tone="bad" title="Problems to fix before saving">
            <ul className="wp-issues" style={{ paddingLeft: 16 }}>
              {e.elsewhere.map((i, n) => {
                const fix = e.fixFor(i);
                return (
                  <li key={n}>
                    <button type="button" className="ms-link" onClick={() => e.go(i.path)}>
                      <code>{i.path}</code>
                    </button>
                    : {i.message}
                    {fix && (
                      <>
                        {" "}
                        <button type="button" className="ms-link" onClick={() => e.applyFix(fix)}>
                          {fix.label}
                        </button>
                      </>
                    )}
                  </li>
                );
              })}
            </ul>
          </Banner>
        )}
        {tab === "general" && (
          <>
            <Appearance />
            <GeneralSection {...props} />
            <section className="m-section">
              <span className="m-label">Folder</span>
              <span className="ms-mono">{detail.root}</span>
            </section>
            <DeleteWorkspace ws={ws} root={detail.root} />
          </>
        )}
        {tab === "projects" && (
          <div className="ms-projects ms-stack">
            <ProjectsSection {...props} onAdd={addProject} stacks={detail.stacks} />
          </div>
        )}
        {tab === "git" && <GitCredentials />}
        {tab === "routing" && (
          <div className="ms-routing ms-stack">
            <RoutingSection {...props} harnesses={detail.harnesses} agentInfo={detail.agents} />
            <Panel
              title="Native providers"
              subtitle="saved for every workspace on this machine; environment variables take precedence"
            >
              <ProviderCredentials providers={detail.providers} onSaved={onSaved} />
            </Panel>
          </div>
        )}
        {tab === "mcp" && (
          <div className="ms-mcp ms-stack">
            <McpSection {...props} ws={ws} saved={e.base.mcp_servers} agents={detail.agents} />
          </div>
        )}
        {tab === "permissions" && (
          <PermissionsSection
            {...props}
            global={detail.global_permissions}
            sandbox={detail.sandbox}
            savedSandbox={detail.settings.sandbox_mode}
            globalSandbox={detail.global_sandbox}
            globalEnforcement={detail.global_tool_enforcement}
          />
        )}
        {tab === "instructions" && (
          <div className="ms-stacked ms-stack">
            <InstructionsSection {...props} ws={ws} projects={detail.projects.map((p) => p.key)} />
          </div>
        )}
        {tab === "notifications" && <NotificationsSection {...props} />}
        {tab === "signin" && <SignInSessions />}
      </div>
    </div>
  );
}

/** Theme for this browser: a device choice, so it applies at once and never goes through Save. */
function Appearance() {
  const { theme, toggleTheme } = useShell();
  const options = [
    { id: "dark", label: "Dark", icon: "moon" },
    { id: "light", label: "Light", icon: "sun" },
  ] as const;
  return (
    <section className="m-section">
      <span className="m-label">Appearance</span>
      <div className="ms-segment" role="radiogroup" aria-label="Theme">
        {options.map((o) => (
          <button
            type="button"
            key={o.id}
            role="radio"
            aria-checked={theme === o.id}
            className="ms-segment-item"
            onClick={() => theme !== o.id && toggleTheme()}
          >
            <Icon name={o.icon} size={14} />
            {o.label}
          </button>
        ))}
      </div>
    </section>
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
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  };
  return (
    <section className="m-section ms-danger">
      <span className="m-label">Delete workspace</span>
      <span className="ms-text">
        Deleting removes the workspace from the list and deletes <code>.ostra/workspace.toml</code> and{" "}
        <code>.ostra/workspace.db</code> in <code>{root}</code>, so its settings and session history are gone. Project
        folders, session folders under <code>.ostra/sessions</code>, and each project's <code>.ostra</code> files stay
        on disk.
      </span>
      {error && <span className="ms-error">{error}</span>}
      {confirming ? (
        <div className="ms-row">
          <button type="button" className="m-btn m-btn-quiet" disabled={busy} onClick={() => setConfirming(false)}>
            Cancel
          </button>
          <button
            type="button"
            className="m-btn ms-btn-danger"
            style={{ flex: 1 }}
            disabled={busy}
            onClick={() => void remove()}
          >
            <Icon name="trash-2" size={15} />
            {busy ? "Deleting…" : "Delete this workspace"}
          </button>
        </div>
      ) : (
        <button type="button" className="m-btn ms-btn-danger-outline" onClick={() => setConfirming(true)}>
          <Icon name="trash-2" size={15} />
          Delete workspace…
        </button>
      )}
    </section>
  );
}
