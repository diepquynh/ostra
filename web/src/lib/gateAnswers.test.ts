import { describe, expect, it } from "vitest";
import type { GateAnswer, GateView, Question } from "../api/types";
import {
  ANSWER_KIND,
  answerSummary,
  approval,
  CHOICES,
  type ChoiceGateKind,
  changeRequest,
  choice,
  choiceAnswer,
  closingAnswer,
  defaultSelections,
  isChoiceKind,
  OTHER,
  orderedOptions,
  parseDollars,
  permission,
  questionsAnswer,
  skillsAnswer,
  toggleSelection,
} from "./gateAnswers";

const q = (id: string, recommended = 1, multi = false): Question => ({
  id,
  question: `Question ${id}?`,
  tag: "Scope",
  options: [
    { label: "A", description: "first" },
    { label: "B", description: "second" },
  ],
  recommended,
  multi_select: multi,
});

describe("open questions", () => {
  it("lists the recommended option first", () => {
    expect(orderedOptions(q("Q1", 1)).map((o) => [o.label, o.recommended])).toEqual([
      ["B", true],
      ["A", false],
    ]);
  });

  it("requires every question answered and an Other text when Other is picked", () => {
    const qs = [q("Q1"), q("Q2")];
    const sel = defaultSelections(qs);
    expect(questionsAnswer(qs, sel)).toEqual({ error: "Q1 has no answer yet." });
    sel.Q1 = toggleSelection(sel.Q1, "A", false);
    sel.Q2 = toggleSelection(sel.Q2, OTHER, false);
    expect(questionsAnswer(qs, sel)).toEqual({ error: "Q2: write your answer in the Other field." });
    sel.Q2 = { ...sel.Q2, other: " Only admins " };
    expect(questionsAnswer(qs, sel)).toEqual({
      answer: {
        kind: "questions",
        answers: [
          { id: "Q1", question: "Question Q1?", answer: "A" },
          { id: "Q2", question: "Question Q2?", answer: "Only admins" },
        ],
      },
    });
  });

  it("toggles multi-select and replaces single-select", () => {
    let s = { selected: [] as string[], other: "" };
    s = toggleSelection(s, "A", true);
    s = toggleSelection(s, "B", true);
    expect(s.selected).toEqual(["A", "B"]);
    s = toggleSelection(s, "A", true);
    expect(s.selected).toEqual(["B"]);
    expect(toggleSelection(s, "A", false).selected).toEqual(["A"]);
  });
});

describe("other answers", () => {
  it("builds approvals and change requests", () => {
    expect(approval(true)).toEqual({ kind: "approval", approved: true, feedback: null });
    expect(changeRequest("  ")).toHaveProperty("error");
    expect(changeRequest("Drop R3")).toEqual({ answer: { kind: "approval", approved: false, feedback: "Drop R3" } });
  });

  it("builds choices with optional text", () => {
    expect(choice("stop")).toEqual({ kind: "choice", option: "stop", text: null });
    expect(choice("fact", " JDK 21 ")).toEqual({ kind: "choice", option: "fact", text: "JDK 21" });
  });

  it("only takes closing answers for questions that were asked", () => {
    const a = closingAnswer(
      [
        { project: "web", phases: 2, ask_tests: true, ask_docs: false },
        { project: "api", phases: 1, ask_tests: true, ask_docs: true },
      ],
      { web: { tests: true, docs: true } },
    );
    expect(a).toEqual({
      kind: "closing",
      items: [
        { project: "web", tests: true, docs: false },
        { project: "api", tests: false, docs: false },
      ],
    });
  });

  it("builds permission and skill answers", () => {
    expect(permission("always-in-workspace")).toEqual({ kind: "permission", answer: "always-in-workspace" });
    expect(
      skillsAnswer(
        [
          { name: "entity", kind: "creation", description: "", disposition: "generate", exemplars: [] },
          { name: "convention", kind: "convention", description: "", disposition: "reuse", exemplars: [] },
        ],
        { entity: "drop" },
      ),
    ).toEqual({
      kind: "skills",
      decisions: [
        { name: "entity", disposition: "drop" },
        { name: "convention", disposition: "reuse" },
      ],
    });
  });
});

