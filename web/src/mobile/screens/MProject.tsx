import { Chip, Icon, type IconName, Spinner } from "@ostra/design";
import { useEffect, useMemo, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router";
import { api } from "../../api";
import type { GitRepoStatus, ProjectView } from "../../api/types";
import { ARTIFACTS_ROOT } from "../../features/context/tags";
import { useAsync } from "../../lib/hooks";
import { useProjectFsChanges, useWorkspaceTree } from "../../lib/live";
import { useNav, useShell, useWorkspace } from "../../lib/nav";
import { resourcePath } from "../../lib/resource";
import { throttle } from "../../lib/store";
import { useMobileHeader } from "../header";
import { MFile } from "./MFile";
import { MFiles } from "./MFiles";
import { MGit } from "./MGit";
import { type ProjectTab, parseProjectHash, projectHash, stackDepth } from "./projectNav";
import "./MProject.css";

const COMMAND_LABEL: Record<string, string> = {
  build: "Build",
  test: "Test",
  test_one: "One test",
  format: "Format",
  lint: "Lint",
  typecheck: "Typecheck",
  run: "Run",
};

/** `project:<key>` and `file:<key>:<path>`: a project's Overview, Files and Git tabs, a folder, or one file. */
export function MProject({ ws, projectKey, path }: { ws: string; projectKey: string; path: string | null }) {
  const location = useLocation();
  const navigate = useNavigate();
  const rootRef = useRef<HTMLDivElement>(null);
  const view = parseProjectHash(path ? "" : location.hash);
  const art = projectKey === ARTIFACTS_ROOT;
  const depth = stackDepth(location.state);
  const dirName = !path && view.dir ? view.dir.split("/").pop() : null;
  useMobileHeader(dirName, dirName ? `${art ? "artifacts" : projectKey}/${view.dir}` : undefined);
  // The shell reads the depth from history state to decide whether Back pops or goes to the overview.
  const go = (next: { tab: ProjectTab; dir: string }, replace: boolean) =>
    navigate(`${location.pathname}${location.search}${projectHash(next)}`, {
      replace,
      state: { depth: replace ? depth : depth + 1 },
    });

  // biome-ignore lint/correctness/useExhaustiveDependencies: a new folder starts at the top.
  useEffect(() => {
    rootRef.current?.scrollTo?.({ top: 0 });
    rootRef.current?.closest(".m-scroll")?.scrollTo?.({ top: 0 });
  }, [view.dir, view.tab, path]);

  return (
    <div ref={rootRef} className="mp-root">
      <div className="mp-page">
        {path ? (
          <MFile ws={ws} projectKey={projectKey} path={path} />
        ) : art ? (
          <MFiles
            ws={ws}
            projectKey={projectKey}
            dir={view.dir}
            root={!view.dir}
            openDir={(dir) => go({ tab: "files", dir }, false)}
          />
        ) : (
          <ProjectTabs ws={ws} projectKey={projectKey} tab={view.tab} dir={view.dir} go={go} />
        )}
      </div>
    </div>
  );
}

type TabsProps = {
  ws: string;
  projectKey: string;
  tab: ProjectTab;
  dir: string;
  go: (next: { tab: ProjectTab; dir: string }, replace: boolean) => void;
};

function ProjectTabs({ ws, projectKey, tab, dir, go }: TabsProps) {
  const { detail } = useWorkspace();
  const shell = useShell();
  const p = detail?.projects.find((x) => x.key === projectKey);
  const status = useAsync<GitRepoStatus | null>(
    () => (p?.is_git ? api.gitStatus(ws, projectKey) : Promise.resolve(null)),
    [ws, projectKey, p?.is_git],
  );
  const reload = useMemo(() => throttle(() => status.reload(), 500), [status.reload]);
  useEffect(() => reload.cancel, [reload]);
  useProjectFsChanges(ws, projectKey, reload);

  if (!detail)
    return (
      <div className="mp-pad">
        <span className="m-muted" style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <Spinner size={11} /> Reading the projects…
        </span>
      </div>
    );
  if (!p)
    return (
      <div className="mp-pad">
        <span className="m-muted">
          No project <code>{projectKey}</code> in this workspace. It may have been removed.
        </span>
        <button type="button" className="m-btn" onClick={shell.addProject}>
          <Icon name="folder-plus" size={15} /> Add project
        </button>
      </div>
    );

  const st = status.data;
  const changed = st ? st.staged.length + st.unstaged.length + st.conflicted.length : null;
  const tabs: { id: ProjectTab; label: string; count?: number | null }[] = [
    { id: "overview", label: "Overview" },
    { id: "files", label: "Files" },
    ...(p.is_git ? [{ id: "git" as const, label: "Git", count: changed || null }] : []),
  ];

  return (
    <>
      {!dir && (
        <div
          className="mp-seg"
          role="tablist"
          aria-label="Project sections"
          style={{ gridTemplateColumns: `repeat(${tabs.length}, minmax(0, 1fr))` }}
        >
          {tabs.map((t) => (
            <button
              type="button"
              role="tab"
              key={t.id}
              aria-selected={tab === t.id}
              onClick={() => go({ tab: t.id, dir: "" }, true)}
            >
              {t.label}
              {t.count ? <span className="mp-count">{t.count}</span> : null}
            </button>
          ))}
        </div>
      )}
      {tab === "files" ? (
        <MFiles
          ws={ws}
          projectKey={projectKey}
          dir={dir}
          root={!dir}
          openDir={(d) => go({ tab: "files", dir: d }, false)}
        />
      ) : tab === "git" && p.is_git ? (
        <MGit ws={ws} projectKey={projectKey} status={status} />
      ) : (
        <Overview ws={ws} project={p} status={st} />
      )}
    </>
  );
}

function Overview({ ws, project: p, status }: { ws: string; project: ProjectView; status: GitRepoStatus | null }) {
  const nav = useNav();
  const navigate = useNavigate();
  const { reload } = useWorkspace();
  const tree = useWorkspaceTree(ws);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [pulled, setPulled] = useState<string | null>(null);
  const initialized = p.init_status === "initialized";
  const lessons = useAsync(
    () => (initialized ? api.lessons(ws, p.key) : Promise.resolve([])),
    [ws, p.key, initialized],
  );
  const initSession = tree.sessions.find((s) => s.kind.kind === "init" && s.kind.project === p.key);
  const profile = p.profile;

  const init = () => {
    setBusy(true);
    setError(null);
    api.initProject(ws, p.key).then(
      (s) => {
        setBusy(false);
        reload();
        nav.open(`session:${s.id}`);
      },
      (e: Error) => {
        setBusy(false);
        setError(e.message);
      },
    );
  };
  const remove = () => {
    if (!confirmRemove) return setConfirmRemove(true);
    setBusy(true);
    api.removeProject(ws, p.key).then(
      () => {
        reload();
        nav.open("ws:overview");
      },
      (e: Error) => {
        setBusy(false);
        setConfirmRemove(false);
        setError(e.message);
      },
    );
  };
  const pull = () => {
    setBusy(true);
    setPulled(null);
    api
      .pullProject(ws, p.key)
      .then(
        (r) =>
          setPulled(
            r.updated
              ? `Pulled ${r.branch ?? "the branch"} from ${r.before ?? "?"} to ${r.after ?? "?"}.`
              : `${r.branch ?? "The branch"} is already up to date.`,
          ),
        (e: Error) => setError(e.message),
      )
      .finally(() => setBusy(false));
  };
  const depth = stackDepth(useLocation().state);
  const withProject = (id: string) =>
    navigate(`${resourcePath(ws, id)}?project=${encodeURIComponent(p.key)}`, { state: { depth: depth + 1 } });

  const facts: [string, string][] = [
    ["Branch", p.git_branch ?? (p.is_git ? "git" : "none")],
    ["Ahead", status ? String(status.ahead) : p.is_git ? "…" : "0"],
    [
      "Changed",
      status ? String(status.staged.length + status.unstaged.length + status.conflicted.length) : p.is_git ? "…" : "0",
    ],
    ["Skills", String(profile?.skills.length ?? 0)],
    ["Rules", String(profile?.review_rules.length ?? 0)],
    ["Lessons", lessons.data ? String(lessons.data.length) : initialized ? "…" : "0"],
  ];
  const commands = Object.entries(profile?.commands ?? {}).filter((e): e is [string, string] => !!e[1]);
  const maint: { icon: IconName; label: string; sub: string; onTap: () => void; danger?: boolean }[] = [
    ...(p.is_git
      ? [
          {
            icon: "arrow-down" as const,
            label: "Pull",
            sub: "Fast-forwards the checked-out branch from its upstream",
            onTap: pull,
          },
        ]
      : []),
    ...(initialized
      ? [
          {
            icon: "refresh-ccw" as const,
            label: "Re-initialize",
            sub: "Rescans the code and regenerates the inventory, commands and skills",
            onTap: init,
          },
          {
            icon: "brain" as const,
            label: "Memory",
            sub: "Lessons the reviewers recorded for this project",
            onTap: () => withProject("ws:memory"),
          },
        ]
      : []),
    {
      icon: "trash-2",
      label: confirmRemove ? `Tap again to remove ${p.key}` : "Remove from workspace",
      sub: `Deletes nothing on disk. ${p.path} and its .ostra folder stay, so you can import it again.`,
      onTap: remove,
      danger: true,
    },
  ];

  return (
    <div className="mp-pad">
      {error && <span className="mp-error">{error}</span>}
      {pulled && (
        <div className="mp-note">
          <span className="mp-result">{pulled}</span>
        </div>
      )}
      {p.init_status === "missing" && (
        <div className="mp-callout mp-bad">
          <span className="mp-callout-title">Folder missing</span>
          <span className="mp-callout-text">
            Ostra cannot find <code>{p.path}</code>. Restore the folder or remove the project from the workspace.
          </span>
        </div>
      )}
      {p.init_status === "not_initialized" && (
        <div className="mp-callout">
          <span className="mp-callout-title">Not initialized</span>
          <span className="mp-callout-text">
            Initialize {p.key} so Ostra can generate its inventory, build commands and skills. Sessions cannot use it
            until then.
            {p.ultracode_bootstrap && " Its Ultracode bootstrap in .ultracode/ can be reused."}
          </span>
          <button type="button" className="m-btn m-btn-primary" disabled={busy} onClick={init}>
            {busy ? "Starting…" : `Initialize ${p.key}`}
          </button>
        </div>
      )}
      {p.init_status === "initializing" && (
        <div className="mp-callout">
          <span className="mp-callout-title">Initializing</span>
          <span className="mp-callout-text">
            The init session is scouting the code and proposing skills. It stops for your approval before it writes any
            skill.
          </span>
          {initSession && (
            <button type="button" className="m-btn m-btn-primary" onClick={() => nav.open(`session:${initSession.id}`)}>
              Open the init session
            </button>
          )}
        </div>
      )}
      <div className="mp-facts">
        {facts.map(([k, v]) => (
          <div key={k} className="mp-fact">
            <span className="m-label">{k}</span>
            <span className="mp-fact-v">{v}</span>
          </div>
        ))}
      </div>
      {commands.length > 0 && (
        <section className="m-section" style={{ gap: 8 }}>
          <span className="m-label">Commands</span>
          <div className="mp-list">
            {commands.map(([k, v]) => (
              <div key={k} className="mp-cmd">
                <span className="mp-cmd-k">{COMMAND_LABEL[k] ?? k}</span>
                <span className="mp-cmd-v">{v}</span>
              </div>
            ))}
          </div>
        </section>
      )}
      <section className="m-section" style={{ gap: 8 }}>
        <div style={{ display: "flex", alignItems: "center" }}>
          <span className="m-label" style={{ flex: 1 }}>
            Skills
          </span>
          <button type="button" className="mp-link" onClick={() => withProject("ws:skills")}>
            Manage
          </button>
        </div>
        {profile?.skills.length ? (
          <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
            {profile.skills.map((s) => (
              <Chip key={s.name} mono icon="book-open" title={`${s.kind} · ${s.path}`}>
                {s.name}
              </Chip>
            ))}
          </div>
        ) : (
          <span className="m-muted">No skills yet.</span>
        )}
      </section>
      <section className="m-section" style={{ gap: 8 }}>
        <span className="m-label">Maintenance</span>
        <div className="mp-list">
          {maint.map((m) => (
            <button type="button" key={m.icon} className="mp-item" disabled={busy} onClick={m.onTap}>
              <Icon
                name={m.icon}
                size={15}
                style={{ color: m.danger ? "var(--bad)" : "var(--text-muted)", flex: "none" }}
              />
              <span className="mp-item-body">
                <span style={{ fontSize: 13.5, color: m.danger ? "var(--bad)" : undefined }}>{m.label}</span>
                <span className="mp-item-sub">{m.sub}</span>
              </span>
            </button>
          ))}
        </div>
      </section>
    </div>
  );
}
