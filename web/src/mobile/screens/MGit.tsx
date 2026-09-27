import { fileIcon, Icon, Spinner } from "@ostra/design";
import { useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import type { GitBranch, GitChange, GitOpResult, GitRepoStatus } from "../../api/types";
import { type Async, useAsync } from "../../lib/hooks";
import { useNav } from "../../lib/nav";
import { fileId } from "../../lib/resource";
import { throttle } from "../../lib/store";
import { GIT_MARK } from "../../screens/project/files";

/** Reload the status this often while the tab shows, because edits outside Ostra send no message. */
const POLL_MS = 10_000;

type Section = "conflicted" | "staged" | "unstaged";

const pathsOf = (c: GitChange) => (c.orig_path ? [c.path, c.orig_path] : [c.path]);
const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** The Git tab of a project: branch, fetch, pull, push, the commit box, and the changes. */
export function MGit({
  ws,
  projectKey,
  status,
}: {
  ws: string;
  projectKey: string;
  status: Async<GitRepoStatus | null>;
}) {
  const nav = useNav();
  const [running, setRunning] = useState<string | null>(null);
  const [result, setResult] = useState<{ tone: "bad" | "info"; text: string } | null>(null);
  const [message, setMessage] = useState("");
  const [menu, setMenu] = useState(false);
  const [naming, setNaming] = useState(false);
  const [newBranch, setNewBranch] = useState("");
  const branches = useAsync(
    () => (menu ? api.gitBranches(ws, projectKey) : Promise.resolve([] as GitBranch[])),
    [ws, projectKey, menu],
  );

  const reload = useMemo(() => throttle(() => status.reload(), 500), [status.reload]);
  useEffect(() => reload.cancel, [reload]);
  useEffect(() => {
    const onFocus = () => reload();
    window.addEventListener("focus", onFocus);
    const t = setInterval(() => document.visibilityState === "visible" && reload(), POLL_MS);
    return () => {
      window.removeEventListener("focus", onFocus);
      clearInterval(t);
    };
  }, [reload]);

  const st = status.data;
  if (!st)
    return (
      <div className="mp-pad">
        {status.error ? (
          <span className="mp-error">{status.error.message}</span>
        ) : (
          <span className="m-muted" style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <Spinner size={11} /> Reading git status…
          </span>
        )}
      </div>
    );
  if (!st.is_git)
    return (
      <div className="mp-pad">
        <span className="m-muted">
          <code>{projectKey}</code> is not in a git repository.
        </span>
      </div>
    );

  const locked = !!running || !!st.busy;
  const canCommit =
    !!message.trim() && (st.staged.length > 0 || st.staged_elsewhere > 0) && st.conflicted.length === 0 && !locked;

  const run = (
    label: string,
    fn: () => Promise<GitOpResult | { output: string; status?: undefined }>,
    done?: (out: string) => string | null,
  ) => {
    setRunning(label);
    setResult(null);
    setMenu(false);
    fn()
      .then((r) => {
        if (r.status) status.set(r.status);
        else status.reload();
        const text = done?.(r.output) ?? null;
        if (text) setResult({ tone: "info", text });
      })
      .catch((e: Error) => {
        setResult({ tone: "bad", text: e.message });
        status.reload();
      })
      .finally(() => setRunning(null));
  };
  const stage = (paths: string[]) => run("stage", () => api.gitStage(ws, projectKey, paths));
  const unstage = (paths: string[]) => run("unstage", () => api.gitUnstage(ws, projectKey, paths));
  const checkout = (branch: string, create = false) => {
    setNaming(false);
    setNewBranch("");
    run(
      "checkout",
      () => api.gitCheckout(ws, projectKey, { branch, create, start: null }),
      () => (create ? `Created and switched to ${branch}.` : `Switched to ${branch}.`),
    );
  };
  const commit = () =>
    canCommit &&
    run(
      "commit",
      () => api.gitCommit(ws, projectKey, message),
      (out) => {
        setMessage("");
        return out.split("\n")[0] || "Committed.";
      },
    );

  const list = branches.data ?? [];
  const branchGroups = [
    { label: "Branches", items: list.filter((b) => !b.remote) },
    { label: "Remote branches", items: list.filter((b) => b.remote) },
  ].filter((g) => g.items.length);

  const section = (id: Section, title: string, items: GitChange[], all?: { label: string; onTap: () => void }) =>
    items.length > 0 && (
      <div key={id} style={{ display: "flex", flexDirection: "column" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "0 4px 0 16px" }}>
          <span className="m-label" style={{ flex: 1 }}>
            {title} <span style={{ fontFamily: "var(--font-mono)" }}>{items.length}</span>
          </span>
          {all && (
            <button
              type="button"
              className="mp-link"
              style={{ height: 40, padding: "0 12px" }}
              disabled={locked}
              onClick={all.onTap}
            >
              {all.label}
            </button>
          )}
        </div>
        {items.map((c) => {
          const mark = GIT_MARK[c.mark];
          const name = c.path.split("/").pop() ?? c.path;
          const dir = c.path.split("/").slice(0, -1).join("/") || projectKey;
          const gone = c.mark === "D";
          return (
            <div key={`${id}:${c.path}`} className="mp-node">
              <div className="mp-node-line">
                <button
                  type="button"
                  className="mp-node-btn"
                  style={{ minHeight: 52 }}
                  onClick={() => !gone && nav.open(fileId(projectKey, c.path))}
                >
                  <Icon name={fileIcon(c.path)} size={16} style={{ color: "var(--text-muted)", flex: "none" }} />
                  <span className="mp-item-body" style={{ gap: 1 }}>
                    <span
                      className="mp-node-name"
                      style={{ color: mark.color, textDecoration: gone ? "line-through" : undefined }}
                    >
                      {name}
                    </span>
                    <span className="mp-node-sub">
                      {dir} · {mark.word}
                      {c.orig_path ? ` from ${c.orig_path}` : ""}
                    </span>
                  </span>
                </button>
                {id !== "conflicted" && (
                  <button
                    type="button"
                    className="mp-more"
                    style={{ height: 52, color: "var(--text-secondary)" }}
                    disabled={locked}
                    aria-label={`${id === "staged" ? "Unstage" : "Stage"} ${c.path}`}
                    onClick={() => (id === "staged" ? unstage : stage)(pathsOf(c))}
                  >
                    <Icon name={id === "staged" ? "minus" : "plus"} size={17} />
                  </button>
                )}
              </div>
            </div>
          );
        })}
      </div>
    );

  const clean = !st.staged.length && !st.unstaged.length && !st.conflicted.length;
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
      <div className="mp-pad" style={{ gap: 10 }}>
        <div className="mp-tools" style={{ alignItems: "center" }}>
          <button
            type="button"
            className="m-btn"
            style={{
              flex: 1,
              minWidth: 0,
              justifyContent: "flex-start",
              gap: 8,
              padding: "0 12px",
              background: "var(--surface-raised)",
            }}
            disabled={locked}
            title="Switch or create a branch"
            aria-expanded={menu}
            onClick={() => {
              setMenu((o) => !o);
              setNaming(false);
            }}
          >
            <Icon name="git-branch" size={15} style={{ color: "var(--text-muted)", flex: "none" }} />
            <span className="mp-node-name" style={{ flex: 1, fontSize: 13.5 }}>
              {st.branch ?? `detached at ${st.head ?? "?"}`}
            </span>
            <Icon name="chevron-down" size={15} style={{ color: "var(--text-muted)", flex: "none" }} />
          </button>
          <button
            type="button"
            className="mp-tool"
            aria-label="Fetch from the remote"
            disabled={locked}
            onClick={() =>
              run(
                "fetch",
                () => api.gitFetch(ws, projectKey),
                () => "Fetched.",
              )
            }
          >
            <Icon name="refresh-ccw" size={17} />
          </button>
          <button
            type="button"
            className="mp-tool"
            aria-label={st.behind ? `Pull ${plural(st.behind, "commit")}` : "Pull"}
            disabled={locked || !st.upstream}
            onClick={() =>
              run(
                "pull",
                () => api.pullProject(ws, projectKey),
                (out) => out.split("\n").pop() || "Pulled.",
              )
            }
          >
            <Icon name="arrow-down" size={17} />
            {st.behind > 0 && <span className="mp-tool-badge">{st.behind}</span>}
          </button>
          <button
            type="button"
            className="mp-tool"
            aria-label={
              st.upstream ? (st.ahead ? `Push ${plural(st.ahead, "commit")}` : "Push") : "Push and set the upstream"
            }
            disabled={locked || !st.branch}
            onClick={() =>
              run(
                "push",
                () => api.gitPush(ws, projectKey),
                () => "Pushed.",
              )
            }
          >
            <Icon name="arrow-up" size={17} />
            {st.ahead > 0 && <span className="mp-tool-badge">{st.ahead}</span>}
          </button>
        </div>
        <span className="m-mono-sub">
          {st.upstream
            ? `Tracking ${st.upstream}${st.ahead ? ` · ↑${st.ahead}` : ""}${st.behind ? ` · ↓${st.behind}` : ""}`
            : st.branch
              ? `No upstream. Push sets origin/${st.branch}.`
              : "Detached HEAD. Switch to a branch to push."}
        </span>
        {menu && (
          <div className="mp-branches">
            {naming ? (
              <form
                style={{ display: "flex", gap: 6, padding: 8, borderBottom: "1px solid var(--border-subtle)" }}
                onSubmit={(e) => {
                  e.preventDefault();
                  if (newBranch.trim()) checkout(newBranch.trim(), true);
                }}
              >
                <input
                  autoFocus
                  className="m-input mp-mono-input"
                  aria-label="New branch name"
                  placeholder="New branch from HEAD"
                  value={newBranch}
                  onChange={(e) => setNewBranch(e.target.value)}
                />
                <button type="submit" className="m-btn m-btn-primary" disabled={!newBranch.trim()}>
                  Create
                </button>
              </form>
            ) : (
              <button
                type="button"
                className="mp-branch"
                style={{ minHeight: 48, borderBottom: "1px solid var(--border-subtle)", fontSize: 14 }}
                onClick={() => setNaming(true)}
              >
                <Icon name="plus" size={15} style={{ color: "var(--text-muted)" }} />
                Create a branch…
              </button>
            )}
            {branches.loading && !list.length && (
              <span className="m-muted" style={{ display: "flex", gap: 8, padding: 12 }}>
                <Spinner size={11} /> Reading the branches…
              </span>
            )}
            {branches.error && (
              <span className="mp-error" style={{ padding: 12 }}>
                {branches.error.message}
              </span>
            )}
            {branchGroups.map((g) => (
              <div key={g.label} style={{ display: "flex", flexDirection: "column" }}>
                <span className="m-label" style={{ padding: "8px 12px 4px" }}>
                  {g.label}
                </span>
                {g.items.map((b) => (
                  <button
                    type="button"
                    key={b.name}
                    className="mp-branch"
                    onClick={() => (b.current ? setMenu(false) : checkout(b.name))}
                  >
                    <Icon name={b.remote ? "globe" : "git-branch"} size={15} style={{ color: "var(--text-muted)" }} />
                    <span className="mp-item-body" style={{ gap: 1 }}>
                      <span className="mp-node-name">{b.name}</span>
                      <span className="mp-node-sub">{b.subject ? `${b.commit} ${b.subject}` : b.commit}</span>
                    </span>
                    {b.current && <Icon name="check" size={15} style={{ color: "var(--accent)" }} />}
                  </button>
                ))}
              </div>
            ))}
          </div>
        )}
        <textarea
          className="m-input"
          rows={2}
          aria-label="Commit message"
          placeholder="Commit message"
          value={message}
          disabled={!!st.busy}
          onChange={(e) => setMessage(e.target.value)}
        />
        <button type="button" className="m-btn m-btn-primary" disabled={!canCommit} onClick={commit}>
          {running === "commit" ? <Spinner size={12} /> : <Icon name="check" size={15} />}
          {running === "commit"
            ? "Committing…"
            : st.staged.length
              ? `Commit ${plural(st.staged.length, "file")}`
              : "Commit"}
        </button>
        {st.staged_elsewhere > 0 && (
          <span style={{ display: "flex", gap: 6, fontSize: 12.5, color: "var(--warn)", lineHeight: 1.45 }}>
            <Icon name="circle-alert" size={14} style={{ flex: "none", marginTop: 1 }} />
            {plural(st.staged_elsewhere, "staged file")} outside this project will be committed too, because a commit
            takes the whole index.
          </span>
        )}
        {st.busy && (
          <div className="mp-note">
            <Icon name="info" size={15} style={{ color: "var(--info)", flex: "none", marginTop: 2 }} />
            <span style={{ textWrap: "pretty" }}>{st.busy}</span>
          </div>
        )}
        {running && running !== "commit" && (
          <span className="m-muted" style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <Spinner size={12} /> Running git {running}…
          </span>
        )}
        {result && (
          <div className={`mp-note${result.tone === "bad" ? " mp-bad" : ""}`}>
            <span className="mp-result">{result.text}</span>
          </div>
        )}
      </div>
      {clean ? (
        <span className="m-muted" style={{ padding: "8px 16px" }}>
          No changes. The work tree matches {st.head ?? "the empty repository"}.
        </span>
      ) : (
        <>
          {section("conflicted", "Merge conflicts", st.conflicted)}
          {section("staged", "Staged changes", st.staged, { label: "Unstage all", onTap: () => unstage([]) })}
          {section("unstaged", "Changes", st.unstaged, { label: "Stage all", onTap: () => stage([]) })}
          {st.truncated && (
            <span className="m-help" style={{ padding: "0 16px" }}>
              Only the first 2,000 changes of each list are shown.
            </span>
          )}
        </>
      )}
    </div>
  );
}
