import { useState } from "react";
import { api } from "../api";
import type { GitPullResult, ProjectView } from "../api/types";
import { Banner, Button, Chip, Icon, Spinner, StatusChip, Tabs } from "../design";
import { useShell, useWorkspace } from "../lib/nav";
import { DependencyGraph } from "./project/DependencyGraph";
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
 * Resource `project:<key>`: the project header, then the overview (commands, skills, maintenance, or the
 * Initialize flow). "Browse files" opens the Files view. Scrolls its own content.
 */
export function ProjectScreen({ ws, projectKey }: ProjectScreenProps) {
  const { detail } = useWorkspace();
  const shell = useShell();
  const p = detail?.projects.find((x) => x.key === projectKey);
  const [pulling, setPulling] = useState(false);
  const [pulled, setPulled] = useState<{ ok: GitPullResult } | { error: string } | null>(null);
  const [section, setSection] = useState<"overview" | "dependencies">("overview");

  const pull = () => {
    setPulling(true);
    setPulled(null);
    api.pullProject(ws, projectKey).then(
      (ok) => setPulled({ ok }),
      (e: Error) => setPulled({ error: e.message }),
    ).finally(() => setPulling(false));
  };

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
        {p.is_git && (
          <Button size="sm" icon="git-pull-request" disabled={pulling} title="Fast-forward the checked-out branch from its upstream" onClick={pull}>
            {pulling ? "Pulling…" : "Pull"}
          </Button>
        )}
        <Button size="sm" icon="folder-tree" onClick={() => shell.browseFiles(p.key)}>
          Browse files
        </Button>
      </div>
      {pulled && (
        <div style={{ padding: "12px 24px 0", flex: "none" }}>
          {"error" in pulled ? (
            <Banner tone="bad">{pulled.error}</Banner>
          ) : (
            <Banner tone="info">
              {pulled.ok.updated
                ? `Pulled ${pulled.ok.branch ?? "the branch"} from ${pulled.ok.before ?? "?"} to ${pulled.ok.after ?? "?"}.`
                : `${pulled.ok.branch ?? "The branch"} is already up to date.`}
            </Banner>
          )}
        </div>
      )}
      <div style={{ padding: "12px 24px 0", flex: "none" }}>
        <Tabs
          label="Project sections"
          tabs={[
            { id: "overview", label: "Overview", icon: "layout-dashboard" },
            { id: "dependencies", label: "Dependencies", icon: "git-fork" },
          ]}
          value={section}
          onChange={(id) => setSection(id as "overview" | "dependencies")}
        />
      </div>
      <div style={{ flex: 1, overflow: "auto", minHeight: 0, paddingTop: 12 }}>
        {section === "overview" ? <ProjectOverview ws={ws} project={p} /> : <DependencyGraph key={p.key} ws={ws} projectKey={p.key} />}
      </div>
    </div>
  );
}
