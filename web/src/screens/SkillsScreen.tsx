import { useEffect, useMemo, useState } from "react";
import { useLocation } from "react-router";
import { api } from "../api";
import type { ProjectSkills, SkillDoc, SkillView } from "../api/types";
import { Banner, Button, Chip, Dialog, Input, Panel, Select } from "../design";
import { useAsync } from "../lib/hooks";
import { useShell, useWorkspace } from "../lib/nav";
import { LoadError, Loading, Page } from "./workspace/Page";
import "./skills.css";

export type SkillsScreenProps = { ws: string };

const KINDS = ["creation", "test", "convention", "module-hub", "other"];

const TEMPLATE = (name: string) => `---
name: ${name}
description: Use when a task ... Covers ...
---

# ${name}

Steps an agent follows, in order.
`;

type Selection = { project: string; origin: SkillView["origin"]; path: string } | { project: string; creating: true };

const keyOf = (project: string, s: Pick<SkillView, "origin" | "path">) => `${project}\u0000${s.origin}\u0000${s.path}`;

function state(s: SkillView): { label: string; tone: "ok" | "warn" | "bad" | "neutral" } {
  if (s.origin === "harness") return { label: s.path.split("/").slice(0, 2).join("/"), tone: "neutral" };
  if (!s.exists) return { label: "File missing", tone: "bad" };
  if (!s.entry) return { label: "Not registered", tone: "warn" };
  return { label: s.entry.kind, tone: "ok" };
}

/**
 * Resource `ws:skills`: every project's skills in one place. Registered skills reach every agent through the repo
 * brief; skills in a harness directory such as `.claude/skills/` can be adopted into `.agents/skills/`.
 * `?project=<key>` narrows the list to one project.
 */
export function SkillsScreen({ ws }: SkillsScreenProps) {
  const { detail } = useWorkspace();
  const { addProject } = useShell();
  const { search } = useLocation();
  const [filter, setFilter] = useState<string>(() => new URLSearchParams(search).get("project") ?? "");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<Selection | null>(null);
  const all = useAsync(() => api.skills(ws), [ws]);

  useEffect(() => {
    const p = new URLSearchParams(search).get("project");
    if (p) setFilter(p);
  }, [search]);

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (all.data ?? [])
      .filter((g) => !filter || g.project === filter)
      .map((g) => ({
        ...g,
        skills: q
          ? g.skills.filter((s) =>
              `${s.name} ${s.description ?? ""} ${s.entry?.component_type ?? ""}`.toLowerCase().includes(q),
            )
          : g.skills,
      }));
  }, [all.data, filter, query]);

  const projects = detail?.projects ?? [];
  const group = selected ? all.data?.find((g) => g.project === selected.project) : undefined;
  const skill =
    selected && !("creating" in selected)
      ? group?.skills.find((s) => keyOf(selected.project, s) === keyOf(selected.project, selected))
      : undefined;

  const newSkill = () => {
    const project = filter || projects.find((p) => p.init_status !== "missing")?.key;
    if (project) setSelected({ project, creating: true });
  };

  if (!detail) {
    return (
      <Page title="Skills">
        <Loading>Reading the projects…</Loading>
      </Page>
    );
  }

  return (
    <Page
      title="Skills"
      actions={
        projects.length > 0 && (
          <Button size="sm" icon="plus" onClick={newSkill}>
            New skill
          </Button>
        )
      }
    >
      <p className="wp-lead">
        Skills are the per-project instructions agents load before they create a component, write a test, or follow a
        convention. Only skills registered in <code>.ostra/project.toml</code> reach agents, through the repo brief.
        Skills in a harness directory such as <code>.claude/skills/</code> are listed so you can adopt them into{" "}
        <code>.agents/skills/</code>.
      </p>
      {projects.length === 0 ? (
        <Banner
          tone="info"
          actions={
            <Button size="sm" icon="folder-plus" onClick={addProject}>
              Add project
            </Button>
          }
        >
          This workspace has no projects yet. Skills belong to a project, so add one first.
        </Banner>
      ) : (
        <>
          <div className="wp-row">
            <Select
              size="sm"
              aria-label="Project"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              options={[{ value: "", label: "All projects" }, ...projects.map((p) => ({ value: p.key, label: p.key }))]}
            />
            <span className="wp-spacer" />
            <Input
              size="sm"
              icon="search"
              type="search"
              aria-label="Search skills"
              placeholder="Search skills"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              style={{ width: 260 }}
            />
          </div>
          {all.error && <LoadError error={all.error} onRetry={all.reload} />}
          <div className="sk-layout">
            <div className="sk-list">
              {all.loading && !all.data && <Loading>Reading the skills…</Loading>}
              {groups.map((g) => (
                <SkillGroup key={g.project} group={g} selected={selected} query={query.trim()} onSelect={setSelected} />
              ))}
            </div>
            <div className="sk-editor">
              {selected && "creating" in selected ? (
                <SkillEditor
                  key={`new:${selected.project}`}
                  ws={ws}
                  projects={(all.data ?? []).filter((g) => !g.blocked).map((g) => g.project)}
                  project={selected.project}
                  skill={null}
                  blocked={null}
                  onSaved={(project, doc) => {
                    all.reload();
                    setSelected({ project, origin: doc.skill.origin, path: doc.skill.path });
                  }}
                  onDeleted={() => setSelected(null)}
                />
              ) : selected && skill ? (
                <SkillEditor
                  key={keyOf(selected.project, skill)}
                  ws={ws}
                  projects={[selected.project]}
                  project={selected.project}
                  skill={skill}
                  blocked={group?.blocked ?? null}
                  onSaved={(project, doc) => {
                    all.reload();
                    setSelected({ project, origin: doc.skill.origin, path: doc.skill.path });
                  }}
                  onDeleted={() => {
                    all.reload();
                    setSelected(null);
                  }}
                />
              ) : (
                <Panel>
                  <div className="wp-muted">Select a skill to read or edit it, or create a new one.</div>
                </Panel>
              )}
            </div>
          </div>
        </>
      )}
    </Page>
  );
}

