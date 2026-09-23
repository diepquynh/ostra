import { useState } from "react";
import { useNavigate } from "react-router";
import { api } from "../../api";
import type { WorkspaceDetail } from "../../api/types";
import { resourcePath } from "../../lib/resource";

/**
 * "Initialize <key>" after creating a workspace: start the init session, then open it. `open` runs first
 * so the caller's own navigation to the workspace is replaced by the session's URL.
 */
export function useInitAfterCreate(open: (d: WorkspaceDetail) => void) {
  const navigate = useNavigate();
  const [busy, setBusy] = useState(false);
  const init = (d: WorkspaceDetail, key: string) => {
    setBusy(true);
    api.initProject(d.id, key).then(
      (s) => {
        open(d);
        navigate(resourcePath(d.id, `session:${s.id}`));
      },
      () => {
        open(d);
        navigate(resourcePath(d.id, `project:${key}`));
      },
    );
  };
  return { init, busy };
}
