import { Banner, Chip, Icon, Spinner } from "@ostra/design";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { useLocation } from "react-router";
import { api } from "../../api";
import type { Lesson, ProjectSkills, SkillDoc, SkillView } from "../../api/types";
import { formatCost, formatTime } from "../../lib/format";
import { useAsync, useChannel, useThrottled } from "../../lib/hooks";
import { useActivity, useSessionSummaries } from "../../lib/live";
import { useNav, useShell, useWorkspace } from "../../lib/nav";
import { parseLessonAnchor } from "../../screens/MemoryScreen";
import { KINDS, skillState, TEMPLATE } from "../../screens/SkillsScreen";
import { COST_GROUPS, type CostGroup, costLines, costTotals } from "../../screens/workspace/costTable";
import { useAnchor } from "../../screens/workspace/Page";
import { useMobileHeader } from "../header";
import { parseSkillAnchor, skillAnchor } from "./workspaceLogic";
import "./MWorkspace.css";

export type MWorkspacePage = "cost" | "skills" | "memory";

/** `ws:cost`, `ws:skills`, `ws:memory`. */
export function MWorkspace({ ws, page }: { ws: string; page: MWorkspacePage }) {
  if (page === "cost") return <MCost ws={ws} />;
  if (page === "skills") return <MSkills ws={ws} />;
  return <MMemory ws={ws} />;
}

function Seg<T extends string>({
  label,
  value,
  onChange,
  items,
}: {
  label: string;
  value: T;
  onChange: (v: T) => void;
  items: { id: T; label: string }[];
}) {
  return (
    <div
      className="mw-seg"
      role="tablist"
      aria-label={label}
      style={{ gridTemplateColumns: `repeat(${items.length}, minmax(0, 1fr))` }}
    >
      {items.map((it) => (
        <button
          type="button"
          role="tab"
          key={it.id}
          aria-selected={value === it.id}
          className="mw-seg__item"
          onClick={() => onChange(it.id)}
        >
          {it.label}
        </button>
      ))}
    </div>
  );
}

function Loading({ children }: { children: ReactNode }) {
  return (
    <span className="m-muted" style={{ display: "flex", gap: 8, alignItems: "center" }}>
      <Spinner size={11} /> {children}
    </span>
  );
}

function Failed({ error, onRetry }: { error: Error; onRetry: () => void }) {
  return (
    <Banner tone="bad" style={{ margin: 0 }}>
      {error.message}{" "}
      <button type="button" className="mw-link" onClick={onRetry}>
        Try again
      </button>
    </Banner>
  );
}

// ---- Cost

type Range = "week" | "all";

