import { useCallback, useEffect, useMemo, useState } from "react";
import { api, HttpError } from "../../api";
import type { ValidationIssue, WorkspaceDetail } from "../../api/types";
import { flash, useAfterPaint, useAnchor } from "./Page";
import type { SectionProps } from "./SettingsSections";
import {
  anchorCandidates,
  fieldIds,
  fromForm,
  mapIssues,
  type SettingsForm,
  type SettingsTab,
  settingKeyOf,
  stableJson,
  tabOf,
  toForm,
} from "./settingsForm";

const VALIDATE_DELAY_MS = 400;

const same = (a: unknown, b: unknown) => stableJson(a) === stableJson(b);

function dedupe(list: ValidationIssue[]): ValidationIssue[] {
  const seen = new Set<string>();
  return list.filter((i) => {
    const k = `${i.path}\n${i.message}`;
    return !seen.has(k) && (seen.add(k), true);
  });
}

/**
 * The settings editor's state: the form, validation as you edit, save and discard, and the
 * `#setting:<dotted key>` anchor that opens a field's tab and rings the field.
 */
export function useSettingsEditor(ws: string, detail: WorkspaceDetail, onSaved: () => void) {
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
      api
        .validateSettings(ws, JSON.parse(settingsKey))
        .then(
          (list) => alive && setServerIssues(list),
          () => {},
        )
        .finally(() => alive && setChecking(false));
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
  const elsewhere = [
    ...issues.filter((i) => tabOf(i.path) !== tab),
    ...map.unmatched.filter((i) => tabOf(i.path) === tab),
  ];

  const props: SectionProps = { form, update, issues: issuesFor };
  return {
    base,
    form,
    tab,
    setTab,
    issues,
    map,
    dirty,
    checking,
    saving,
    saved,
    error,
    external,
    elsewhere,
    status,
    props,
    go,
    discard,
    save,
  };
}
