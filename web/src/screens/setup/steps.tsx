import {
  Banner,
  Button,
  Checkbox,
  Chip,
  CodeView,
  FolderPicker,
  Icon,
  IconButton,
  type IconName,
  Input,
  LANE_ORDER,
  type LaneState,
  LaneStepper,
  Panel,
  Spinner,
  StatusChip,
  Switch,
  Tabs,
} from "@ostra/design";
import { type ReactNode, useEffect, useState } from "react";
import type { CloneProject, ValidationIssue, WorkspaceDetail } from "../../api/types";
import { useChannel } from "../../lib/hooks";
import { CloneForm } from "./CloneForm";
import { listFolders, makeFolder, useFolderInfo } from "./folders";
import { useGitCredentials } from "./GitCredentials";
import { HarnessAction, SetupTerminalPanel, useHarnessSetup } from "./HarnessSetup";
import { ImportForm } from "./ImportForm";
import { ProviderCredentials } from "./ProviderCredentials";
import type { Wizard } from "./useWizard";
import {
  clonePath,
  expandHome,
  HARNESS_LABEL,
  keySource,
  PRESET_LABEL,
  PROVIDER_LABEL,
  patchName,
  presetExecutor,
  presetsFor,
  projectIndex,
  projectStart,
  stepForIssue,
  tomlFor,
  type WizardValues,
} from "./wizard";

export function useReducedMotion(): boolean {
  const q =
    typeof window !== "undefined" && window.matchMedia ? window.matchMedia("(prefers-reduced-motion: reduce)") : null;
  const [reduced, setReduced] = useState(!!q?.matches);
  useEffect(() => {
    if (!q) return;
    const on = () => setReduced(q.matches);
    q.addEventListener?.("change", on);
    return () => q.removeEventListener?.("change", on);
  }, [q]);
  return reduced;
}

function StepHead({ title, text }: { title: string; text?: ReactNode }) {
  return (
    <div>
      <h2 style={{ margin: "0 0 6px", font: "var(--type-title)" }}>{title}</h2>
      {text && <div style={{ color: "var(--text-secondary)", lineHeight: 1.55 }}>{text}</div>}
    </div>
  );
}

function FieldErrors({ issues }: { issues?: ValidationIssue[] }) {
  if (!issues?.length) return null;
  return (
    <>
      {issues.map((i) => (
        <span key={i.path + i.message} className="os-field__error" role="alert">
          {i.message}
        </span>
      ))}
    </>
  );
}

const FACTS: [IconName, string, string][] = [
  [
    "git-pull-request",
    "Work goes through a pipeline",
    "Research, spec, fact-check, plan, build, review, test and docs. Each stage says what it produces and why it exists.",
  ],
  [
    "hand",
    "You approve at gates",
    "Ostra stops for your answer before a spec or plan is used, and before any command no rule allows.",
  ],
  [
    "square-terminal",
    "Agents run where you choose",
    "On Ostra's own loop with an API key, or inside Claude Code, Codex, Grok Build or Antigravity.",
  ],
];

function StepWelcome() {
  const reduced = useReducedMotion();
  const [n, setN] = useState(reduced ? 4 : 0);
  useEffect(() => {
    if (reduced) return;
    const t = setInterval(() => setN((x) => (x + 1) % (LANE_ORDER.length + 3)), 700);
    return () => clearInterval(t);
  }, [reduced]);
  const lanes = Object.fromEntries(
    LANE_ORDER.map((id, i) => [id, { status: i < n ? "done" : i === n ? "current" : "pending" } satisfies LaneState]),
  );
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 22 }}>
      <div>
        <h1
          style={{
            margin: "0 0 8px",
            font: "var(--weight-semibold) var(--text-2xl)/1.2 var(--font-sans)",
            letterSpacing: "var(--tracking-tight)",
          }}
        >
          Set up Ostra
        </h1>
        <div style={{ fontSize: "var(--text-md)", color: "var(--text-secondary)", lineHeight: 1.55 }}>
          Ostra runs a software development lifecycle on your own machine. You describe a change; code drives it from
          stage to stage and models do the work inside each stage.
        </div>
      </div>
      <LaneStepper lanes={lanes} />
      <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
        {FACTS.map(([ic, t, d], i) => (
          <div key={t} className="os-rise" style={{ display: "flex", gap: 12, animationDelay: `${120 + i * 90}ms` }}>
            <span
              style={{
                width: 30,
                height: 30,
                flex: "none",
                display: "grid",
                placeItems: "center",
                borderRadius: 6,
                background: "var(--surface-raised)",
                border: "1px solid var(--border-default)",
                color: "var(--accent-fg)",
              }}
            >
              <Icon name={ic} size={15} />
            </span>
            <div>
              <div style={{ fontWeight: 500 }}>{t}</div>
              <div style={{ fontSize: "var(--text-sm)", color: "var(--text-secondary)", lineHeight: 1.5 }}>{d}</div>
            </div>
          </div>
        ))}
      </div>
      <div style={{ fontSize: "var(--text-sm)", color: "var(--text-muted)" }}>
        Setup takes about two minutes: check this machine, create a workspace, add the project folders Ostra should work
        on.
      </div>
    </div>
  );
}