/** Spend this week or all time, one grouping at a time, each row with its share as a bar. */
function MCost({ ws }: { ws: string }) {
  const [range, setRange] = useState<Range>("week");
  const [group, setGroup] = useState<CostGroup>("session");
  const { activity } = useActivity(ws);
  const since = range === "week" ? (activity?.week_since ?? null) : null;
  const waiting = range === "week" && since === null;
  const report = useAsync(() => (waiting ? Promise.resolve(null) : api.cost(ws, since)), [ws, since, waiting]);
  const { sessions } = useSessionSummaries(ws);
  const { open } = useNav();
  const refresh = useThrottled(report.reload, 2000);
  useChannel(`workspace:${ws}`, (m) => {
    if (m.type === "activity" || m.type === "session_updated") refresh();
  });

  const r = report.data;
  const g = COST_GROUPS.find((x) => x.id === group) ?? COST_GROUPS[0];
  const rows = useMemo(
    () => (r ? costLines(r[g.field], g.id, r.total.usage.cost_usd, sessions) : []),
    [r, g, sessions],
  );
  const raw = useMemo(() => new Map((r?.[g.field] ?? []).map((x) => [x.key, x.usage.cost_usd])), [r, g]);
  const max = Math.max(0, ...raw.values());
  const t = r ? costTotals(r) : null;

  return (
    <div className="m-page">
      <Seg<Range>
        label="Time range"
        value={range}
        onChange={setRange}
        items={[
          { id: "week", label: "This week" },
          { id: "all", label: "All time" },
        ]}
      />
      {report.error && !r && <Failed error={report.error} onRetry={report.reload} />}
      {!r && !report.error && <Loading>Reading the spend…</Loading>}
      {r && t && (
        <>
          <div className="m-card mw-total">
            <span className="m-label">{range === "week" ? "This week" : "All time"}</span>
            <span className="mw-total__value">{t.total}</span>
            <span style={{ fontSize: 12, color: "var(--text-muted)" }}>
              {t.runs} {t.runs === "1" ? "run" : "runs"}
              {activity && ` · ${formatCost(activity.spend_today_usd)} today`}
              {t.build !== "none" && ` · ${t.build} building`}
            </span>
            <span style={{ fontSize: 12, color: "var(--text-muted)" }}>
              Cache reads per tool call: {t.perCall}. A rising value means each step re-reads more context.
            </span>
          </div>
          {r.total.executions === 0 ? (
            <div className="m-section">
              <span className="m-muted">
                {range === "week"
                  ? "No spend this week. Switch to All time for earlier runs."
                  : "No spend yet. Costs appear here after a session runs its first execution."}
              </span>
              <button type="button" className="m-btn" onClick={() => open("ws:overview")}>
                <Icon name="plus" size={14} /> New task
              </button>
            </div>
          ) : (
            <section className="m-section" style={{ gap: 4 }}>
              <Seg<CostGroup>
                label="Group by"
                value={group}
                onChange={setGroup}
                items={COST_GROUPS.map((x) => ({ id: x.id, label: x.column }))}
              />
              {rows.map((c) => {
                const body = (
                  <>
                    <span style={{ display: "flex", alignItems: "baseline", gap: 8 }}>
                      <span className={c.mono ? "mw-cost__label mw-mono" : "mw-cost__label"}>{c.label}</span>
                      <span style={{ fontSize: 12, color: "var(--text-muted)", whiteSpace: "nowrap" }}>
                        {c.runs} {c.runs === 1 ? "run" : "runs"}
                      </span>
                      <span className="mw-mono" style={{ fontWeight: 500 }}>
                        {c.cost}
                      </span>
                    </span>
                    <progress
                      className="mw-bar"
                      value={raw.get(c.id) ?? 0}
                      max={max || 1}
                      aria-label={`${c.share} of the total`}
                    />
                    <span className="mw-mono" style={{ fontSize: 11, color: "var(--text-muted)" }}>
                      in {c.input} · out {c.output} · cache {c.cacheReads}
                      {c.build && ` · build ${c.build}`}
                    </span>
                  </>
                );
                return c.open ? (
                  <button
                    type="button"
                    key={c.id}
                    className="mw-cost mw-cost--link"
                    onClick={() => c.open && open(c.open)}
                  >
                    {body}
                  </button>
                ) : (
                  <div key={c.id} className="mw-cost">
                    {body}
                  </div>
                );
              })}
              {rows.length === 0 && <span className="m-muted">Nothing recorded for this grouping.</span>}
            </section>
          )}
        </>
      )}
    </div>
  );
}

// ---- Skills

const findSkill = (groups: ProjectSkills[] | null | undefined, project: string, origin: string, path: string) =>
  groups?.find((g) => g.project === project)?.skills.find((s) => s.origin === origin && s.path === path);

