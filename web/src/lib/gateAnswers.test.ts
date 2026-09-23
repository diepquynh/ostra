import { describe, expect, it } from "vitest";
import type { Question } from "../api/types";
import {
  OTHER,
  approval,
  changeRequest,
  choice,
  closingAnswer,
  defaultSelections,
  orderedOptions,
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
