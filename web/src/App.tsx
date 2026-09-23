import { useEffect, useState } from "react";
import { createBrowserRouter, Navigate, RouterProvider, useParams, type RouteObject } from "react-router";
import { api, onUnauthorized } from "./api";
import { exchangeToken, tokenFromInput } from "./auth";
import { Layout } from "./components/Layout";
import { ErrorBox, Loading } from "./components/Status";
import { ArtifactPage } from "./features/artifacts/ArtifactPage";
import { BoardPage } from "./features/board/BoardPage";
import { CostPage } from "./features/cost/CostPage";
import { ExecutionPage } from "./features/executions/ExecutionPage";
import { MemoryPage } from "./features/memory/MemoryPage";
import { SettingsPage } from "./features/settings/SettingsPage";
import { HomePage } from "./features/workspaces/HomePage";
import { ProjectsPage } from "./features/workspaces/ProjectsPage";
import { WorkspacePage } from "./features/workspaces/WorkspacePage";
import { useAsync } from "./lib/hooks";

/** `/s/:id` resolves the session's workspace, for deep links that only know the session. */
function SessionRedirect() {
  const { id = "" } = useParams();
  const s = useAsync(() => api.session(id), [id]);
  if (s.error) return <ErrorBox error={s.error} />;
  if (!s.data) return <Loading />;
  return <Navigate replace to={`/w/${s.data.summary.workspace}/s/${id}${location.hash}`} />;
}

function ExecutionRedirect() {
  const { id = "" } = useParams();
  const e = useAsync(async () => {
    const x = await api.execution(id);
    if (!x.session) return null;
    return (await api.session(x.session)).summary.workspace;
  }, [id]);
  if (e.error) return <ErrorBox error={e.error} />;
  if (e.loading) return <Loading />;
  return e.data ? <Navigate replace to={`/w/${e.data}/x/${id}`} /> : <div className="empty">This execution belongs to no session.</div>;
}

function NotFound() {
  return <div className="empty">Nothing here. Go back to the workspace list.</div>;
}

export const routes: RouteObject[] = [
  {
    path: "/",
    element: <Layout />,
    children: [
      { index: true, element: <HomePage /> },
      { path: "w/:ws", element: <WorkspacePage /> },
      { path: "w/:ws/projects", element: <ProjectsPage /> },
      { path: "w/:ws/settings", element: <SettingsPage /> },
      { path: "w/:ws/memory", element: <MemoryPage /> },
      { path: "w/:ws/cost", element: <CostPage /> },
      { path: "w/:ws/s/:id", element: <BoardPage /> },
      { path: "w/:ws/x/:id", element: <ExecutionPage /> },
      { path: "w/:ws/artifact", element: <ArtifactPage /> },
      { path: "s/:id", element: <SessionRedirect /> },
      { path: "x/:id", element: <ExecutionRedirect /> },
      { path: "*", element: <NotFound /> },
    ],
  },
];

let router: ReturnType<typeof createBrowserRouter> | null = null;

function SignIn({ error }: { error: string | null }) {
  const [link, setLink] = useState("");
  const [message, setMessage] = useState<string | null>(error);
  const [busy, setBusy] = useState(false);
  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const token = tokenFromInput(link);
    if (!token) {
      setMessage("Paste the whole link the ostra command printed, or just the token after #token=.");
      return;
    }
    setBusy(true);
    const err = await exchangeToken(token);
    setBusy(false);
    if (err) setMessage(err);
    else location.replace("/");
  };
  return (
    <div className="page narrow" style={{ paddingTop: 80 }}>
      <div className="card">
        <h1>Sign in to Ostra</h1>
        <p>
          This browser has no session with the Ostra server. Open the URL that the <code>ostra</code> command printed in your terminal, or
          paste it below. It carries a one-time token that signs this browser in.
        </p>
        <form onSubmit={submit} className="row" style={{ gap: 8 }}>
          <input
            aria-label="Sign-in link"
            placeholder="http://…/#token=…"
            value={link}
            onChange={(e) => setLink(e.target.value)}
            style={{ flex: 1 }}
          />
          <button type="submit" className="primary" disabled={busy || !link.trim()}>
            Sign in
          </button>
        </form>
        <p className="muted small">
          Each link works once and expires after 15 minutes. Run <code>ostra url</code> on the server for a new one; the server keeps
          running.
        </p>
        {message && <div className="form-error">{message}</div>}
      </div>
    </div>
  );
}

export function App({ exchangeError }: { exchangeError: string | null }) {
  const [unauthorized, setUnauthorized] = useState(false);
  useEffect(() => onUnauthorized(() => setUnauthorized(true)), []);
  if (unauthorized) return <SignIn error={exchangeError} />;
  router ??= createBrowserRouter(routes);
  return <RouterProvider router={router} />;
}
