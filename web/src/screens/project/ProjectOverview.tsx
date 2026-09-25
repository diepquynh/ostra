import { useState } from "react";
import { useNavigate } from "react-router";
import { api } from "../../api";
import type { ProjectView } from "../../api/types";
import type { Commands } from "../../api/gen/Commands";
import { Banner, Button, Chip, Dialog, Panel, Stepper, Table } from "../../design";
import { CommandsPanel } from "./CommandsPanel";
import { useAsync } from "../../lib/hooks";
import { useWorkspaceTree } from "../../lib/live";
import { useNav, useWorkspace } from "../../lib/nav";
import { resourcePath } from "../../lib/resource";

const INIT_STEPS = [{ label: "Detect" }, { label: "Scout" }, { label: "Propose skills" }, { label: "Your approval" }, { label: "Generate" }, { label: "Inventory" }];

const EMPTY_COMMANDS: Commands = { build: null, test: null, test_one: null, format: null, lint: null, typecheck: null, run: null };

/** Overview tab: commands, skills, module map and maintenance, or the Initialize flow for a project that has none. */
export function ProjectOverview({ ws, project }: { ws: string; project: ProjectView }) {
  const p = project;
  const nav = useNav();
  const navigate = useNavigate();
  const { reload } = useWorkspace();
  const tree = useWorkspaceTree(ws);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const lessons = useAsync(() => (p.init_status === "initialized" ? api.lessons(ws, p.key) : Promise.resolve([])), [ws, p.key, p.init_status]);
  const initSession = tree.sessions.find((s) => s.kind.kind === "init" && s.kind.project === p.key);

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
    setBusy(true);
    api.removeProject(ws, p.key).then(
      () => {
        setConfirmRemove(false);
        reload();
        nav.close(`project:${p.key}`);
      },
      (e: Error) => {
        setBusy(false);
        setConfirmRemove(false);
        setError(e.message);
      },
    );
  };

  const removeButton = (variant: "ghost" | "danger") => (
    <Button size="sm" variant={variant} onClick={() => setConfirmRemove(true)}>
      Remove from workspace
    </Button>
  );

  const profile = p.profile;

  return (
    <div style={{ padding: "18px 24px 40px", display: "flex", flexDirection: "column", gap: 16 }}>
      {error && <Banner tone="bad">{error}</Banner>}
      {p.init_status === "missing" ? (
        <Banner tone="bad" title="Folder missing" actions={removeButton("danger")}>
          Ostra cannot find <code>{p.path}</code>. Restore the folder or remove the project from the workspace. Removing deletes nothing on disk.
        </Banner>
      ) : p.init_status === "initializing" ? (
        <Panel title="Initializing" icon="sparkles" tone="highlight">
          <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
            <div style={{ lineHeight: 1.55, color: "var(--text-secondary)" }}>
              The init session is scouting the code and proposing skills. It stops for your approval before it writes any skill.
            </div>
            <Stepper orientation="horizontal" current={1} steps={INIT_STEPS} />
            {initSession && (
              <div>
                <Button variant="primary" iconRight="arrow-right" onClick={() => nav.open(`session:${initSession.id}`)}>
                  Open the init session
                </Button>
              </div>
            )}
          </div>
        </Panel>
      ) : p.init_status === "not_initialized" ? (
        <Panel title="Initialize this project" icon="sparkles" tone="warn">
          <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
            <div style={{ lineHeight: 1.55, color: "var(--text-secondary)" }}>
              Initializing scouts the code for recurring patterns, proposes skills for your approval, then writes <code>.ostra/INVENTORY.md</code>,{" "}
              <code>.ostra/project.toml</code>, and the skills every agent loads. No pipeline task can target this project until it is done.
            </div>
            {p.ultracode_bootstrap && (
              <div style={{ fontSize: "var(--text-sm)", color: "var(--text-secondary)" }}>
                This folder has an Ultracode bootstrap in <code>.ultracode/</code>. Initializing can reuse its skills.
              </div>
            )}
            <Stepper orientation="horizontal" current={0} steps={INIT_STEPS} />
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
              <Button variant="primary" icon="play" disabled={busy} onClick={init}>
                {busy ? "Starting…" : `Initialize ${p.key}`}
              </Button>
              <Button variant="ghost" icon="refresh-ccw" onClick={reload}>
                Check again
              </Button>
              {removeButton("ghost")}
            </div>
          </div>
        </Panel>
      ) : (
        <>
          <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(280px, 1fr))", gap: 16 }}>
            <CommandsPanel ws={ws} projectKey={p.key} commands={profile?.commands ?? EMPTY_COMMANDS} onSaved={reload} />
            <Panel
              title="Skills"
              subtitle={profile?.skills.length ?? 0}
              icon="book-open"
              actions={
                <Button size="sm" variant="ghost" icon="pencil" onClick={() => navigate(`${resourcePath(ws, "ws:skills")}?project=${encodeURIComponent(p.key)}`)}>
                  Manage
                </Button>
              }
            >
              {profile?.skills.length ? (
                <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
                  {profile.skills.map((s) => (
                    <Chip key={s.name} mono outline title={`${s.kind} · ${s.path}`}>
                      {s.name}
                    </Chip>
                  ))}
                </div>
              ) : (
                <div style={{ fontSize: "var(--text-sm)", color: "var(--text-muted)" }}>No skills yet.</div>
              )}
              <div style={{ fontSize: "var(--text-sm)", color: "var(--text-muted)", marginTop: 10 }}>
                {profile?.review_rules.length ?? 0} review rules · {lessons.data ? `${lessons.data.length} lessons in memory` : "reading lessons…"}
              </div>
            </Panel>
            <Panel title="Maintenance" icon="wrench">
              <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
                <Button size="sm" icon="refresh-ccw" disabled={busy} onClick={init}>
                  Re-initialize
                </Button>
                <Button size="sm" icon="brain" onClick={() => navigate(`${resourcePath(ws, "ws:memory")}?project=${encodeURIComponent(p.key)}`)}>
                  Memory
                </Button>
                {removeButton("danger")}
              </div>
              <div style={{ fontSize: "var(--text-sm)", color: "var(--text-muted)", marginTop: 10 }}>Removing deletes nothing on disk.</div>
            </Panel>
          </div>
          {profile && profile.module_map.length > 0 && (
            <Panel title="Modules" subtitle={profile.module_map.length} icon="list-tree" bodyFlush>
              <Table
                dense
                rowKey="glob"
                rows={profile.module_map}
                columns={[
                  { key: "glob", label: "Files", render: (m) => <code>{m.glob}</code> },
                  { key: "area", label: "Area" },
                  { key: "reference", label: "Reference", render: (m) => (m.reference ? <code>{m.reference}</code> : null) },
                ]}
              />
            </Panel>
          )}
          {profile && profile.review_rules.length > 0 && (
            <Panel title="Review rules" subtitle={profile.review_rules.length} icon="list-checks" bodyFlush>
              <Table
                dense
                rowKey="id"
                rows={profile.review_rules}
                columns={[
                  { key: "id", label: "Rule", width: 110, render: (r) => <code>{r.id}</code> },
                  { key: "rule", label: "What it checks" },
                  { key: "severity", label: "Severity", width: 80 },
                  { key: "auto_fixable", label: "Auto-fix", width: 80, render: (r) => (r.auto_fixable ? "yes" : "") },
                ]}
              />
            </Panel>
          )}
        </>
      )}
      {confirmRemove && (
        <Dialog
          title={`Remove ${p.key} from this workspace?`}
          onClose={() => setConfirmRemove(false)}
          width={460}
          footer={
            <>
              <span style={{ flex: 1 }} />
              <Button onClick={() => setConfirmRemove(false)}>Cancel</Button>
              <Button variant="danger" disabled={busy} onClick={remove}>
                Remove {p.key}
              </Button>
            </>
          }
        >
          <div style={{ lineHeight: 1.55, color: "var(--text-secondary)" }}>
            Nothing on disk is deleted. <code>{p.path}</code> and its <code>.ostra/</code> folder stay, so you can import it again later.
          </div>
        </Dialog>
      )}
    </div>
  );
}
