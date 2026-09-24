import { parseResource } from "../lib/resource";
import { ArtifactScreen } from "./ArtifactScreen";
import { CostScreen } from "./CostScreen";
import { ExecutionScreen } from "./ExecutionScreen";
import { FileScreen } from "./FileScreen";
import { MemoryScreen } from "./MemoryScreen";
import { ProjectScreen } from "./ProjectScreen";
import { SessionScreen } from "./SessionScreen";
import { SettingsScreen } from "./SettingsScreen";
import { SkillsScreen } from "./SkillsScreen";
import { WorkspaceScreen } from "./WorkspaceScreen";

export { ArtifactScreen, CostScreen, ExecutionScreen, FileScreen, MemoryScreen, ProjectScreen, SessionScreen, SettingsScreen, SkillsScreen, WorkspaceScreen };
export { Onboarding } from "./setup/Onboarding";
export { NewWorkspaceDialog } from "./setup/NewWorkspaceDialog";
export { AddProjectDialog } from "./setup/AddProjectDialog";

/** Resources whose screen scrolls its own content, so the shell's center pane must not scroll. */
export const selfScrolling = (id: string) => /^(artifact|file|project):/.test(id);

/** The screen for a resource id. Keyed by the id so each tab starts with fresh state. */
export function ScreenFor({ ws, id }: { ws: string; id: string }) {
  const r = parseResource(id);
  if (!r) return null;
  switch (r.type) {
    case "ws":
      if (r.page === "cost") return <CostScreen key={id} ws={ws} />;
      if (r.page === "settings") return <SettingsScreen key={id} ws={ws} />;
      if (r.page === "memory") return <MemoryScreen key={id} ws={ws} />;
      if (r.page === "skills") return <SkillsScreen key={id} ws={ws} />;
      return <WorkspaceScreen key={id} ws={ws} />;
    case "session":
      return <SessionScreen key={id} ws={ws} id={r.id} />;
    case "exec":
      return <ExecutionScreen key={id} ws={ws} id={r.id} />;
    case "artifact":
      return <ArtifactScreen key={id} ws={ws} path={r.path} />;
    case "project":
      return <ProjectScreen key={id} ws={ws} projectKey={r.key} />;
    case "file":
      return <FileScreen key={id} ws={ws} projectKey={r.key} path={r.path} />;
  }
}