type Tone = "ok" | "warn" | "neutral";

function CheckRow({
  delay,
  label,
  sub,
  result,
  tone,
  action,
}: {
  delay: number;
  label: string;
  sub: string;
  result: string;
  tone: Tone;
  action?: ReactNode;
}) {
  const [done, setDone] = useState(delay === 0);
  useEffect(() => {
    if (delay === 0) return;
    const t = setTimeout(() => setDone(true), delay);
    return () => clearTimeout(t);
  }, [delay]);
  const icon: IconName = tone === "ok" ? "circle-check" : tone === "warn" ? "circle-alert" : "circle-minus";
  const color = tone === "ok" ? "var(--ok)" : tone === "warn" ? "var(--warn)" : "var(--text-muted)";
  return (
    <div
      className="os-rise"
      style={{
        display: "flex",
        alignItems: "center",
        gap: 10,
        height: 38,
        padding: "0 12px",
        borderBottom: "1px solid var(--border-subtle)",
        animationDelay: `${delay / 4}ms`,
      }}
    >
      <span style={{ width: 16, display: "grid", placeItems: "center" }}>
        {done ? (
          <Icon name={icon} size={15} style={{ color }} />
        ) : (
          <Spinner size={12} style={{ color: "var(--text-muted)" }} />
        )}
      </span>
      <span style={{ flex: 1, minWidth: 0 }}>
        {label}{" "}
        <span style={{ font: "var(--text-sm)/1 var(--font-mono)", color: "var(--text-muted)", marginLeft: 4 }}>
          {sub}
        </span>
      </span>
      {done ? (
        <Chip tone={tone}>{result}</Chip>
      ) : (
        <span style={{ fontSize: "var(--text-sm)", color: "var(--text-muted)" }}>Checking…</span>
      )}
      {done && action}
    </div>
  );
}

