import type {
  ClosingChoice,
  ClosingItem,
  GateAnswer,
  PermissionAnswer,
  Question,
  SkillProposal,
} from "../api/types";

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

/** A change request must say what to change. */
export function changeRequest(feedback: string): { answer: GateAnswer } | { error: string } {
  if (!feedback.trim()) return { error: "Say what should change so the spec agent can rewrite it." };
  return { answer: approval(false, feedback) };
}

export function choice(option: string, text?: string): GateAnswer {
  const t = text?.trim() ?? "";
  return { kind: "choice", option, text: t ? t : null };
}

/** Closing gate: for each project, only the questions asked are taken from the form. */
export function closingAnswer(items: ClosingItem[], picks: Record<string, { tests: boolean; docs: boolean }>): GateAnswer {
  const out: ClosingChoice[] = items.map((item) => {
    const p = picks[item.project] ?? { tests: false, docs: false };
    return { project: item.project, tests: item.ask_tests ? p.tests : false, docs: item.ask_docs ? p.docs : false };
  });
  return { kind: "closing", items: out };
}

export function permission(answer: PermissionAnswer): GateAnswer {
  return { kind: "permission", answer };
}

export const DISPOSITIONS = ["generate", "regenerate", "reuse", "drop"] as const;

export function skillsAnswer(skills: SkillProposal[], picks: Record<string, string>): GateAnswer {
  return {
    kind: "skills",
    decisions: skills.map((s) => ({ name: s.name, disposition: picks[s.name] ?? s.disposition })),
  };
}
