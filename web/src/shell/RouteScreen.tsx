import { Navigate, useLocation, useParams } from "react-router";
import { resourceFromPath } from "../lib/resource";
import { ScreenFor } from "../screens";

/** The center pane for the current URL: the screen of the resource the URL names. */
export function RouteScreen() {
  const { pathname, search } = useLocation();
  const r = resourceFromPath(pathname, search);
  if (!r)
    return (
      <div style={{ padding: 24, color: "var(--text-muted)" }}>
        Nothing here. Press the workspace name to go back to its overview.
      </div>
    );
  return <ScreenFor ws={r.ws} id={r.id} />;
}

/** `/w/:ws/projects` was the old projects page; projects now open one per tab from the sidebar. */
export function ProjectsRedirect() {
  const { ws = "" } = useParams();
  return <Navigate replace to={`/w/${encodeURIComponent(ws)}`} />;
}