/** Every project's skills; a skill opens as `#skill:<project>:<origin>:<path>`, so Back returns to the list. */
function MSkills({ ws }: { ws: string }) {
  const { detail } = useWorkspace();
  const { addProject } = useShell();
  const { open } = useNav();
  const pushAnchor = (anchor: string, o: { replace?: boolean } = {}) => open("ws:skills", { anchor, push: !o.replace });
  const { anchor } = useAnchor();
  const { search } = useLocation();
  const all = useAsync(() => api.skills(ws), [ws]);
  const [filter, setFilter] = useState<string>(() => new URLSearchParams(search).get("project") ?? "all");
  const [query, setQuery] = useState("");

  const target = parseSkillAnchor(anchor);
  const projects = detail?.projects ?? [];
  const groups = all.data ?? [];

  if (target) {
    const group = groups.find((g) => g.project === target.project);
    if (target.creating)
      return (
        <SkillEditor
          key={`new:${target.project}`}
          ws={ws}
          project={target.project}
          projects={groups.filter((g) => !g.blocked).map((g) => g.project)}
          skill={null}
          blocked={group?.blocked ?? null}
          onSaved={(p, doc) => {
            all.reload();
            pushAnchor(skillAnchor(p, doc.skill), { replace: true });
          }}
        />
      );
    const skill = findSkill(groups, target.project, target.origin, target.path);
    if (!skill)
      return (
        <div className="m-page">
          {all.loading ? <Loading>Reading the skills…</Loading> : <span className="m-muted">This skill is gone.</span>}
        </div>
      );
    return (
      <SkillEditor
        key={`${target.project}:${skill.origin}:${skill.path}`}
        ws={ws}
        project={target.project}
        projects={[target.project]}
        skill={skill}
        blocked={group?.blocked ?? null}
        onSaved={(p, doc) => {
          all.reload();
          pushAnchor(skillAnchor(p, doc.skill), { replace: true });
        }}
        onDeleted={() => all.reload()}
      />
    );
  }

  const q = query.trim().toLowerCase();
  const shown = groups.filter((g) => filter === "all" || g.project === filter);
  const newSkill = () => {
    const project = filter !== "all" ? filter : projects.find((p) => p.init_status !== "missing")?.key;
    if (project) pushAnchor(`skill-new:${project}`);
  };

  return (
    <div className="m-page">
      {projects.length === 0 && detail ? (
        <Banner tone="info" style={{ margin: 0 }}>
          This workspace has no projects yet. Skills belong to a project, so add one first.{" "}
          <button type="button" className="mw-link" onClick={addProject}>
            Add project
          </button>
        </Banner>
      ) : (
        <>
          <div style={{ display: "flex", gap: 8 }}>
            <input
              className="m-input"
              type="search"
              aria-label="Find a skill"
              placeholder="Find a skill"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            <button type="button" className="m-btn m-btn-primary" style={{ flex: "none" }} onClick={newSkill}>
              <Icon name="plus" size={15} /> New
            </button>
          </div>
          <div className="mw-chips">
            {[
              { project: "all", count: groups.reduce((n, g) => n + g.skills.length, 0) },
              ...groups.map((g) => ({ project: g.project, count: g.skills.length })),
            ].map((f) => (
              <button
                type="button"
                key={f.project}
                aria-pressed={filter === f.project}
                className="mw-chip"
                onClick={() => setFilter(f.project)}
              >
                {f.project === "all" ? "All projects" : f.project}
                <span className="mw-chip__count">{f.count}</span>
              </button>
            ))}
          </div>
          <span className="m-help" style={{ marginTop: -10 }}>
            Only skills registered in .ostra/project.toml reach agents. Skills in a harness folder such as
            .claude/skills/ can be adopted.
          </span>
          {all.error && <Failed error={all.error} onRetry={all.reload} />}
          {all.loading && !all.data && <Loading>Reading the skills…</Loading>}
          {shown.map((g) => {
            const items = q
              ? g.skills.filter((s) =>
                  `${s.name} ${s.description ?? ""} ${s.entry?.component_type ?? ""}`.toLowerCase().includes(q),
                )
              : g.skills;
            const p = projects.find((x) => x.key === g.project);
            return (
              <section key={g.project} style={{ display: "flex", flexDirection: "column" }}>
                <span className="m-label" style={{ paddingBottom: 4 }}>
                  {g.project}
                </span>
                {g.blocked && (
                  <Banner tone="warn" style={{ margin: "4px 0" }}>
                    {g.blocked}
                  </Banner>
                )}
                {items.map((s) => {
                  const st = skillState(s);
                  return (
                    <button
                      type="button"
                      key={`${s.origin}:${s.path}`}
                      className="m-row"
                      style={{ minHeight: 56, gap: 10, padding: "6px 0" }}
                      onClick={() => pushAnchor(skillAnchor(g.project, s))}
                    >
                      <Icon
                        name={s.origin === "harness" ? "square-terminal" : "book-open"}
                        size={15}
                        style={{ color: "var(--text-muted)", flex: "none" }}
                      />
                      <span className="m-row-body">
                        <span className="mw-mono">{s.name}</span>
                        <span className="mw-desc">{s.description || s.path}</span>
                      </span>
                      <Chip tone={st.tone} mono={st.tone === "ok" || st.tone === "neutral"}>
                        {st.label}
                      </Chip>
                    </button>
                  );
                })}
                {items.length === 0 && (
                  <span className="m-muted" style={{ padding: "10px 0" }}>
                    {q
                      ? "No skills match."
                      : p && p.init_status !== "initialized"
                        ? "No skills. Initialize the project to generate them."
                        : "No skills yet."}
                  </span>
                )}
              </section>
            );
          })}
        </>
      )}
    </div>
  );
}