function SkillGroup({
  group,
  selected,
  query,
  onSelect,
}: {
  group: ProjectSkills;
  selected: Selection | null;
  query: string;
  onSelect: (s: Selection) => void;
}) {
  const registered = group.skills.filter((s) => s.entry).length;
  return (
    <Panel title={group.project} subtitle={`${registered} registered`} icon="folder-git-2" bodyFlush>
      {group.blocked && (
        <div className="sk-note">
          <Banner tone="warn">{group.blocked}</Banner>
        </div>
      )}
      {group.skills.length === 0 ? (
        <div className="sk-note wp-muted">
          {query ? `No skills match “${query}”.` : "No skills yet. Initialize the project or create one."}
        </div>
      ) : (
        <ul className="sk-rows">
          {group.skills.map((s) => {
            const k = keyOf(group.project, s);
            const active = !!selected && !("creating" in selected) && keyOf(selected.project, selected) === k;
            const st = state(s);
            return (
              <li key={k}>
                <button
                  type="button"
                  className="sk-row"
                  data-active={active || undefined}
                  onClick={() => onSelect({ project: group.project, origin: s.origin, path: s.path })}
                >
                  <span className="sk-row__head">
                    <span className="wp-mono sk-row__name">{s.name}</span>
                    <Chip tone={st.tone}>{st.label}</Chip>
                  </span>
                  {s.description && <span className="sk-row__desc">{s.description}</span>}
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </Panel>
  );
}

function SkillEditor({
  ws,
  projects,
  project: initialProject,
  skill,
  blocked,
  onSaved,
  onDeleted,
}: {
  ws: string;
  projects: string[];
  project: string;
  skill: SkillView | null;
  blocked: string | null;
  onSaved: (project: string, doc: SkillDoc) => void;
  onDeleted: () => void;
}) {
  const creating = skill === null;
  const harness = skill?.origin === "harness";
  const [project, setProject] = useState(initialProject);
  const doc = useAsync<SkillDoc | null>(
    () =>
      skill
        ? harness
          ? api.harnessSkill(ws, project, skill.path)
          : skill.exists
            ? api.skill(ws, project, skill.name)
            : Promise.resolve(null)
        : Promise.resolve(null),
    [ws, project, skill?.path],
  );
  const [name, setName] = useState(skill?.name ?? "");
  const [kind, setKind] = useState(skill?.entry?.kind ?? "creation");
  const [componentType, setComponentType] = useState(skill?.entry?.component_type ?? "");
  const [content, setContent] = useState<string | null>(creating ? TEMPLATE("new-skill") : null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [adopting, setAdopting] = useState(false);

  useEffect(() => {
    if (doc.data && content === null) setContent(doc.data.content);
  }, [doc.data, content]);

  const text = content ?? "";
  const original = doc.data?.content ?? "";
  const dirty =
    creating ||
    !skill?.entry ||
    !skill.exists ||
    text !== original ||
    kind !== (skill.entry.kind ?? "") ||
    (componentType || null) !== (skill.entry.component_type ?? null);

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const save = () =>
    run(async () => {
      const saved = await api.saveSkill(ws, project, name.trim(), {
        kind,
        component_type: componentType.trim() || null,
        content: text,
      });
      setContent(saved.content);
      doc.set(saved);
      onSaved(project, saved);
    });

  const title = creating ? "New skill" : skill.name;
  const saveLabel = creating
    ? "Create the skill"
    : !skill.entry
      ? "Register the skill"
      : !skill.exists
        ? "Write SKILL.md"
        : "Save";

  return (
    <Panel
      title={<span className="wp-mono">{title}</span>}
      subtitle={skill ? skill.path : project}
      icon="book-open"
      actions={
        harness ? (
          <Button size="sm" variant="primary" icon="file-plus" disabled={!!blocked} onClick={() => setAdopting(true)}>
            Adopt into Ostra
          </Button>
        ) : (
          <span className="wp-row" style={{ gap: 6 }}>
            {!creating && (
              <Button
                size="sm"
                variant="danger"
                icon="trash-2"
                disabled={busy || !!blocked}
                onClick={() => setConfirmDelete(true)}
              >
                Delete
              </Button>
            )}
            <Button
              size="sm"
              variant="primary"
              icon="check"
              disabled={busy || !!blocked || !dirty || !name.trim() || !text.trim()}
              onClick={() => void save()}
            >
              {busy ? "Saving…" : saveLabel}
            </Button>
          </span>
        )
      }
    >
      <div className="wp-stack">
        {blocked && <Banner tone="warn">{blocked}</Banner>}
        {skill && !harness && !skill.entry && (
          <Banner tone="warn" title="Agents do not load this skill yet">
            It is in <code>{skill.path.replace(/\/[^/]+\/SKILL\.md$/, "/")}</code> but not in <code>project.toml</code>.
            Set its kind and register it so the repo brief lists it.
          </Banner>
        )}
        {skill && !skill.exists && (
          <Banner tone="bad" title="SKILL.md is missing">
            <code>project.toml</code> names <code>{skill.path}</code>, but the file is not there. Write it here, or
            delete the entry.
          </Banner>
        )}
        {harness && (
          <Banner tone="info" title="Executions do not load this skill">
            It lives in a harness directory. Adopting copies its folder into <code>.agents/skills/</code> and registers
            it, so every executor loads it.
          </Banner>
        )}
        {error && <Banner tone="bad">{error}</Banner>}
        {!harness && (
          <div className="sk-fields">
            {creating ? (
              <>
                {projects.length > 1 && (
                  <Select
                    label="Project"
                    value={project}
                    onChange={(e) => setProject(e.target.value)}
                    options={projects}
                  />
                )}
                <Input
                  label="Name"
                  mono
                  placeholder="entity"
                  hint="Letters, digits, - and _. It becomes the folder under .agents/skills/."
                  value={name}
                  onChange={(e) => {
                    const next = e.target.value;
                    if (text === TEMPLATE(name || "new-skill")) setContent(TEMPLATE(next || "new-skill"));
                    setName(next);
                  }}
                />
              </>
            ) : null}
            <Select
              label="Kind"
              value={kind}
              onChange={(e) => setKind(e.target.value)}
              options={KINDS}
              hint="Convention and module-hub skills reach every agent; the others reach the agents that create or test code."
            />
            <Input
              label="Use for"
              placeholder="JPA entity"
              hint="The component type this skill creates. The brief shows it next to the skill."
              value={componentType}
              onChange={(e) => setComponentType(e.target.value)}
            />
          </div>
        )}
        {doc.loading && !creating && content === null && skill?.exists ? (
          <Loading>Reading SKILL.md…</Loading>
        ) : doc.error ? (
          <LoadError error={doc.error} onRetry={doc.reload} />
        ) : harness ? (
          <pre className="sk-preview">{doc.data?.content}</pre>
        ) : (
          <Input
            label="SKILL.md"
            multiline
            mono
            rows={24}
            spellCheck={false}
            className="sk-content"
            value={text}
            onChange={(e) => setContent(e.target.value)}
          />
        )}
      </div>
      {confirmDelete && skill && (
        <Dialog
          title={`Delete ${skill.name}?`}
          width={480}
          onClose={() => setConfirmDelete(false)}
          footer={
            <>
              <span className="wp-spacer" />
              <Button onClick={() => setConfirmDelete(false)}>Keep it</Button>
              <Button
                variant="danger"
                disabled={busy}
                onClick={() =>
                  void run(async () => {
                    await api.deleteSkill(ws, project, skill.name);
                    setConfirmDelete(false);
                    onDeleted();
                  })
                }
              >
                Delete the skill
              </Button>
            </>
          }
        >
          <div className="wp-stack">
            <span>
              This removes the entry from <code>project.toml</code> and deletes{" "}
              <code>{skill.path.replace(/\/SKILL\.md$/, "/")}</code> with every file in it. Later executions in{" "}
              {project} will no longer load it.
            </span>
            {error && <Banner tone="bad">{error}</Banner>}
          </div>
        </Dialog>
      )}
      {adopting && skill && (
        <AdoptDialog
          skill={skill}
          error={error}
          busy={busy}
          onClose={() => setAdopting(false)}
          onAdopt={(asName, asKind, ct) =>
            void run(async () => {
              const saved = await api.adoptSkill(ws, project, asName, {
                from: skill.path,
                kind: asKind,
                component_type: ct || null,
              });
              setAdopting(false);
              onSaved(project, saved);
            })
          }
        />
      )}
    </Panel>
  );
}

function AdoptDialog({
  skill,
  error,
  busy,
  onClose,
  onAdopt,
}: {
  skill: SkillView;
  error: string | null;
  busy: boolean;
  onClose: () => void;
  onAdopt: (name: string, kind: string, componentType: string) => void;
}) {
  const [name, setName] = useState(skill.name);
  const [kind, setKind] = useState("creation");
  const [ct, setCt] = useState("");
  return (
    <Dialog
      title="Adopt the skill"
      subtitle={skill.path}
      width={520}
      onClose={onClose}
      footer={
        <>
          <span className="wp-spacer" />
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="primary"
            disabled={busy || !name.trim()}
            onClick={() => onAdopt(name.trim(), kind, ct.trim())}
          >
            Adopt
          </Button>
        </>
      }
    >
      <div className="wp-stack">
        <Input
          label="Name"
          mono
          hint="The folder under .agents/skills/. The harness copy stays where it is."
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <Select label="Kind" value={kind} onChange={(e) => setKind(e.target.value)} options={KINDS} />
        <Input label="Use for" placeholder="JPA entity" value={ct} onChange={(e) => setCt(e.target.value)} />
        {error && <Banner tone="bad">{error}</Banner>}
      </div>
    </Dialog>
  );
}