function StepCheck({ w }: { w: Wizard }) {
  const reduced = useReducedMotion();
  const e = w.env.data;
  const setup = useHarnessSetup(e?.harnesses, w.env.reload);
  if (w.env.error)
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
        <StepHead title="Check this machine" />
        <Banner
          tone="bad"
          actions={
            <Button size="sm" onClick={w.env.reload}>
              Check again
            </Button>
          }
        >
          Ostra could not read this machine's setup: {w.env.error.message}
        </Banner>
      </div>
    );
  let n = 0;
  const d = () => (reduced ? 0 : 350 + n++ * 260);
  const anyKey = e?.providers.some((p) => p.has_key) ?? true;
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
      <StepHead
        title="Check this machine"
        text="Ostra reads provider keys from the environment, the keys saved below, or the OS keychain. Environment variables take precedence. Saved keys never come back to the browser."
      />
      {!e ? (
        <div style={{ display: "flex", gap: 8, alignItems: "center", color: "var(--text-muted)" }}>
          <Spinner size={12} /> Checking this machine…
        </div>
      ) : (
        <>
          {!anyKey && (
            <Banner tone="bad">
              No model provider has a usable API key. Save one below, or set <code>ANTHROPIC_API_KEY</code> or{" "}
              <code>OPENAI_API_KEY</code> in the environment that starts <code>ostra</code> and restart it.
            </Banner>
          )}
          <div>
            <div className="os-section-label" style={{ marginBottom: 6 }}>
              Model providers
            </div>
            <div className="os-panel">
              {e.providers.map((p) => (
                <CheckRow
                  key={p.name}
                  delay={d()}
                  label={PROVIDER_LABEL[p.name] ?? p.name}
                  sub={keySource(p.source)}
                  result={p.has_key ? "Usable" : "Missing"}
                  tone={p.has_key ? "ok" : "warn"}
                />
              ))}
            </div>
          </div>
          <div>
            <div className="os-section-label" style={{ marginBottom: 6 }}>
              Provider keys and base URLs
            </div>
            <ProviderCredentials providers={e.providers} onSaved={w.env.reload} />
          </div>
          <div>
            <div className="os-section-label" style={{ marginBottom: 6 }}>
              Harness CLIs
            </div>
            <div className="os-panel">
              {e.harnesses.map((h) => (
                <CheckRow
                  key={h.harness}
                  delay={d()}
                  label={HARNESS_LABEL[h.harness] ?? h.harness}
                  sub={h.command + (h.version ? ` ${h.version}` : "")}
                  result={
                    !h.installed
                      ? "Not installed"
                      : h.logged_in === true
                        ? "Logged in"
                        : h.logged_in === false
                          ? "Not logged in"
                          : "Installed"
                  }
                  tone={!h.installed ? "neutral" : h.logged_in === false ? "warn" : "ok"}
                  action={
                    <HarnessAction
                      h={h}
                      starting={setup.starting === h.harness}
                      onStart={(a) => setup.start(h.harness, a)}
                    />
                  }
                />
              ))}
            </div>
          </div>
          <div>
            <div className="os-section-label" style={{ marginBottom: 6 }}>
              Sandbox
            </div>
            <div className="os-panel">
              <CheckRow
                delay={d()}
                label="Agent command sandbox"
                sub={
                  e.sandbox.message ??
                  [`${e.sandbox.backend ?? "sandbox"}, mode ${e.sandbox.mode}`, e.sandbox.gaps]
                    .filter(Boolean)
                    .join(". ")
                }
                result={e.sandbox.active ? "On" : "Off"}
                tone={e.sandbox.active ? "ok" : "warn"}
              />
              <CheckRow
                delay={d()}
                label="Shell for the Bash tool"
                sub={e.shell.message ?? e.shell.path ?? ""}
                result={e.shell.available ? "Found" : "Missing"}
                tone={e.shell.available ? "ok" : "warn"}
              />
            </div>
          </div>
          <SetupTerminalPanel run={setup.run} error={setup.error} onClose={setup.close} onCheck={w.env.reload} />
        </>
      )}
      <div style={{ fontSize: "var(--text-sm)", color: "var(--text-muted)", lineHeight: 1.5 }}>
        Agents run on the native executor unless workspace settings route them to an installed harness. Install runs the
        vendor's official installer script, and Log in runs the CLI's own login, both in a terminal here. A harness that
        is not logged in can also log in later from an execution's Terminal tab.
      </div>
    </div>
  );
}

function StepName({ w }: { w: Wizard }) {
  const { v, set } = w;
  const info = useFolderInfo(v.root);
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
      <StepHead
        title="Name the workspace"
        text="A workspace holds your projects and their settings: which executor and model each agent runs on, permissions, memory, and custom instructions."
      />
      <div className="os-field">
        <Input
          label="Name"
          value={v.name}
          onChange={(e) => set({ name: e.target.value, nameTouched: true })}
          placeholder="shop"
          autoFocus
        />
        <FieldErrors issues={w.byStep.name?.filter((i) => i.path === "name")} />
      </div>
      <div className="os-field">
        <span className="os-field__label">Directory</span>
        <span className="os-field__hint">
          Ostra writes <code>.ostra/</code> here: workspace settings, the session database, and session artifacts.
          Projects are referenced by path and never copied.
        </span>
      </div>
      <div className="os-field">
        <FolderPicker
          value={v.root}
          onChange={(root) => set(patchName(v, root))}
          list={listFolders}
          mkdir={makeFolder}
          height={180}
        />
        {info.state === "missing" && (
          <span className="os-field__hint">
            No folder at <code>{v.root}</code> yet. Ostra creates it when it creates the workspace.
          </span>
        )}
        {info.state === "exists" && info.isOstraProject && (
          <span className="os-field__hint">
            This folder is an Ostra project. A workspace usually lives next to its projects, not inside one.
          </span>
        )}
        <FieldErrors issues={w.byStep.name?.filter((i) => i.path === "root")} />
      </div>
    </div>
  );
}

