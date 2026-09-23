import { useCallback, useEffect, useMemo, useState } from "react";
import { api, HttpError } from "../api";
import type { ValidationIssue, WorkspaceDetail } from "../api/types";
import { Banner, Button, Tabs } from "../design";
import { useShell, useWorkspace } from "../lib/nav";
import { flash, Loading, Page, useAfterPaint, useAnchor } from "./workspace/Page";
import {
  anchorCandidates,
  fieldIds,
  fromForm,
  mapIssues,
  SETTINGS_TABS,
  settingKeyOf,
  stableJson,
  tabOf,
  toForm,
  type SettingsForm,
  type SettingsTab,
} from "./workspace/settingsForm";
import {
  GeneralSection,
  InstructionsSection,
  NotificationsSection,
  PermissionsSection,
  ProjectsSection,
  RoutingSection,
  type SectionProps,
} from "./workspace/SettingsSections";

export type SettingsScreenProps = { ws: string };

const VALIDATE_DELAY_MS = 400;

/**
 * Resource `ws:settings`: workspace settings as forms, validated as you edit and again on save. A
 * `#setting:<dotted key>` anchor (from ⌘K) opens that field's tab and rings the field.
 */
export function SettingsScreen({ ws }: SettingsScreenProps) {
  const { detail, reload } = useWorkspace();
  if (!detail) {
    return (
      <Page title="Settings">
        <Loading>Reading the settings…</Loading>
      </Page>
    );
  }
  return <SettingsEditor ws={ws} detail={detail} onSaved={reload} />;
}

const same = (a: unknown, b: unknown) => stableJson(a) === stableJson(b);

function dedupe(list: ValidationIssue[]): ValidationIssue[] {
  const seen = new Set<string>();
  return list.filter((i) => {
    const k = `${i.path}\n${i.message}`;
    return !seen.has(k) && (seen.add(k), true);
  });
}

