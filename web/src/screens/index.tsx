import { parseResource } from "../lib/resource";
import { ArtifactScreen } from "./ArtifactScreen";
import { CostScreen } from "./CostScreen";
import { DependencyFileScreen } from "./DependencyFileScreen";
import { BookScreen, DocsScreen } from "./DocsScreen";
import { ExecutionScreen } from "./ExecutionScreen";
import { FileScreen } from "./FileScreen";
import { MemoryScreen } from "./MemoryScreen";
import { ProjectScreen } from "./ProjectScreen";
import { SessionScreen } from "./SessionScreen";
import { SettingsScreen } from "./SettingsScreen";
import { SkillsScreen } from "./SkillsScreen";
import { WorkspaceScreen } from "./WorkspaceScreen";

export { AddProjectDialog } from "./setup/AddProjectDialog";
export { NewWorkspaceDialog } from "./setup/NewWorkspaceDialog";
export { Onboarding } from "./setup/Onboarding";
export {
  ArtifactScreen,
  BookScreen,
  CostScreen,
  DependencyFileScreen,
  DocsScreen,
  ExecutionScreen,
  FileScreen,
  MemoryScreen,
  ProjectScreen,
  SessionScreen,
  SettingsScreen,
  SkillsScreen,
  WorkspaceScreen,
};

/** Resources whose screen scrolls its own content, so the shell's center pane must not scroll. */
export const selfScrolling = (id: string) => /^(artifact|file|dep|project|book):/.test(id);

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
      if (r.page === "docs") return <DocsScreen key={id} ws={ws} />;
      return <WorkspaceScreen key={id} ws={ws} />;
    case "session":
      return <SessionScreen key={id} ws={ws} id={r.id} />;
    case "book":
      return <BookScreen key={id} ws={ws} id={r.id} />;
    case "exec":
      return <ExecutionScreen key={id} ws={ws} id={r.id} />;
    case "artifact":
      return <ArtifactScreen key={id} ws={ws} path={r.path} />;
    case "project":
      return <ProjectScreen key={id} ws={ws} projectKey={r.key} />;
    case "file":
      return <FileScreen key={id} ws={ws} projectKey={r.key} path={r.path} />;
    case "dep":
      return <DependencyFileScreen key={id} ws={ws} projectKey={r.key} uri={r.uri} />;
  }
}