function StepProjects({ w }: { w: Wizard }) {
  const { v, set } = w;
  const [adding, setAdding] = useState(v.projects.length === 0 && v.clones.length === 0);
  const [source, setSource] = useState<"folder" | "git">("folder");
  const [clone, setClone] = useState<CloneProject | null>(null);
  // Remounts the clone form after each add, so it starts empty.
  const [cloneForm, setCloneForm] = useState(0);
  const creds = useGitCredentials();
  const issues = w.byStep.projects ?? [];
  const takenPaths = Object.fromEntries(v.projects.map((p) => [p.path, p.key]));
  const takenKeys = [...v.projects.map((p) => p.key), ...v.clones.map((c) => c.key)];
  const any = v.projects.length + v.clones.length > 0;
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
      <StepHead
        title="Add projects"
        text="Import folders on this machine, or clone git repositories into the workspace. Clones run after the workspace is created. You can skip this and add projects later."
      />
      {any && (
        <div className="os-panel">
          {v.projects.map((p, i) => {
            const mine = issues.filter((x) => projectIndex(x.path) === i);
            return (
              <div
                key={p.key}
                className="os-rise"
                style={{
                  borderBottom: i < v.projects.length - 1 || v.clones.length ? "1px solid var(--border-subtle)" : 0,
                }}
              >
                <div style={{ display: "flex", alignItems: "center", gap: 10, height: 40, padding: "0 8px 0 12px" }}>
                  <Icon name="folder-git-2" size={15} style={{ color: "var(--accent-fg)" }} />
                  <span style={{ fontFamily: "var(--font-mono)", fontWeight: 600 }}>{p.key}</span>
                  <span
                    style={{
                      flex: 1,
                      minWidth: 0,
                      font: "var(--text-sm)/1 var(--font-mono)",
                      color: "var(--text-muted)",
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                    }}
                    title={p.path}
                  >
                    {p.path}
                  </span>
                  <Chip>{p.stack || "detect"}</Chip>
                  <StatusChip kind="init" status={p.isOstraProject ? "initialized" : "not_initialized"} />
                  <IconButton
                    size="sm"
                    icon="x"
                    label={`Remove ${p.key}`}
                    onClick={() => set({ projects: v.projects.filter((x) => x.key !== p.key) })}
                  />
                </div>
                {mine.length > 0 && (
                  <div style={{ display: "flex", flexDirection: "column", gap: 2, padding: "0 12px 8px 37px" }}>
                    <FieldErrors issues={mine} />
                  </div>
                )}
              </div>
            );
          })}
          {v.clones.map((c, i) => (
            <div
              key={c.key}
              className="os-rise"
              style={{
                display: "flex",
                alignItems: "center",
                gap: 10,
                height: 40,
                padding: "0 8px 0 12px",
                borderBottom: i < v.clones.length - 1 ? "1px solid var(--border-subtle)" : 0,
              }}
            >
              <Icon name="git-fork" size={15} style={{ color: "var(--accent-fg)" }} />
              <span style={{ fontFamily: "var(--font-mono)", fontWeight: 600 }}>{c.key}</span>
              <span
                style={{
                  flex: 1,
                  minWidth: 0,
                  font: "var(--text-sm)/1 var(--font-mono)",
                  color: "var(--text-muted)",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
                title={`${c.url} → ${clonePath(c, v.root, w.home)}`}
              >
                {c.url}
              </span>
              <Chip>{c.stack || "detect"}</Chip>
              <Chip tone="info">clone</Chip>
              <IconButton
                size="sm"
                icon="x"
                label={`Remove ${c.key}`}
                onClick={() => set({ clones: v.clones.filter((x) => x.key !== c.key) })}
              />
            </div>
          ))}
        </div>
      )}
      {adding ? (
        <Panel
          title={source === "folder" ? "Import a folder" : "Clone from git"}
          icon={source === "folder" ? "folder-plus" : "git-fork"}
          actions={
            any && (
              <Button size="sm" variant="ghost" onClick={() => setAdding(false)}>
                Done
              </Button>
            )
          }
        >
          <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
            <Tabs
              label="Project source"
              value={source}
              onChange={(id) => setSource(id as "folder" | "git")}
              tabs={[
                { id: "folder", label: "Folder on this machine", icon: "folder-plus" },
                { id: "git", label: "Clone from git", icon: "git-fork" },
              ]}
            />
            {source === "folder" ? (
              <ImportForm
                takenKeys={takenKeys}
                takenPaths={takenPaths}
                stacks={w.env.data?.stacks ?? []}
                initialPath={projectStart(v.root)}
                onAdd={(p) => set({ projects: [...v.projects, p] })}
              />
            ) : (
              <>
                <CloneForm
                  key={cloneForm}
                  takenKeys={takenKeys}
                  stacks={w.env.data?.stacks ?? []}
                  root={expandHome(v.root.trim(), w.home)}
                  credentials={creds.list}
                  serverErrors={{}}
                  onDraft={setClone}
                />
                <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
                  <span className="os-field__hint" style={{ flex: 1 }}>
                    Save tokens and SSH keys under Settings, Git, or let the machine's own git and SSH setup answer. The
                    clone runs when you create the workspace.
                  </span>
                  <Button
                    icon="plus"
                    disabled={!clone}
                    onClick={() => {
                      if (!clone) return;
                      set({ clones: [...v.clones, clone] });
                      setClone(null);
                      setCloneForm((n) => n + 1);
                    }}
                  >
                    Add to the list
                  </Button>
                </div>
              </>
            )}
          </div>
        </Panel>
      ) : (
        <div>
          <Button icon="plus" onClick={() => setAdding(true)}>
            Add another project
          </Button>
        </div>
      )}
    </div>
  );
}

const MODES: [WizardValues["mode"], string, string][] = [
  ["default", "Default", "Asks before file edits and before any command no rule allows."],
  ["acceptEdits", "Accept edits", "File edits inside the project are allowed; unlisted commands still ask."],
  ["plan", "Plan (read-only)", "Agents may read but not change the project."],
];

function StepDefaults({ w }: { w: Wizard }) {
  const { v, set } = w;
  const presets = presetsFor(w.env.data);
  const issues = w.byStep.defaults ?? [];
  useEffect(() => {
    if (w.env.data && !presets.includes(v.preset)) set({ preset: "native" });
  }, [w.env.data, presets, v.preset, set]);
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 18 }}>
      <StepHead
        title="Choose defaults"
        text="Everything here is saved in .ostra/workspace.toml and can change later under Settings. Changes apply to the next execution."
      />
      <div className="os-field">
        <span className="os-field__label">Permission mode</span>
        <div style={{ display: "flex", flexDirection: "column", gap: 10, marginTop: 4 }}>
          {MODES.map(([k, l, d]) => (
            <Checkbox
              key={k}
              radio
              name="wizard-mode"
              checked={v.mode === k}
              onChange={() => set({ mode: k })}
              label={l}
              description={d}
            />
          ))}
        </div>
        <FieldErrors issues={issues.filter((i) => i.path.startsWith("permissions"))} />
      </div>
      <div className="os-field">
        <span className="os-field__label">Where implementers run</span>
        <div style={{ marginTop: 4 }}>
          <Tabs
            variant="segmented"
            label="Where implementers run"
            value={v.preset}
            onChange={(preset) => set({ preset: preset as WizardValues["preset"] })}
            tabs={presets.map((p) => ({ id: p, label: PRESET_LABEL[p] }))}
          />
        </div>
        <span className="os-field__hint">
          Only logged-in harnesses are offered. Research, spec, fact-check and review stay on the native loop.
        </span>
        <FieldErrors issues={issues.filter((i) => i.path.startsWith("routing"))} />
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
        <Switch label="YOLO by default" tone="warn" checked={v.yolo} onChange={() => set({ yolo: !v.yolo })} />
        {v.yolo && (
          <Banner tone="warn">
            Ostra answers every gate and permission ask itself and lists each decision at the end. Guards, deny rules,
            the fact-check PASS requirement, and security blocks still apply.
          </Banner>
        )}
        <Switch
          label="Browser notifications when a gate is waiting"
          checked={v.push}
          onChange={() => set({ push: !v.push })}
        />
        <FieldErrors issues={issues.filter((i) => i.path.startsWith("yolo") || i.path.startsWith("notifications"))} />
      </div>
    </div>
  );
}

