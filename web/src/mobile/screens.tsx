import { parseResource } from "../lib/resource";
import { BookScreen, DependencyFileScreen, DocsScreen } from "../screens";
import { MArtifact } from "./screens/MArtifact";
import { MExecution } from "./screens/MExecution";
import { MHome } from "./screens/MHome";
import { MProject } from "./screens/MProject";
import { MSession } from "./screens/MSession";
import { MSettings } from "./screens/MSettings";
import { MWorkspace } from "./screens/MWorkspace";

/** Screens that scroll their own panes, so the shell's stack must not scroll. */
export const mobileFills = (id: string) => id.startsWith("dep:") || id.startsWith("book:");

/** The mobile screen for a resource id. Keyed by the id so each screen starts with fresh state. */
export function MobileScreenFor({ ws, id }: { ws: string; id: string }) {
  const r = parseResource(id);
  if (!r) return null;
  switch (r.type) {
    case "ws":
      if (r.page === "overview") return <MHome key={id} ws={ws} />;
      if (r.page === "settings") return <MSettings key={id} ws={ws} />;
      if (r.page === "docs") return <DocsScreen key={id} ws={ws} />;
      return <MWorkspace key={id} ws={ws} page={r.page} />;
    case "session":
      return <MSession key={id} ws={ws} id={r.id} />;
    case "book":
      return <BookScreen key={id} ws={ws} id={r.id} />;
    case "exec":
      return <MExecution key={id} ws={ws} id={r.id} />;
    case "artifact":
      return <MArtifact key={id} ws={ws} path={r.path} />;
    case "project":
      return <MProject key={id} ws={ws} projectKey={r.key} path={null} />;
    case "file":
      return <MProject key={id} ws={ws} projectKey={r.key} path={r.path} />;
    case "dep":
      return <DependencyFileScreen key={id} ws={ws} projectKey={r.key} uri={r.uri} />;
  }
}