function Field({ label, children }: { label: ReactNode; children: ReactNode }) {
  return (
    <label className="mw-field">
      {label}
      {children}
    </label>
  );
}

function SkillEditor({
  ws,
  project: initialProject,
  projects,
  skill,
  blocked,
  onSaved,
  onDeleted,
}: {
  ws: string;
  project: string;
  projects: string[];
  skill: SkillView | null;
  blocked: string | null;
  onSaved: (project: string, doc: SkillDoc) => void;
  onDeleted?: () => void;
}) {
  const { close } = useNav();
  const creating = skill === null;
  useMobileHeader(creating ? "New skill" : skill.name, `Skill · ${initialProject}`);
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
  const [useFor, setUseFor] = useState(skill?.entry?.component_type ?? "");
  const [content, setContent] = useState<string | null>(creating ? TEMPLATE("new-skill") : null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [adopt, setAdopt] = useState<{ name: string; kind: string; useFor: string } | null>(null);
  const top = useRef<HTMLDivElement>(null);

  useEffect(() => top.current?.scrollIntoView?.({ block: "start" }), []);
  useEffect(() => {
    if (doc.data && content === null) setContent(doc.data.content);
  }, [doc.data, content]);

  const text = content ?? "";
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
      const doc2 = await api.saveSkill(ws, project, name.trim(), {
        kind,
        component_type: useFor.trim() || null,
        content: text,
      });
      setContent(doc2.content);
      doc.set(doc2);
      setSaved(`Saved to ${project}/${doc2.skill.path}`);
      onSaved(project, doc2);
    });

  const saveLabel = creating
    ? "Create skill"
    : !skill.entry
      ? "Save and register"
      : !skill.exists
        ? "Write SKILL.md"
        : "Save";
  const path = skill && !creating ? skill.path : `.agents/skills/${name.trim() || "name"}/SKILL.md`;

  if (harness && skill)
    return (
      <div ref={top} className="m-page">
        <div className="m-card" style={{ gap: 10, padding: "12px 14px" }}>
          <span className="mw-title">{skill.name}</span>
          <span style={{ fontSize: 13.5, lineHeight: 1.55, color: "var(--text-secondary)", textWrap: "pretty" }}>
            Found in {skill.path.split("/").slice(0, 2).join("/")}. Ostra reads it but does not route work to it. Adopt
            it to copy it into .agents/skills and register it.
          </span>
          {blocked && <span style={{ fontSize: 12.5, color: "var(--warn)" }}>{blocked}</span>}
          {!adopt ? (
            <button
              type="button"
              className="m-btn m-btn-primary"
              disabled={!!blocked}
              onClick={() => setAdopt({ name: skill.name, kind: "creation", useFor: "" })}
            >
              Adopt into Ostra
            </button>
          ) : (
            <>
              <Field label="Name">
                <input
                  className="m-input mw-mono-input"
                  value={adopt.name}
                  onChange={(e) => setAdopt({ ...adopt, name: e.target.value })}
                />
              </Field>
              <div className="mw-two">
                <Field label="Kind">
                  <KindSelect value={adopt.kind} onChange={(v) => setAdopt({ ...adopt, kind: v })} />
                </Field>
                <Field label="Use for">
                  <input
                    className="m-input"
                    placeholder="JPA entity"
                    value={adopt.useFor}
                    onChange={(e) => setAdopt({ ...adopt, useFor: e.target.value })}
                  />
                </Field>
              </div>
              {error && <span className="mw-error">{error}</span>}
              <div style={{ display: "flex", gap: 8 }}>
                <button type="button" className="m-btn m-btn-quiet" onClick={() => setAdopt(null)}>
                  Cancel
                </button>
                <button
                  type="button"
                  className="m-btn m-btn-primary"
                  style={{ flex: 1 }}
                  disabled={busy || !adopt.name.trim()}
                  onClick={() =>
                    void run(async () => {
                      const doc2 = await api.adoptSkill(ws, project, adopt.name.trim(), {
                        from: skill.path,
                        kind: adopt.kind,
                        component_type: adopt.useFor.trim() || null,
                      });
                      onSaved(project, doc2);
                    })
                  }
                >
                  {busy ? "Adopting…" : "Adopt"}
                </button>
              </div>
            </>
          )}
        </div>
        {doc.error ? (
          <Failed error={doc.error} onRetry={doc.reload} />
        ) : doc.data ? (
          <pre className="mw-pre">{doc.data.content}</pre>
        ) : (
          <Loading>Reading SKILL.md…</Loading>
        )}
      </div>
    );

  return (
    <div ref={top} className="m-page" style={{ gap: 16 }}>
      {!creating && <span className="mw-title">{skill.name}</span>}
      {blocked && (
        <Banner tone="warn" style={{ margin: 0 }}>
          {blocked}
        </Banner>
      )}
      {skill && !skill.exists && (
        <div className="mw-note mw-note--bad">
          <Icon name="file-x" size={15} style={{ color: "var(--bad)", flex: "none" }} />
          <span>project.toml lists this skill but its SKILL.md is missing. Saving writes a new one.</span>
        </div>
      )}
      {skill?.exists && !skill.entry && (
        <div className="mw-note mw-note--warn">
          <Icon name="triangle-alert" size={15} style={{ color: "var(--warn)", flex: "none" }} />
          <span>This folder is not listed in project.toml, so no agent loads it. Saving registers it.</span>
        </div>
      )}
      <Field label="Project">
        <select
          className="m-input mw-mono-input"
          value={project}
          disabled={!creating}
          onChange={(e) => setProject(e.target.value)}
        >
          {(creating ? projects : [project]).map((p) => (
            <option key={p} value={p}>
              {p}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Name">
        <input
          className="m-input mw-mono-input"
          value={name}
          readOnly={!creating}
          placeholder="order-event"
          onChange={(e) => {
            const next = e.target.value;
            if (text === TEMPLATE(name || "new-skill")) setContent(TEMPLATE(next || "new-skill"));
            setName(next);
          }}
        />
      </Field>
      <div className="mw-two">
        <Field label="Kind">
          <KindSelect value={kind} onChange={setKind} />
        </Field>
        <Field label="Use for">
          <input
            className="m-input"
            placeholder="axum route handler"
            value={useFor}
            onChange={(e) => setUseFor(e.target.value)}
          />
        </Field>
      </div>
      <Field
        label={
          <span style={{ display: "flex", gap: 6, minWidth: 0 }}>
            SKILL.md{" "}
            <span className="mw-desc mw-mono" style={{ fontSize: 12 }}>
              {path}
            </span>
          </span>
        }
      >
        {doc.loading && content === null && skill?.exists ? (
          <Loading>Reading SKILL.md…</Loading>
        ) : doc.error ? (
          <Failed error={doc.error} onRetry={doc.reload} />
        ) : (
          <textarea
            className="m-input mw-code"
            rows={14}
            spellCheck={false}
            value={text}
            onChange={(e) => {
              setContent(e.target.value);
              setSaved(null);
            }}
          />
        )}
      </Field>
      {saved && (
        <span style={{ display: "flex", gap: 6, alignItems: "center", fontSize: 12.5, color: "var(--ok)" }}>
          <Icon name="check" size={14} /> {saved}
        </span>
      )}
      {error && <span className="mw-error">{error}</span>}
      <div style={{ display: "flex", gap: 8 }}>
        {!creating && (
          <button
            type="button"
            className="m-btn mw-danger"
            disabled={busy || !!blocked}
            onClick={() => {
              if (!confirmDelete) return setConfirmDelete(true);
              void run(async () => {
                await api.deleteSkill(ws, project, skill.name);
                onDeleted?.();
                close(`ws:skills`);
              });
            }}
          >
            {confirmDelete ? "Tap again to delete" : "Delete"}
          </button>
        )}
        <button
          type="button"
          className="m-btn m-btn-primary"
          style={{ flex: 1 }}
          disabled={busy || !!blocked || !name.trim() || !text.trim()}
          onClick={() => void save()}
        >
          {busy ? "Saving…" : saveLabel}
        </button>
      </div>
      {confirmDelete && !creating && (
        <span className="m-help" style={{ marginTop: -8 }}>
          Deleting removes the entry from project.toml and the folder {skill.path.replace(/\/SKILL\.md$/, "/")} with
          every file in it.
        </span>
      )}
    </div>
  );
}

function KindSelect({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return (
    <select className="m-input mw-mono-input" value={value} onChange={(e) => onChange(e.target.value)}>
      {KINDS.map((k) => (
        <option key={k} value={k}>
          {k}
        </option>
      ))}
    </select>
  );
}

// ---- Memory

const SEARCH_DELAY_MS = 250;

type Editing = { id: number | null; area: string; lesson: string };

/** Lessons per project: tap one to edit or delete it. A `#lesson:<key>:<id>` anchor opens that lesson. */
function MMemory({ ws }: { ws: string }) {
  const { detail } = useWorkspace();
  const { addProject } = useShell();
  const { anchor, nonce } = useAnchor();
  const projects = detail?.projects ?? [];
  const target = parseLessonAnchor(anchor);
  const [picked, setPicked] = useState<string | null>(target?.project ?? null);
  const project = picked && projects.some((p) => p.key === picked) ? picked : (projects[0]?.key ?? null);
  const [query, setQuery] = useState("");
  const [debounced, setDebounced] = useState("");
  const [openId, setOpenId] = useState<number | null>(target?.id ?? null);
  const [editing, setEditing] = useState<Editing | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const t = setTimeout(() => setDebounced(query.trim()), SEARCH_DELAY_MS);
    return () => clearTimeout(t);
  }, [query]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: `nonce` re-applies the same anchor.
  useEffect(() => {
    const t = parseLessonAnchor(anchor);
    if (!t) return;
    setPicked(t.project);
    setOpenId(t.id);
    setQuery("");
    setDebounced("");
  }, [anchor, nonce]);

  const lessons = useAsync(
    () => (project ? api.lessons(ws, project, debounced || undefined) : Promise.resolve([] as Lesson[])),
    [ws, project, debounced],
  );
  const rows = lessons.data ?? [];

  const run = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
      setEditing(null);
      setConfirmDelete(null);
      lessons.reload();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const form = (e: Editing) => (
    <div className="m-card" style={{ gap: 10 }}>
      <Field label="Area">
        <input
          className="m-input mw-mono-input"
          placeholder="orders/cancel"
          value={e.area}
          onChange={(ev) => setEditing({ ...e, area: ev.target.value })}
        />
      </Field>
      <Field label="Lesson">
        <textarea
          className="m-input"
          rows={4}
          placeholder="What an agent must know before working here, and why."
          value={e.lesson}
          onChange={(ev) => setEditing({ ...e, lesson: ev.target.value })}
        />
      </Field>
      {error && <span className="mw-error">{error}</span>}
      <div style={{ display: "flex", gap: 8 }}>
        <button
          type="button"
          className="m-btn m-btn-quiet"
          onClick={() => {
            setEditing(null);
            setError(null);
          }}
        >
          Cancel
        </button>
        <button
          type="button"
          className="m-btn m-btn-primary"
          style={{ flex: 1 }}
          disabled={busy || !e.area.trim() || !e.lesson.trim() || !project}
          onClick={() =>
            project &&
            void run(async () => {
              const saved = await api.saveLesson(ws, project, {
                id: e.id,
                area: e.area.trim(),
                lesson: e.lesson.trim(),
              });
              setOpenId(saved.id);
            })
          }
        >
          {busy ? "Saving…" : e.id === null ? "Add the lesson" : "Save"}
        </button>
      </div>
    </div>
  );

  if (detail && projects.length === 0)
    return (
      <div className="m-page">
        <Banner tone="info" style={{ margin: 0 }}>
          This workspace has no projects yet. Lessons belong to a project, so add one first.{" "}
          <button type="button" className="mw-link" onClick={addProject}>
            Add project
          </button>
        </Banner>
      </div>
    );

  return (
    <div className="m-page">
      <span className="m-help" style={{ fontSize: 12.5 }}>
        Agents record lessons when something cost real effort to find out, and recall them before working in an area.
        Edit or delete any lesson that is wrong or stale, because agents act on what they recall.
      </span>
      {projects.length > 1 && (
        <div className="mw-chips" style={{ marginTop: -8 }}>
          {projects.map((p) => (
            <button
              type="button"
              key={p.key}
              aria-pressed={project === p.key}
              className="mw-chip mw-mono"
              onClick={() => {
                setPicked(p.key);
                setOpenId(null);
                setEditing(null);
              }}
            >
              {p.key}
            </button>
          ))}
        </div>
      )}
      <div style={{ display: "flex", gap: 8 }}>
        <input
          className="m-input"
          type="search"
          aria-label="Search lessons"
          placeholder="Search lessons"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <button
          type="button"
          className="m-btn m-btn-primary"
          style={{ flex: "none" }}
          disabled={!project}
          onClick={() => setEditing({ id: null, area: "", lesson: "" })}
        >
          <Icon name="plus" size={15} /> Add
        </button>
      </div>
      {editing?.id === null && form(editing)}
      {lessons.error && <Failed error={lessons.error} onRetry={lessons.reload} />}
      <section style={{ display: "flex", flexDirection: "column" }}>
        {lessons.loading && !lessons.data && <Loading>Reading the lessons…</Loading>}
        {rows.map((l) =>
          editing?.id === l.id ? (
            <div key={l.id} style={{ padding: "8px 0" }}>
              {form(editing)}
            </div>
          ) : (
            <div key={l.id} className="mw-lesson" data-open={openId === l.id || undefined}>
              <button
                type="button"
                className="mw-lesson__head"
                aria-expanded={openId === l.id}
                onClick={() => {
                  setOpenId(openId === l.id ? null : l.id);
                  setConfirmDelete(null);
                }}
              >
                <span className="mw-mono" style={{ fontSize: 12, color: "var(--text-muted)" }}>
                  {l.area}
                </span>
                <span className={openId === l.id ? "mw-lesson__text" : "mw-lesson__text mw-clamp"}>{l.lesson}</span>
                <span className="mw-mono" style={{ fontSize: 11, color: "var(--text-muted)" }}>
                  {l.source} · {formatTime(l.created_at)}
                </span>
              </button>
              {openId === l.id && (
                <div style={{ display: "flex", gap: 8, paddingBottom: 12 }}>
                  <button
                    type="button"
                    className="m-btn m-btn-sm"
                    onClick={() => setEditing({ id: l.id, area: l.area, lesson: l.lesson })}
                  >
                    <Icon name="pencil" size={13} /> Edit
                  </button>
                  <button
                    type="button"
                    className="m-btn m-btn-sm mw-danger"
                    disabled={busy}
                    onClick={() => {
                      if (confirmDelete !== l.id) return setConfirmDelete(l.id);
                      if (project) void run(() => api.deleteLesson(ws, project, l.id));
                    }}
                  >
                    {confirmDelete === l.id ? "Tap again to delete" : "Delete"}
                  </button>
                </div>
              )}
            </div>
          ),
        )}
        {!lessons.loading && rows.length === 0 && (
          <span className="m-muted" style={{ padding: "12px 0" }}>
            {debounced
              ? `No lessons match “${debounced}”.`
              : `No lessons recorded for ${project} yet. Agents add them as they work, or add one yourself.`}
          </span>
        )}
        {error && !editing && <span className="mw-error">{error}</span>}
      </section>
    </div>
  );
}