function Checklist({ w }: { w: Wizard }) {
  const [line, setLine] = useState<{ key: string; text: string } | null>(null);
  useChannel(w.cloning && w.created ? `workspace:${w.created.id}` : null, (m) => {
    if (m.type === "git_progress") setLine({ key: m.key, text: m.line });
  });
  const failed = Object.keys(w.cloneFailures);
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
      <StepHead title={w.done ? "Workspace ready" : "Creating the workspace"} />
      <div className="os-panel">
        {w.tasks.map((t, i) => {
          const n = w.progress;
          const mono = t.includes("/");
          const key = t.startsWith("Clone ") ? t.slice(6) : null;
          const failure = key ? w.cloneFailures[key] : undefined;
          const note = failure ?? (key && i === n && line?.key === key ? line.text : null);
          return (
            <div
              key={t}
              style={{
                display: "flex",
                alignItems: "center",
                flexWrap: "wrap",
                columnGap: 10,
                minHeight: 36,
                padding: "0 12px",
                borderBottom: i < w.tasks.length - 1 ? "1px solid var(--border-subtle)" : 0,
                color: i <= n ? "var(--text-primary)" : "var(--text-muted)",
              }}
            >
              <span style={{ width: 16, display: "grid", placeItems: "center" }}>
                {failure ? (
                  <Icon name="circle-x" size={15} style={{ color: "var(--bad)" }} />
                ) : i < n ? (
                  <Icon name="circle-check" size={15} style={{ color: "var(--ok)" }} />
                ) : i === n ? (
                  <Spinner size={12} style={{ color: "var(--accent)" }} />
                ) : (
                  <span className="os-dot os-dot--hollow" />
                )}
              </span>
              <span
                style={{
                  fontFamily: mono ? "var(--font-mono)" : undefined,
                  fontSize: mono ? "var(--text-sm)" : undefined,
                }}
              >
                {t}
              </span>
              {note && (
                <span
                  style={{
                    flexBasis: "100%",
                    padding: "0 0 8px 26px",
                    font: "var(--text-sm)/1.4 var(--font-mono)",
                    color: failure ? "var(--bad)" : "var(--text-muted)",
                    overflowWrap: "anywhere",
                  }}
                >
                  {note}
                </span>
              )}
            </div>
          );
        })}
      </div>
      {w.done && failed.length > 0 && (
        <Banner
          tone="warn"
          title={failed.length === 1 ? `${failed[0]} was not cloned` : `${failed.length} repositories were not cloned`}
        >
          The workspace was created. Fix the problem above, then clone again from Add project in the workspace.
        </Banner>
      )}
    </div>
  );
}

