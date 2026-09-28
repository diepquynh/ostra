import type {
  ClosingChoice,
  ClosingItem,
  GateAnswer,
  GatePayload,
  GateView,
  PermissionAnswer,
  Question,
  SkillProposal,
} from "../api/types";
import { humanize } from "./format";

export type GateKindName = GatePayload["kind"];
export type AnswerKind = GateAnswer["kind"];

/** The answer kind each gate accepts, mirroring `runner::validate_answer` in the engine. */
export const ANSWER_KIND: Record<GateKindName, AnswerKind> = {
  open_questions: "questions",
  spec_approval: "approval",
  plan_approval: "approval",
  fact_check_recurring: "choice",
  review_cap: "choice",
  stuck: "choice",
  phase_blocked: "choice",
  closing_gate: "closing",
  permission: "permission",
  harness_failure: "choice",
  skill_approval: "skills",
  execution_failed: "choice",
  budget_reached: "choice",
  implementation_review: "choice",
};

export const OTHER = "__other__";

/** Per-question selection in the open-questions card: option labels, or OTHER with free text. */
export type QuestionSelection = { selected: string[]; other: string };

/** Options in display order: the recommended option first, labelled "(Recommended)". */
export function orderedOptions(q: Question): { label: string; description: string; recommended: boolean }[] {
  const opts = q.options.map((o, i) => ({ ...o, recommended: i === q.recommended }));
  const rec = opts.findIndex((o) => o.recommended);
  if (rec > 0) opts.unshift(...opts.splice(rec, 1));
  return opts;
}

export function defaultSelections(questions: Question[]): Record<string, QuestionSelection> {
  const out: Record<string, QuestionSelection> = {};
  for (const q of questions) out[q.id] = { selected: [], other: "" };
  return out;
}

/** Build a Questions answer. Returns an error naming the first unanswered question. */
export function questionsAnswer(
  questions: Question[],
  selections: Record<string, QuestionSelection>,
): { answer: GateAnswer } | { error: string } {
  const answers = [];
  for (const q of questions) {
    const sel = selections[q.id] ?? { selected: [], other: "" };
    const labels = sel.selected.filter((s) => s !== OTHER);
    const parts = [...labels];
    if (sel.selected.includes(OTHER)) {
      if (!sel.other.trim()) return { error: `${q.id}: write your answer in the Other field.` };
      parts.push(sel.other.trim());
    }
    if (parts.length === 0) return { error: `${q.id} has no answer yet.` };
    answers.push({ id: q.id, question: q.question, answer: parts.join("; ") });
  }
  return { answer: { kind: "questions", answers } };
}

export function toggleSelection(sel: QuestionSelection, label: string, multi: boolean): QuestionSelection {
  if (!multi) return { ...sel, selected: [label] };
  const has = sel.selected.includes(label);
  return { ...sel, selected: has ? sel.selected.filter((s) => s !== label) : [...sel.selected, label] };
}

export function approval(approved: boolean, feedback?: string): GateAnswer {
  const text = feedback?.trim() ?? "";
  return { kind: "approval", approved, feedback: text ? text : null };
}

/** A change request must say what to change, because the engine rejects an empty one. */
export function changeRequest(feedback: string): { answer: GateAnswer } | { error: string } {
  if (!feedback.trim()) return { error: "Say what should change so the spec agent can rewrite it." };
  return { answer: approval(false, feedback) };
}

export function choice(option: string, text?: string): GateAnswer {
  const t = text?.trim() ?? "";
  return { kind: "choice", option, text: t ? t : null };
}

export type ChoiceGateKind =
  | "fact_check_recurring"
  | "review_cap"
  | "stuck"
  | "phase_blocked"
  | "harness_failure"
  | "execution_failed"
  | "budget_reached"
  | "implementation_review";

export type ChoiceOption = {
  /** The option string the engine's fold matches on. */
  option: string;
  /** Button label. It names the object, and the answered card repeats it. */
  label: string;
  variant: "primary" | "default" | "danger";
  /** Whether the option sends the gate's text field: never, when filled in, or always (required). */
  text: "none" | "optional" | "required";
  /** Shown when a required text is empty. */
  missing?: string;
};

