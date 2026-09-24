import { useEffect, useMemo, useState } from "react";
import { api } from "../api";
import type { GitBranch, GitChange, GitOpResult, GitRepoStatus, ProjectView } from "../api/types";
import { Banner, Button, Icon, IconButton, Input, Menu, Select, Spinner, TreeItem, type MenuItem } from "../design";
import { useAsync } from "../lib/hooks";
import { useProjectFsChanges } from "../lib/live";
import { throttle } from "../lib/store";
import { GIT_MARK } from "../screens/project/files";
import { fileIcon } from "./FilesPanel";

/** Reload the status this often while the panel shows, because edits outside Ostra send no message. */
const POLL_MS = 10_000;

type GitPanelProps = {
  ws: string;
  projects: ProjectView[];
  project: string | null;
  setProject: (key: string) => void;
  selected: { key: string; path: string } | null;
  onOpenFile: (key: string, path: string) => void;
  onAddProject: () => void;
};

type Section = "conflicted" | "staged" | "unstaged";

/** Paths an unstage or stage of `c` covers: a rename's source goes with it. */
const pathsOf = (c: GitChange) => (c.orig_path ? [c.path, c.orig_path] : [c.path]);

/** Left-dock Git tab: branch, fetch, pull, push, the commit box, and the staged and unstaged changes of one project. */
export function GitPanel({ ws, projects, project, setProject, selected, onOpenFile, onAddProject }: GitPanelProps) {
  const key = project && projects.some((p) => p.key === project) ? project : (projects[0]?.key ?? null);
  const status = useAsync(() => (key ? api.gitStatus(ws, key) : Promise.resolve(null)), [ws, key]);
  const [running, setRunning] = useState<string | null>(null);
  const [result, setResult] = useState<{ tone: "bad" | "info"; text: string } | null>(null);
  const [message, setMessage] = useState("");
  const [branchMenu, setBranchMenu] = useState(false);
  const [newBranch, setNewBranch] = useState<string | null>(null);
  const branches = useAsync(() => (key && branchMenu ? api.gitBranches(ws, key) : Promise.resolve([] as GitBranch[])), [ws, key, branchMenu]);

  const reload = useMemo(() => throttle(() => status.reload(), 500), [status.reload]);
  useEffect(() => reload.cancel, [reload]);
  useProjectFsChanges(ws, key, reload);
  useEffect(() => {
    const onFocus = () => reload();
    window.addEventListener("focus", onFocus);
    const t = setInterval(() => document.visibilityState === "visible" && reload(), POLL_MS);
    return () => {
      window.removeEventListener("focus", onFocus);
      clearInterval(t);
    };
  }, [reload]);
  useEffect(() => setResult(null), [key]);

  if (!key)
    return (
      <div style={{ padding: 12, display: "flex", flexDirection: "column", gap: 10, color: "var(--text-muted)", fontSize: "var(--text-sm)" }}>
        This workspace has no projects yet. Add one to see its git changes.
        <div>
          <Button size="sm" icon="folder-plus" onClick={onAddProject}>
            Add project
          </Button>
        </div>
      </div>
    );

  const st: GitRepoStatus | null = status.data;
  const locked = !!running || !!st?.busy;
  const canCommit = !!message.trim() && !!st && (st.staged.length > 0 || st.staged_elsewhere > 0) && st.conflicted.length === 0 && !locked;
  const clean = st && !st.staged.length && !st.unstaged.length && !st.conflicted.length;

  /** Run one git command; a status in its answer replaces the shown one. */
  const run = (label: string, fn: () => Promise<GitOpResult | { output: string; status?: undefined }>, done?: (out: string) => string | null) => {
    setRunning(label);
    setResult(null);
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

  const stage = (paths: string[]) => run("stage", () => api.gitStage(ws, key, paths));
  const unstage = (paths: string[]) => run("unstage", () => api.gitUnstage(ws, key, paths));
  const commit = () => {
    if (!canCommit) return;
    run("commit", () => api.gitCommit(ws, key, message), (out) => {
      setMessage("");
      return out.split("\n")[0] || "Committed.";
    });
  };
  const checkout = (branch: string, create = false) => {
    setBranchMenu(false);
    setNewBranch(null);
    run("checkout", () => api.gitCheckout(ws, key, { branch, create, start: null }), () => (create ? `Created and switched to ${branch}.` : `Switched to ${branch}.`));
  };

  const branchItems = (): MenuItem[] => {
    if (branches.loading && !branches.data?.length) return [{ type: "heading", label: "Reading the branches…" }];
    const list = branches.data ?? [];
    const item = (b: GitBranch): MenuItem => ({
      id: b.name,
      label: b.name,
      sub: b.subject ? `${b.commit} ${b.subject}` : b.commit,
      icon: b.remote ? "globe" : "git-branch",
      checked: b.current,
      onSelect: () => !b.current && checkout(b.name),
    });
    const local = list.filter((b) => !b.remote);
    const remote = list.filter((b) => b.remote);
    return [
      { id: "new", label: "Create a branch…", icon: "plus", onSelect: () => (setBranchMenu(false), setNewBranch("")) },
      ...(local.length ? [{ type: "heading", label: "Branches" } as const, ...local.map(item)] : []),
      ...(remote.length ? [{ type: "heading", label: "Remote branches" } as const, ...remote.map(item)] : []),
    ];
  };

  const renderRow = (c: GitChange, section: Section) => {
    const mark = GIT_MARK[c.mark];
    const name = c.path.split("/").pop() ?? c.path;
    const dir = c.path.split("/").slice(0, -1).join("/");
    const gone = c.mark === "D";
    return (
      <TreeItem
        key={`${section}:${c.path}`}
        label={<span style={{ color: mark.color, textDecoration: gone ? "line-through" : undefined }}>{name}</span>}
        icon={fileIcon(c.path)}
        meta={dir || null}
        selected={selected?.key === key && selected.path === c.path}
        onClick={() => !gone && onOpenFile(key, c.path)}
        title={`${c.path} · ${mark.word}${c.orig_path ? ` from ${c.orig_path}` : ""}${section === "staged" ? ", staged" : ""}`}
        trailing={
          <span style={{ display: "inline-flex", alignItems: "center", gap: 2, flex: "none" }}>
            {section !== "conflicted" && (
              <span className="git-row-action">
                <IconButton
                  size="sm"
                  icon={section === "staged" ? "minus" : "plus"}
                  label={section === "staged" ? `Unstage ${c.path}` : `Stage ${c.path}`}
                  disabled={locked}
                  onClick={(e) => {
                    e.stopPropagation();
                    (section === "staged" ? unstage : stage)(pathsOf(c));
                  }}
                />
              </span>
            )}
            <span style={{ color: mark.color, fontWeight: 600, width: 12, textAlign: "center", fontSize: "var(--text-sm)" }}>{mark.letter}</span>
          </span>
        }
      />
    );
  };

  const section = (id: Section, title: string, list: GitChange[], action?: { icon: "plus" | "minus"; label: string; onClick: () => void }) =>
    list.length > 0 && (
      <div key={id}>
        <div className="os-tree-section">
          <span>{title}</span>
          <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
            {action && <IconButton size="sm" icon={action.icon} label={action.label} disabled={locked} onClick={action.onClick} />}
            <span style={{ fontFamily: "var(--font-mono)" }}>{list.length}</span>
          </span>
        </div>
        {list.map((c) => renderRow(c, id))}
      </div>
    );

  return (
    <div style={{ display: "flex", flexDirection: "column", minHeight: 0, flex: 1 }}>
      <div style={{ display: "flex", flexDirection: "column", gap: 6, padding: "8px 8px 6px" }}>
        <div style={{ display: "flex", gap: 4 }}>
          <div style={{ flex: 1, minWidth: 0 }}>
            <Select size="sm" mono aria-label="Project" value={key} onChange={(e) => setProject(e.target.value)} options={projects.map((p) => ({ value: p.key, label: p.key }))} />
          </div>
          <IconButton size="sm" icon="refresh-ccw" label="Fetch from the remote" disabled={locked || !st?.is_git} onClick={() => run("fetch", () => api.gitFetch(ws, key), () => "Fetched.")} />
          <IconButton
            size="sm"
            icon="arrow-down"
            label={st?.behind ? `Pull ${st.behind} commit${st.behind === 1 ? "" : "s"}` : "Pull"}
            disabled={locked || !st?.upstream}
            onClick={() => run("pull", () => api.pullProject(ws, key), (out) => out.split("\n").pop() || "Pulled.")}
          />
          <IconButton
            size="sm"
            icon="arrow-up"
            label={st?.upstream ? (st.ahead ? `Push ${st.ahead} commit${st.ahead === 1 ? "" : "s"}` : "Push") : "Push and set the upstream"}
            disabled={locked || !st?.branch}
            onClick={() => run("push", () => api.gitPush(ws, key), () => "Pushed.")}
          />
        </div>
        {st?.is_git && (
          <div style={{ display: "flex", alignItems: "center", gap: 6, minWidth: 0 }}>
            <div style={{ position: "relative", minWidth: 0, flex: "0 1 auto" }}>
              <Button size="sm" variant="ghost" icon="git-branch" iconRight="chevron-down" disabled={locked} onClick={() => setBranchMenu((o) => !o)} title="Switch or create a branch">
                <span style={{ fontFamily: "var(--font-mono)", overflow: "hidden", textOverflow: "ellipsis" }}>{st.branch ?? `detached at ${st.head ?? "?"}`}</span>
              </Button>
              <Menu open={branchMenu} onClose={() => setBranchMenu(false)} items={branchItems()} label="Branches" width={280} />
            </div>
            <span style={{ flex: 1 }} />
            {st.upstream && (
              <span style={{ font: "var(--text-xs)/1 var(--font-mono)", color: "var(--text-muted)", whiteSpace: "nowrap" }} title={`Tracking ${st.upstream}`}>
                {st.ahead > 0 && <span title={`${st.ahead} to push`}>↑{st.ahead} </span>}
                {st.behind > 0 && <span title={`${st.behind} to pull`}>↓{st.behind} </span>}
                {st.upstream}
              </span>
            )}
          </div>
        )}
        {newBranch !== null && (
          <Input
            size="sm"
            mono
            autoFocus
            icon="git-branch"
            aria-label="New branch name"
            placeholder="New branch from HEAD"
            value={newBranch}
            onChange={(e) => setNewBranch(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && newBranch.trim()) checkout(newBranch.trim(), true);
              if (e.key === "Escape") setNewBranch(null);
            }}
            onBlur={() => !newBranch.trim() && setNewBranch(null)}
          />
        )}
        {st?.is_git && (
          <>
            <Input
              multiline
              size="sm"
              rows={2}
              aria-label="Commit message"
              placeholder="Commit message (Ctrl+Enter commits)"
              value={message}
              disabled={!!st.busy}
              onChange={(e) => setMessage(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
                  e.preventDefault();
                  commit();
                }
              }}
            />
            <Button size="sm" variant="primary" icon={running === "commit" ? undefined : "check"} disabled={!canCommit} onClick={commit} style={{ justifyContent: "center" }}>
              {running === "commit" ? <Spinner size={10} /> : null}
              {st.staged.length ? `Commit ${st.staged.length} file${st.staged.length === 1 ? "" : "s"}` : "Commit"}
            </Button>
            {st.staged_elsewhere > 0 && (
              <div style={{ fontSize: "var(--text-xs)", color: "var(--warn)", display: "flex", gap: 5, alignItems: "center" }}>
                <Icon name="circle-alert" size={12} />
                {st.staged_elsewhere} staged file{st.staged_elsewhere === 1 ? "" : "s"} outside this project will be committed too, because a commit takes the whole index.
              </div>
            )}
          </>
        )}
        {st?.busy && <Banner tone="info">{st.busy}</Banner>}
        {running && running !== "commit" && (
          <div style={{ fontSize: "var(--text-sm)", color: "var(--text-muted)", display: "flex", gap: 6, alignItems: "center" }}>
            <Spinner size={10} /> Running git {running}…
          </div>
        )}
        {result && (
          <Banner tone={result.tone}>
            <span style={{ whiteSpace: "pre-wrap", wordBreak: "break-word" }}>{result.text}</span>
          </Banner>
        )}
      </div>
      <div role="tree" aria-label={`Git changes in ${key}`} style={{ flex: 1, overflowY: "auto", overflowX: "hidden", padding: "0 8px 8px" }}>
        {!st ? (
          status.error ? (
            <div style={{ padding: "12px 8px", color: "var(--bad)", fontSize: "var(--text-sm)" }}>{status.error.message}</div>
          ) : (
            <div style={{ padding: "12px 8px", color: "var(--text-muted)", fontSize: "var(--text-sm)", display: "flex", gap: 6, alignItems: "center" }}>
              <Spinner size={10} /> Reading git status…
            </div>
          )
        ) : !st.is_git ? (
          <div style={{ padding: "12px 8px", color: "var(--text-muted)", fontSize: "var(--text-sm)" }}>
            <code>{key}</code> is not in a git repository.
          </div>
        ) : clean ? (
          <div style={{ padding: "12px 8px", color: "var(--text-muted)", fontSize: "var(--text-sm)" }}>No changes. The work tree matches {st.head ?? "the empty repository"}.</div>
        ) : (
          <>
            {section("conflicted", "Merge conflicts", st.conflicted)}
            {section("staged", "Staged changes", st.staged, { icon: "minus", label: "Unstage all", onClick: () => unstage([]) })}
            {section("unstaged", "Changes", st.unstaged, { icon: "plus", label: "Stage all", onClick: () => stage([]) })}
            {st.truncated && <div style={{ padding: "6px 8px", color: "var(--text-muted)", fontSize: "var(--text-xs)" }}>Only the first 2,000 changes of each list are shown.</div>}
          </>
        )}
      </div>
    </div>
  );
}