const MONO_ROWS = new Set(["Directory", "Projects", "Implementers"]);

function StepReview({ w }: { w: Wizard }) {
  const { v } = w;
  if (w.creating || w.created) return <Checklist w={w} />;
  const rows: [string, string][] = [
    ["Name", v.name],
    ["Directory", w.home && v.root.startsWith("~/") ? w.home + v.root.slice(1) : v.root],
    [
      "Projects",
      v.projects.length || v.clones.length
        ? [...v.projects.map((p) => p.key), ...v.clones.map((c) => `${c.key} (clone)`)].join(", ")
        : "none yet",
    ],
    ["Permission mode", v.mode],
    ["Implementers", presetExecutor(v.preset)],
    ["YOLO", v.yolo ? "on" : "off"],
    ["Notifications", v.push ? "on" : "off"],
  ];
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
      <StepHead
        title="Review and create"
        text="Ostra writes this file and registers the workspace. Nothing in your project folders changes until you initialize a project."
      />
      <ReviewIssues w={w} />
      <div style={{ display: "grid", gridTemplateColumns: "minmax(0,1fr) minmax(0,1.2fr)", gap: 16 }}>
        <div className="os-panel" style={{ padding: "6px 0" }}>
          {rows.map(([k, val]) => (
            <div
              key={k}
              style={{
                display: "grid",
                gridTemplateColumns: "120px 1fr",
                gap: 10,
                padding: "6px 12px",
                fontSize: "var(--text-base)",
              }}
            >
              <span style={{ color: "var(--text-muted)" }}>{k}</span>
              <span
                style={{
                  fontFamily: MONO_ROWS.has(k) ? "var(--font-mono)" : undefined,
                  fontSize: MONO_ROWS.has(k) ? "var(--text-sm)" : undefined,
                  wordBreak: "break-all",
                }}
              >
                {val}
              </span>
            </div>
          ))}
        </div>
        <CodeView language="toml" code={tomlFor(v, w.home)} maxHeight={260} />
      </div>
    </div>
  );
}

