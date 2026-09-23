import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import { HttpError } from "../../api/client";
import type { ValidationIssue, WorkspaceDetail } from "../../api/types";
import { useAsync } from "../../lib/hooks";
import { useHome } from "./folders";
import { canContinue, creationTasks, initialValues, issuesByStep, stepsFor, toCreateBody, type StepId, type WizardValues } from "./wizard";

/** Time each creation checklist row stays in progress before the next one starts. */
export const TASK_MS = 420;

export type Wizard = ReturnType<typeof useWizard>;

/** State for the setup steps, shared by the full-screen setup and the New workspace dialog. */
export function useWizard(skipWelcome: boolean) {
  const steps = stepsFor(skipWelcome);
  const [i, setI] = useState(0);
  const [dir, setDir] = useState<"forward" | "back">("forward");
  const [v, setV] = useState<WizardValues>(initialValues);
  const env = useAsync(() => api.environment(), []);
  const home = useHome();

  const [issues, setIssues] = useState<ValidationIssue[]>([]);
  const [validating, setValidating] = useState(false);
  const [checkedFor, setCheckedFor] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<WorkspaceDetail | null>(null);
  const [createError, setCreateError] = useState<string | null>(null);
  const [progress, setProgress] = useState(0);

  const id: StepId = steps[i].id;
  const set = useCallback((patch: Partial<WizardValues>) => setV((x) => ({ ...x, ...patch })), []);
  const go = useCallback(
    (n: number) => {
      if (n < 0 || n >= steps.length) return;
      setDir(n < i ? "back" : "forward");
      setI(n);
    },
    [i, steps.length],
  );
  const goTo = (step: StepId) => go(steps.findIndex((s) => s.id === step));

  const body = useMemo(() => toCreateBody(v, home), [v, home]);
  const bodyKey = JSON.stringify(body);

  useEffect(() => {
    if (id !== "review" || creating || created) return;
    let live = true;
    setValidating(true);
    const t = setTimeout(() => {
      api.validateNewWorkspace(body).then(
        (found) => {
          if (!live) return;
          setIssues(found);
          setCheckedFor(bodyKey);
          setValidating(false);
        },
        (e: Error) => {
          if (!live) return;
          setIssues(e instanceof HttpError ? e.issues : []);
          setCreateError(e instanceof HttpError && e.issues.length ? null : e.message);
          setCheckedFor(bodyKey);
          setValidating(false);
        },
      );
    }, 150);
    return () => {
      live = false;
      clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, bodyKey, creating, created]);

  const tasks = creationTasks(v);
  const timer = useRef<ReturnType<typeof setInterval> | null>(null);
  useEffect(() => () => void (timer.current && clearInterval(timer.current)), []);

  const create = () => {
    setCreating(true);
    setCreateError(null);
    setProgress(0);
    // The checklist advances on a timer but holds on its last row until the server answers.
    timer.current = setInterval(() => setProgress((n) => Math.min(n + 1, tasks.length - 1)), TASK_MS);
    api.createWorkspace(body).then(
      (d) => {
        if (timer.current) clearInterval(timer.current);
        setCreated(d);
        timer.current = setInterval(
          () =>
            setProgress((n) => {
              if (n + 1 >= tasks.length && timer.current) clearInterval(timer.current);
              return Math.min(n + 1, tasks.length);
            }),
          TASK_MS / 2,
        );
      },
      (e: Error) => {
        if (timer.current) clearInterval(timer.current);
        setCreating(false);
        setIssues(e instanceof HttpError ? e.issues : []);
        setCreateError(e instanceof HttpError && e.issues.length ? null : e.message);
      },
    );
  };

  const upToDate = checkedFor === bodyKey && !validating;
  return {
    steps,
    i,
    id,
    dir,
    v,
    set,
    go,
    goTo,
    env,
    home,
    canNext: canContinue(id, v),
    issues,
    byStep: issuesByStep(issues),
    validating: validating || (id === "review" && checkedFor !== bodyKey),
    canCreate: upToDate && issues.length === 0 && !creating,
    create,
    creating,
    created,
    createError,
    tasks,
    progress,
    done: !!created && progress >= tasks.length,
  };
}
