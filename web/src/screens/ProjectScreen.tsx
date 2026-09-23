import { useState } from "react";
import type { ProjectView } from "../api/types";
import { Button, Chip, Icon, Spinner, StatusChip, Tabs } from "../design";
import { useShell, useWorkspace } from "../lib/nav";
import { ProjectFiles } from "./project/ProjectFiles";
import { ProjectOverview } from "./project/ProjectOverview";

export type ProjectScreenProps = {
  ws: string;
  /** Project key within the workspace. */
  projectKey: string;
};

/** "rust · axum" from the initializer's profile, else the stack chosen at import. */
export function stackLabel(p: ProjectView): string | null {
  const s = p.profile?.stack;
  if (s?.language) return [s.language, ...s.frameworks].join(" · ");
  return p.stack;
}

/**
 * Resource `project:<key>`: the project header, then an Overview tab (commands, skills, maintenance, or the
 * Initialize flow) and a Files tab (a read-only browser). Scrolls its own content.
 */
export function ProjectScreen({ ws, projectKey }: ProjectScreenProps) {
  const { detail } = useWorkspace();
  const shell = useShell();
  const [tab, setTab] = useState<"overview" | "files">("overview");
  const p = detail?.projects.find((x) => x.key === projectKey);

  if (!detail)
    return (
      <div style={{ padding: 24, display: "flex", gap: 8, alignItems: "center", color: "var(--text-muted)" }}>
        <Spinner size={11} /> Reading the projects…
      </div>
    );
  if (!p)
    return (
      <div style={{ padding: 24, display: "flex", flexDirection: "column", gap: 12, alignItems: "flex-start", color: "var(--text-secondary)" }}>
        <span>
          No project <code>{projectKey}</code> in this workspace. It may have been removed.
        </span>
        <Button icon="folder-plus" onClick={shell.addProject}>
          Add project
        </Button>
      </div>
    );

  const stack = stackLabel(p);
  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", minHeight: 0 }}>
      <div style={{ padding: "20px 24px 0", display: "flex", alignItems: "flex-start", gap: 12, flexWrap: "wrap", flex: "none" }}>
        <div style={{ flex: "1 1 360px", minWidth: 0 }}>
          <h1 style={{ margin: "0 0 6px", font: "var(--type-title)", display: "flex", alignItems: "center", gap: 10 }}>
            <Icon name="folder-git-2" size={20} style={{ color: "var(--text-muted)" }} />
            {p.key}
          </h1>
          <div style={{ display: "flex", flexWrap: "wrap", gap: 6, alignItems: "center" }}>
            <StatusChip kind="init" status={p.init_status} />
            {p.is_git ? (
              <Chip mono icon="git-branch" title={p.git_branch ? "The checked-out branch, from .git/HEAD" : "A git checkout"}>
                {p.git_branch ?? "git"}
              </Chip>
            ) : (
              <Chip tone="warn">not a git checkout</Chip>
            )}
            <span style={{ font: "var(--text-sm)/1 var(--font-mono)", color: "var(--text-muted)", wordBreak: "break-all" }}>
              {p.path}
              {stack && ` · ${stack}`}
            </span>
          </div>
        </div>
        <Button size="sm" icon="folder-tree" onClick={() => shell.browseFiles(p.key)}>
          Browse files
        </Button>
      </div>
      <div style={{ padding: "12px 24px 0", flex: "none" }}>
        <Tabs
          label="Project"
          value={tab}
          onChange={(t) => setTab(t as "overview" | "files")}
          tabs={[
            { id: "overview", label: p.init_status === "not_initialized" ? "Initialize" : "Overview", icon: p.init_status === "not_initialized" ? "sparkles" : "info" },
            { id: "files", label: "Files", icon: "folder-tree" },
          ]}
        />
      </div>
      {tab === "files" ? (
        <ProjectFiles key={p.key} ws={ws} project={p} />
      ) : (
        <div style={{ flex: 1, overflow: "auto", minHeight: 0 }}>
          <ProjectOverview ws={ws} project={p} />
        </div>
      )}
    </div>
  );
}