/** The options of every Choice gate, in button order. Exactly one is primary. */
export const CHOICES: Record<ChoiceGateKind, ChoiceOption[]> = {
  fact_check_recurring: [
    { option: "another-round", label: "Run another fact-check round", variant: "primary", text: "optional" },
    { option: "stop", label: "Stop the session", variant: "danger", text: "none" },
  ],
  review_cap: [
    { option: "another-pass", label: "Run another fix and review pass", variant: "primary", text: "optional" },
    { option: "stop", label: "Stop and block the phase", variant: "default", text: "none" },
  ],
  stuck: [
    {
      option: "fact",
      label: "Re-run with this fact",
      variant: "primary",
      text: "required",
      missing: "Write the missing fact first. The agent re-runs with it quoted verbatim.",
    },
    { option: "block", label: "Block this work", variant: "default", text: "none" },
  ],
  phase_blocked: [
    { option: "retry", label: "Retry the phase", variant: "primary", text: "optional" },
    { option: "leave", label: "Leave the phase blocked", variant: "default", text: "none" },
  ],
  harness_failure: [
    { option: "retry", label: "Retry after login", variant: "primary", text: "none" },
    { option: "native", label: "Run on the native executor", variant: "default", text: "none" },
  ],
  execution_failed: [
    { option: "retry", label: "Retry the execution", variant: "primary", text: "none" },
    { option: "abandon", label: "Abandon this work", variant: "default", text: "none" },
  ],
  budget_reached: [
    { option: "raise", label: "Raise the budget", variant: "primary", text: "optional" },
    { option: "stop", label: "Stop the session", variant: "danger", text: "none" },
  ],
  implementation_review: [
    { option: "done", label: "Accept the implementation", variant: "primary", text: "none" },
    {
      option: "feedback",
      label: "Send feedback",
      variant: "default",
      text: "required",
      missing: "Describe what to change first. Ostra builds it as a reviewed revision.",
    },
  ],
};

export const isChoiceKind = (kind: GateKindName): kind is ChoiceGateKind => ANSWER_KIND[kind] === "choice";

/** A positive dollar amount such as `10` or `$12.50`, or null. The engine parses the same shape. */
export function parseDollars(text: string): number | null {
  const v = Number(text.trim().replace(/^\$/, ""));
  return text.trim() && Number.isFinite(v) && v > 0 ? v : null;
}

/** Build a Choice answer for one of the gate's options, checking the text field the option needs. */
export function choiceAnswer(
  kind: ChoiceGateKind,
  option: string,
  text = "",
): { answer: GateAnswer } | { error: string } {
  const spec = CHOICES[kind].find((o) => o.option === option);
  if (!spec) return { error: `This gate has no option ${option}.` };
  if (spec.text === "required" && !text.trim())
    return { error: spec.missing ?? `Fill in the text above before choosing "${spec.label}".` };
  if (kind === "budget_reached" && spec.option === "raise" && text.trim() && parseDollars(text) === null) {
    return { error: "Enter the amount as a number of US dollars, such as 10. Leave it empty to add the budget again." };
  }
  return { answer: choice(option, spec.text === "none" ? undefined : text) };
}

/** Closing gate: for each project, only the questions asked are taken from the form. */
export function closingAnswer(
  items: ClosingItem[],
  picks: Record<string, { tests: boolean; docs: boolean }>,
): GateAnswer {
  const out: ClosingChoice[] = items.map((item) => {
    const p = picks[item.project] ?? { tests: false, docs: false };
    return { project: item.project, tests: item.ask_tests ? p.tests : false, docs: item.ask_docs ? p.docs : false };
  });
  return { kind: "closing", items: out };
}

export function permission(answer: PermissionAnswer): GateAnswer {
  return { kind: "permission", answer };
}

export const PERMISSION_LABELS: Record<PermissionAnswer, string> = {
  "allow-once": "Allow once",
  "always-in-workspace": "Always in this workspace",
  deny: "Deny",
};

export const DISPOSITIONS = ["generate", "regenerate", "reuse", "drop"] as const;

export function skillsAnswer(skills: SkillProposal[], picks: Record<string, string>): GateAnswer {
  return {
    kind: "skills",
    decisions: skills.map((s) => ({ name: s.name, disposition: picks[s.name] ?? s.disposition })),
  };
}

/** One line saying what was answered, for the answered gate card. */
export function answerSummary(gate: Pick<GateView, "payload" | "answer">): string {
  const a = gate.answer;
  if (!a) return "";
  switch (a.kind) {
    case "questions":
      return a.answers.map((x) => `${x.id}: ${x.answer}`).join("; ");
    case "approval":
      return a.approved ? "Approved" : `Changes requested: ${a.feedback ?? ""}`;
    case "choice": {
      const kind = gate.payload.kind;
      const label =
        (isChoiceKind(kind) ? CHOICES[kind].find((o) => o.option === a.option)?.label : undefined) ??
        humanize(a.option);
      if (!a.text) return label;
      return kind === "budget_reached" && parseDollars(a.text) !== null
        ? `${label} by $${parseDollars(a.text)!.toFixed(2)}`
        : `${label}: ${a.text}`;
    }
    case "closing":
      return a.items
        .map((i) => `${i.project}: tests ${i.tests ? "yes" : "no"}, docs ${i.docs ? "yes" : "no"}`)
        .join("; ");
    case "permission":
      return PERMISSION_LABELS[a.answer];
    case "skills":
      return a.decisions.map((d) => `${d.name} ${d.disposition}`).join(", ");
  }
}
