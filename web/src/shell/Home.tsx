import { useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { api } from "../api";
import { Button, Chip, Icon, Spinner } from "../design";
import { useAsync } from "../lib/hooks";
import { useWorkspaces } from "../lib/live";
import { applyTheme, lastTheme, resolveTheme } from "../lib/theme";
import { NewWorkspaceDialog, Onboarding } from "../screens";
import { LiveMark, REST } from "./LiveMark";
import { workspaceEntry } from "./TitleBar";
import "./shell.css";

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** `/`: the first-run setup on a fresh machine, else the workspace list. */
export function Home() {
  const onboarding = useAsync(() => api.onboarding(), []);
  const { workspaces, loading, error, reload } = useWorkspaces();
  const [creating, setCreating] = useState(false);
  const [skipped, setSkipped] = useState(false);
  const navigate = useNavigate();
  useEffect(() => applyTheme(resolveTheme(lastTheme()), false), []);

  const ob = onboarding.data;
  if (ob && !ob.onboarded_at && ob.workspaces === 0 && !skipped)
    return (
      <Onboarding
        onFinish={(created) => {
          void api.completeOnboarding().catch(() => {});
          if (created) navigate(`/w/${encodeURIComponent(created)}`);
          else {
            setSkipped(true);
            reload();
          }
        }}
      />
    );

  return (
    <div style={{ height: "100%", overflow: "auto", background: "var(--surface-editor)" }}>
      <div
        style={{
          maxWidth: 760,
          margin: "0 auto",
          padding: "48px 24px",
          display: "flex",
          flexDirection: "column",
          gap: 20,
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 9,
            font: "600 15px/1 var(--font-sans)",
            letterSpacing: "0.02em",
          }}
        >
          <LiveMark state={REST} size={20} />
          Ostra
        </div>
        <div style={{ display: "flex", alignItems: "flex-end", gap: 16, flexWrap: "wrap" }}>
          <div style={{ flex: "1 1 360px" }}>
            <h1 style={{ margin: "0 0 6px", font: "var(--type-title)" }}>Workspaces</h1>
            <p style={{ margin: 0, color: "var(--text-secondary)", lineHeight: 1.55 }}>
              A workspace holds your projects and their settings: which executor and model each agent runs on,
              permissions, memory, and custom instructions.
            </p>
          </div>
          <Button variant="primary" icon="plus" onClick={() => setCreating(true)}>
            New workspace
          </Button>
        </div>
        {error && <div style={{ color: "var(--bad)" }}>{error.message}</div>}
        {loading && workspaces.length === 0 && (
          <div style={{ display: "flex", gap: 8, alignItems: "center", color: "var(--text-muted)" }}>
            <Spinner size={11} /> Reading the workspaces…
          </div>
        )}
        {!loading && !error && workspaces.length === 0 && (
          <div className="os-panel" style={{ padding: 16, color: "var(--text-secondary)" }}>
            No workspaces yet. Create one, then add the project folders you want Ostra to work on.
          </div>
        )}
        <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          {workspaces.map((w) => (
            <button
              key={w.id}
              className="os-panel"
              disabled={!w.available}
              onClick={() => navigate(workspaceEntry(w.id))}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 12,
                padding: "12px 14px",
                textAlign: "left",
                font: "inherit",
                color: "inherit",
                cursor: w.available ? "pointer" : "default",
              }}
            >
              <Icon name={w.available ? "box" : "folder-x"} size={16} style={{ color: "var(--text-muted)" }} />
              <span style={{ flex: 1, minWidth: 0 }}>
                <span style={{ display: "block", fontWeight: 600 }}>{w.name}</span>
                <span
                  style={{
                    display: "block",
                    font: "var(--text-sm)/1.4 var(--font-mono)",
                    color: "var(--text-muted)",
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                >
                  {w.root}
                </span>
              </span>
              {!w.available && <Chip tone="bad">Folder missing</Chip>}
              <Chip>{plural(w.projects, "project")}</Chip>
              {w.active_sessions > 0 && <Chip tone="accent">{plural(w.active_sessions, "active session")}</Chip>}
            </button>
          ))}
        </div>
      </div>
      {creating && <NewWorkspaceDialog onClose={() => setCreating(false)} />}
    </div>
  );
}
