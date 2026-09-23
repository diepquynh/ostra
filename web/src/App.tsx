import { useEffect, useState } from "react";
import { createBrowserRouter, Navigate, RouterProvider, useParams, type RouteObject } from "react-router";
import { api, onUnauthorized } from "./api";
import { Spinner } from "./design";
import { useAsync } from "./lib/hooks";
import { Home } from "./shell/Home";
import { ProjectsRedirect, RouteScreen } from "./shell/RouteScreen";
import { SignIn } from "./shell/SignIn";
import { WorkspaceShell } from "./shell/WorkspaceShell";

function Loading() {
  return (
    <div style={{ padding: 24, display: "flex", gap: 8, alignItems: "center", color: "var(--text-muted)" }}>
      <Spinner size={11} /> Loading…
    </div>
  );
}

function Problem({ message }: { message: string }) {
  return <div style={{ padding: 24, color: "var(--bad)" }}>{message}</div>;
}

/** `/s/:id` resolves the session's workspace, for deep links (push notifications) that only know the session. */
function SessionRedirect() {
  const { id = "" } = useParams();
  const s = useAsync(() => api.session(id), [id]);
  if (s.error) return <Problem message={s.error.message} />;
  if (!s.data) return <Loading />;
  return <Navigate replace to={`/w/${encodeURIComponent(s.data.summary.workspace)}/s/${encodeURIComponent(id)}${location.hash}`} />;
}

function ExecutionRedirect() {
  const { id = "" } = useParams();
  const e = useAsync(async () => {
    const x = await api.execution(id);
    if (!x.session) return null;
    return (await api.session(x.session)).summary.workspace;
  }, [id]);
  if (e.error) return <Problem message={e.error.message} />;
  if (e.loading) return <Loading />;
  return e.data ? <Navigate replace to={`/w/${encodeURIComponent(e.data)}/x/${encodeURIComponent(id)}`} /> : <Problem message="This execution belongs to no session." />;
}

function NotFound() {
  return (
    <div style={{ padding: 24, color: "var(--text-secondary)" }}>
      Nothing here. <a href="/">Go back to the workspace list.</a>
    </div>
  );
}

const screen = { element: <RouteScreen /> };

export const routes: RouteObject[] = [
  ...(import.meta.env.DEV ? [{ path: "/_design", lazy: async () => ({ Component: (await import("./design/Gallery")).default }) }] : []),
  { path: "/", element: <Home /> },
  {
    path: "/w/:ws",
    element: <WorkspaceShell />,
    children: [
      { index: true, ...screen },
      { path: "cost", ...screen },
      { path: "settings", ...screen },
      { path: "memory", ...screen },
      { path: "s/:id", ...screen },
      { path: "x/:id", ...screen },
      { path: "artifact", ...screen },
      { path: "p/:key", ...screen },
      { path: "f/:key/*", ...screen },
      { path: "projects", element: <ProjectsRedirect /> },
      { path: "*", ...screen },
    ],
  },
  { path: "/s/:id", element: <SessionRedirect /> },
  { path: "/x/:id", element: <ExecutionRedirect /> },
  { path: "*", element: <NotFound /> },
];

let router: ReturnType<typeof createBrowserRouter> | null = null;

export function App({ exchangeError }: { exchangeError: string | null }) {
  const [unauthorized, setUnauthorized] = useState(false);
  useEffect(() => onUnauthorized(() => setUnauthorized(true)), []);
  if (unauthorized) return <SignIn error={exchangeError} />;
  router ??= createBrowserRouter(routes);
  return <RouterProvider router={router} />;
}