describe("answer shapes per gate kind", () => {
  it("maps every gate kind to the answer kind the engine's validate_answer accepts", () => {
    expect(ANSWER_KIND).toEqual({
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
    });
  });

  it("gives every choice gate exactly one primary option, with the option strings the fold matches", () => {
    const options = Object.fromEntries(Object.entries(CHOICES).map(([k, v]) => [k, v.map((o) => o.option)]));
    expect(options).toEqual({
      fact_check_recurring: ["another-round", "stop"],
      review_cap: ["another-pass", "stop"],
      stuck: ["fact", "block"],
      phase_blocked: ["retry", "leave"],
      harness_failure: ["retry", "native"],
      execution_failed: ["retry", "abandon"],
      budget_reached: ["raise", "stop"],
    });
    for (const [kind, opts] of Object.entries(CHOICES)) {
      expect(
        opts.filter((o) => o.variant === "primary"),
        kind,
      ).toHaveLength(1);
      expect(isChoiceKind(kind as ChoiceGateKind)).toBe(true);
    }
    expect(isChoiceKind("permission")).toBe(false);
  });

  it("sends the text field only for options that take it", () => {
    expect(choiceAnswer("review_cap", "another-pass", " Keep the 409 ")).toEqual({
      answer: { kind: "choice", option: "another-pass", text: "Keep the 409" },
    });
    expect(choiceAnswer("review_cap", "stop", "ignored")).toEqual({
      answer: { kind: "choice", option: "stop", text: null },
    });
    expect(choiceAnswer("fact_check_recurring", "another-round", "")).toEqual({
      answer: { kind: "choice", option: "another-round", text: null },
    });
    expect(choiceAnswer("phase_blocked", "retry", "Use the v2 client")).toEqual({
      answer: { kind: "choice", option: "retry", text: "Use the v2 client" },
    });
    expect(choiceAnswer("harness_failure", "native", "x")).toEqual({
      answer: { kind: "choice", option: "native", text: null },
    });
    expect(choiceAnswer("execution_failed", "abandon")).toEqual({
      answer: { kind: "choice", option: "abandon", text: null },
    });
    expect(choiceAnswer("stuck", "nope")).toEqual({ error: "This gate has no option nope." });
  });

  it("requires the missing fact before a stuck agent re-runs", () => {
    expect(choiceAnswer("stuck", "fact", "  ")).toHaveProperty("error");
    expect(choiceAnswer("stuck", "fact", "Use SDK v2.8")).toEqual({
      answer: { kind: "choice", option: "fact", text: "Use SDK v2.8" },
    });
    expect(choiceAnswer("stuck", "block", "")).toEqual({ answer: { kind: "choice", option: "block", text: null } });
  });

  it("accepts an empty or positive dollar amount when raising the budget", () => {
    expect(choiceAnswer("budget_reached", "raise", "")).toEqual({
      answer: { kind: "choice", option: "raise", text: null },
    });
    expect(choiceAnswer("budget_reached", "raise", "$12.50")).toEqual({
      answer: { kind: "choice", option: "raise", text: "$12.50" },
    });
    expect(choiceAnswer("budget_reached", "raise", "ten")).toHaveProperty("error");
    expect(choiceAnswer("budget_reached", "raise", "-3")).toHaveProperty("error");
    expect(choiceAnswer("budget_reached", "stop", "ten")).toEqual({
      answer: { kind: "choice", option: "stop", text: null },
    });
    expect(parseDollars(" 7 ")).toBe(7);
    expect(parseDollars("0")).toBeNull();
  });

  it("summarizes answers with the labels of the buttons that gave them", () => {
    const g = (payload: GateView["payload"], answer: GateAnswer) => answerSummary({ payload, answer });
    const budget: GateView["payload"] = { kind: "budget_reached", spent_usd: 5, budget_usd: 5 };
    expect(g(budget, choice("raise", "10"))).toBe("Raise the budget by $10.00");
    expect(g(budget, choice("stop"))).toBe("Stop the session");
    expect(g({ kind: "phase_blocked", project: "web", phase: 3, reason: "" }, choice("retry", "Try v2"))).toBe(
      "Retry the phase: Try v2",
    );
    expect(g({ kind: "spec_approval", spec_path: "", summary: "", findings: [] }, approval(true))).toBe("Approved");
    expect(
      g(
        { kind: "plan_approval", plan_path: "", summary: "", phases: [], findings: [] },
        approval(false, "Split phase 2"),
      ),
    ).toBe("Changes requested: Split phase 2");
    const perm: GateView["payload"] = {
      kind: "permission",
      execution: "x",
      agent: "implementer",
      call: { tool: "Bash", input: {} },
      reason: "",
      rule: { layer: "permission", rule: "mode:default" },
      suggestion: null,
    };
    expect(g(perm, permission("deny"))).toBe("Deny");
    expect(g(perm, permission("always-in-workspace"))).toBe("Always in this workspace");
    expect(
      g(
        { kind: "closing_gate", items: [] },
        { kind: "closing", items: [{ project: "web", tests: true, docs: false }] },
      ),
    ).toBe("web: tests yes, docs no");
    expect(
      g(
        { kind: "skill_approval", project: "web", skills: [] },
        { kind: "skills", decisions: [{ name: "entity", disposition: "drop" }] },
      ),
    ).toBe("entity drop");
    expect(
      g(
        { kind: "open_questions", artifact: "spec", artifact_path: "", questions: [] },
        { kind: "questions", answers: [{ id: "Q1", question: "", answer: "A" }] },
      ),
    ).toBe("Q1: A");
    expect(answerSummary({ payload: budget, answer: null })).toBe("");
  });
});