function ReviewIssues({ w }: { w: Wizard }) {
  if (w.createError)
    return (
      <Banner tone="bad" title="Ostra could not create the workspace">
        {w.createError}
      </Banner>
    );
  if (w.validating)
    return (
      <div
        style={{
          display: "flex",
          gap: 8,
          alignItems: "center",
          color: "var(--text-muted)",
          fontSize: "var(--text-sm)",
          height: 20,
        }}
      >
        <Spinner size={11} /> Checking the settings…
      </div>
    );
  if (w.issues.length === 0)
    return (
      <div
        style={{
          display: "flex",
          gap: 8,
          alignItems: "center",
          color: "var(--text-secondary)",
          fontSize: "var(--text-sm)",
          height: 20,
        }}
      >
        <Icon name="circle-check" size={14} style={{ color: "var(--ok)" }} /> The settings pass validation.
      </div>
    );
  return (
    <Banner
      tone="bad"
      title={
        w.issues.length === 1
          ? "Fix this before creating the workspace"
          : `Fix these ${w.issues.length} problems before creating the workspace`
      }
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 6, marginTop: 4 }}>
        {w.issues.map((i) => {
          const step = stepForIssue(i.path);
          return (
            <div key={i.path + i.message} style={{ display: "flex", alignItems: "baseline", gap: 8, flexWrap: "wrap" }}>
              <code>{i.path}</code>
              <span style={{ flex: 1, minWidth: 200 }}>{i.message}</span>
              {step !== "review" && w.steps.some((s) => s.id === step) && (
                <Button size="sm" variant="ghost" iconRight="arrow-right" onClick={() => w.goTo(step)}>
                  Go to {w.steps.find((s) => s.id === step)?.label}
                </Button>
              )}
            </div>
          );
        })}
      </div>
    </Banner>
  );
}

/** The body of the current step. Keyed by step so each change replays the slide. */
export function WizardBody({ w, padding }: { w: Wizard; padding: string }) {
  const body = {
    welcome: <StepWelcome />,
    check: <StepCheck w={w} />,
    name: <StepName w={w} />,
    projects: <StepProjects w={w} />,
    defaults: <StepDefaults w={w} />,
    review: <StepReview w={w} />,
  }[w.id];
  return (
    <div
      key={w.id + (w.creating ? "-c" : "")}
      className={w.dir === "back" ? "os-enter-back" : "os-enter-forward"}
      style={{ padding }}
    >
      {body}
    </div>
  );
}

export type NavProps = {
  w: Wizard;
  onCancel?: () => void;
  cancelLabel?: string;
  onOpen: (d: WorkspaceDetail) => void;
  onInit: (d: WorkspaceDetail, key: string) => void;
  initializing?: boolean;
};

/** Footer buttons: Back, Skip for now, Continue, Create workspace, then Initialize and Open workspace. */
export function WizardNav({ w, onCancel, cancelLabel = "Cancel", onOpen, onInit, initializing }: NavProps) {
  const last = w.i === w.steps.length - 1;
  const toInit = w.created?.projects.find((p) => p.init_status === "not_initialized");
  return (
    <>
      {w.i > 0 && !w.creating && !w.created && (
        <Button variant="ghost" icon="arrow-left" onClick={() => w.go(w.i - 1)}>
          Back
        </Button>
      )}
      {w.i === 0 && onCancel && (
        <Button variant="ghost" onClick={onCancel}>
          {cancelLabel}
        </Button>
      )}
      <span style={{ flex: 1 }} />
      {w.id === "projects" && w.v.projects.length === 0 && (
        <Button variant="ghost" onClick={() => w.go(w.i + 1)}>
          Skip for now
        </Button>
      )}
      {!last && (
        <Button variant="primary" iconRight="arrow-right" disabled={!w.canNext} onClick={() => w.go(w.i + 1)}>
          {w.id === "welcome" ? "Get started" : "Continue"}
        </Button>
      )}
      {last && !w.creating && !w.created && (
        <Button variant="primary" icon="check" disabled={!w.canCreate} onClick={w.create}>
          Create workspace
        </Button>
      )}
      {last && w.done && w.created && (
        <>
          {toInit && (
            <Button icon="sparkles" disabled={initializing} onClick={() => onInit(w.created!, toInit.key)}>
              {initializing ? "Starting…" : `Initialize ${toInit.key}`}
            </Button>
          )}
          <Button variant="primary" iconRight="arrow-right" onClick={() => onOpen(w.created!)}>
            Open workspace
          </Button>
        </>
      )}
    </>
  );
}