function SettingsEditor({ ws, detail, onSaved }: { ws: string; detail: WorkspaceDetail; onSaved: () => void }) {
  const { addProject } = useShell();
  // `base` is the saved settings the form started from, with the server's issues for them.
  const [base, setBase] = useState(detail.settings);
  const [baseIssues, setBaseIssues] = useState<ValidationIssue[]>(detail.validation);
  const [form, setForm] = useState<SettingsForm>(() => toForm(detail.settings));
  const [tab, setTab] = useState<SettingsTab>("general");
  const [serverIssues, setServerIssues] = useState<ValidationIssue[]>(detail.validation);
  const [checking, setChecking] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [target, setTarget] = useState<{ key: string; n: number } | null>(null);

  const derived = useMemo(() => fromForm(form, base), [form, base]);
  const dirty = derived.issues.length > 0 || !same(derived.settings, base);
  const issues = useMemo(() => dedupe([...derived.issues, ...serverIssues]), [derived.issues, serverIssues]);
  const map = useMemo(() => mapIssues(issues, fieldIds(form)), [issues, form]);
  const issuesFor = useCallback((field: string) => map.byField[field] ?? [], [map]);

  const adopt = useCallback((d: WorkspaceDetail) => {
    setBase(d.settings);
    setBaseIssues(d.validation);
    setServerIssues(d.validation);
    setForm(toForm(d.settings));
  }, []);

  // The file changed elsewhere (another tab, an editor). Follow it unless there are edits to keep.
  const detailKey = stableJson(detail.settings);
  const [seenKey, setSeenKey] = useState(detailKey);
  const external = detailKey !== seenKey && !same(detail.settings, base);
  useEffect(() => {
    if (detailKey === seenKey) return;
    if (same(detail.settings, base)) setSeenKey(detailKey);
    else if (!dirty) {
      adopt(detail);
      setSeenKey(detailKey);
    }
  }, [detailKey, seenKey, detail, base, dirty, adopt]);

  // Validate the derived settings as the user edits; unedited settings keep the server's issues for them.
  const settingsKey = stableJson(derived.settings);
  useEffect(() => {
    if (!dirty) {
      setServerIssues(baseIssues);
      setChecking(false);
      return;
    }
    let alive = true;
    setChecking(true);
    const t = setTimeout(() => {
      api.validateSettings(ws, JSON.parse(settingsKey)).then(
        (list) => alive && setServerIssues(list),
        () => {},
      ).finally(() => alive && setChecking(false));
    }, VALIDATE_DELAY_MS);
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [ws, settingsKey, dirty, baseIssues]);

  const update = (fn: (f: SettingsForm) => void) => {
    setSaved(false);
    setForm((prev) => {
      const next = structuredClone(prev);
      fn(next);
      return next;
    });
  };

  const go = useCallback((key: string) => {
    setTab(tabOf(key));
    setTarget((t) => ({ key, n: (t?.n ?? 0) + 1 }));
  }, []);

  const { anchor, nonce } = useAnchor();
  useEffect(() => {
    const key = settingKeyOf(anchor);
    if (key) go(key);
  }, [anchor, nonce, go]);

  useAfterPaint(() => {
    if (!target) return;
    const el = anchorCandidates(target.key)
      .map((id) => document.getElementById(id))
      .find((e) => e !== null);
    flash(el ?? null);
  }, [target, tab]);

  const discard = () => {
    adopt(detail);
    setSeenKey(detailKey);
    setError(null);
    setSaved(false);
  };

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      adopt(await api.saveSettings(ws, derived.settings));
      setSaved(true);
      onSaved();
    } catch (e) {
      if (e instanceof HttpError && e.issues.length) {
        setServerIssues(e.issues);
        go(e.issues[0].path);
      }
      setError((e as Error).message);
    } finally {
      setSaving(false);
    }
  };

  const status = checking
    ? { text: "Checking settings…", color: "var(--text-muted)" }
    : issues.length > 0
      ? { text: `${issues.length} problem${issues.length === 1 ? "" : "s"} to fix before saving`, color: "var(--bad)" }
      : saved
        ? { text: "Saved to .ostra/workspace.toml", color: "var(--ok)" }
        : dirty
          ? { text: "Unsaved changes", color: "var(--text-secondary)" }
          : { text: "Checked as you edit", color: "var(--text-muted)" };

  // Issues the current tab cannot show next to a field: those on other tabs, and those no field claims.
  const elsewhere = [...issues.filter((i) => tabOf(i.path) !== tab), ...map.unmatched.filter((i) => tabOf(i.path) === tab)];
  const props: SectionProps = { form, update, issues: issuesFor };

  return (
    <Page
      title="Settings"
      actions={
        <div className="wp-row" style={{ alignSelf: "center" }}>
          <span style={{ fontSize: "var(--text-sm)", color: status.color }} role="status">
            {status.text}
          </span>
          <Button size="sm" variant="ghost" disabled={!dirty || saving} onClick={discard}>
            Discard
          </Button>
          <Button variant="primary" size="sm" icon="check" disabled={!dirty || saving || checking || issues.length > 0} onClick={() => void save()}>
            {saving ? "Saving…" : "Save"}
          </Button>
        </div>
      }
    >
      {external && dirty && (
        <Banner
          tone="info"
          actions={
            <Button size="sm" onClick={discard}>
              Load the saved settings
            </Button>
          }
        >
          The settings file changed since you started editing. Saving writes your version over it.
        </Banner>
      )}
      {saved && <Banner tone="info">The next execution uses these settings; no restart is needed.</Banner>}
      {error && <Banner tone="bad">{error}</Banner>}
      {elsewhere.length > 0 && (
        <Banner tone="bad" title="Problems to fix before saving">
          <ul className="wp-issues" style={{ paddingLeft: 16 }}>
            {elsewhere.map((i, n) => (
              <li key={n}>
                <button type="button" onClick={() => go(i.path)} style={linkButton} title="Show this field">
                  <code>{i.path}</code>
                </button>
                : {i.message}
              </li>
            ))}
          </ul>
        </Banner>
      )}
      <Tabs
        label="Settings sections"
        value={tab}
        onChange={(id) => setTab(id as SettingsTab)}
        tabs={SETTINGS_TABS.map((t) => ({ ...t, count: map.byTab[t.id] || undefined }))}
      />
      {tab === "general" && <GeneralSection {...props} />}
      {tab === "projects" && <ProjectsSection {...props} onAdd={addProject} stacks={detail.stacks} />}
      {tab === "routing" && <RoutingSection {...props} harnesses={detail.harnesses} agentInfo={detail.agents} />}
      {tab === "permissions" && <PermissionsSection {...props} global={detail.global_permissions} />}
      {tab === "instructions" && <InstructionsSection {...props} />}
      {tab === "notifications" && <NotificationsSection {...props} />}
    </Page>
  );
}

const linkButton = { background: "none", border: 0, padding: 0, color: "inherit", font: "inherit", cursor: "pointer", textDecoration: "underline" } as const;
