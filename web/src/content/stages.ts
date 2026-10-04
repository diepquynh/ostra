import type { Lane, StageKind } from "../api/types";

export type LaneInfo = { title: string; why: string };

/** Why each lane exists. Adapted from Ultracode's docs/philosophy.md. */
export const LANES: Record<Lane, LaneInfo> = {
  research: {
    title: "Research",
    why: "Code written before the problem is understood carries wrong assumptions that nobody challenges until they are hundreds of lines deep. A grounded research pass reads the code and the current documentation first.",
  },
  requirements: {
    title: "Requirements",
    why: "One spec states what the change must do, as testable requirements with acceptance criteria. Every later stage traces back to it, so a missing requirement is caught here instead of in code.",
  },
  verification: {
    title: "Verification",
    why: "When you approve your own spec or plan, a wrong claim in it returns as broken code. Fact-check verifies every concrete claim against the project and the cited sources before you are asked to approve.",
  },
  design: {
    title: "Design",
    why: "Without a design step the architecture drifts. The plan turns the spec into phases with dependencies, risks, and a test policy, and it waits for your approval before any code is written.",
  },
  build: {
    title: "Build",
    why: "Each phase is implemented by an agent that loads the project's skills and conventions and verifies every step with the project's build command, one phase at a time per project.",
  },
  review: {
    title: "Review",
    why: "You cannot review code you wrote an hour ago with fresh eyes. The reviewer checks every change against the project's rules and the phase's requirements, and the loop repeats until the findings are fixed.",
  },
  test: {
    title: "Test",
    why: "Tests are the first thing dropped under pressure. When you ask for them, Ostra first maps what the change touches: every branch of the changed code, the flows that reach it from routes, screens, and other consumers, and the existing tests that cover it. Then it writes unit, integration, and end-to-end tests at the level each check needs and re-runs the existing tests. Which phases are skipped was decided in writing at planning time.",
  },
  docs: {
    title: "Docs",
    why: "Documentation debt grows when nobody writes down how a change works. When you ask for it, the workspace book is rewritten for each project from what actually changed.",
  },
  done: {
    title: "Done",
    why: "The completion report names every stage that did not run and how to run it later, and under YOLO lists every decision Ostra made for you, so nothing is hidden.",
  },
};

export type StageInfo = { label: string; produces: string; protects: string };

export const STAGES: Record<StageKind, StageInfo> = {
  intake: {
    label: "Intake",
    produces: "The recorded request and its options.",
    protects: "Losing what you asked for.",
  },
  classify: {
    label: "Classify",
    produces: "A category, the projects in scope, and the research tasks.",
    protects: "Running the wrong pipeline for the request, such as planning a question that needs only an answer.",
  },
  explore: {
    label: "Explore",
    produces: "One research document per task, with every external fact cited by URL and date.",
    protects: "Building on recalled knowledge of a library or API instead of its current documentation.",
  },
  sufficiency: {
    label: "Sufficiency check",
    produces: "A decision on each item the research did not cover: needed or not.",
    protects: "A spec written from incomplete research, which invalidates the plan built on it.",
  },
  track: {
    label: "Track",
    produces: "Light or full. The light track builds from the research; the full track writes a spec and a plan first.",
    protects: "Three approval rounds on a contained change, or none on a change to a contract or a schema.",
  },
  spec: {
    label: "Spec",
    produces:
      "One spec file: requirements in EARS form, acceptance criteria, contracts, and an External Evidence table.",
    protects: "Requirements that live only in someone's head and never reach the plan.",
  },
  "open-questions": {
    label: "Open questions",
    produces: "Your answers, written back into the spec.",
    protects: "Guessing what you want. Every answer rewrites the spec before anything checks it.",
  },
  "fact-check-spec": {
    label: "Fact-check the spec",
    produces: "A PASS or FAIL verdict with findings.",
    protects: "Claims about the code or external services that are false and would break the implementation.",
  },
  "spec-approval": {
    label: "Spec approval",
    produces: "Your approval, recorded only after a fact-check PASS.",
    protects: "Planning against requirements you have not agreed to.",
  },
  stakes: {
    label: "Stakes",
    produces: "Low, medium, or high. Low stakes skip the plan stage.",
    protects: "Spending a full plan on a one-line change, or skipping one on a risky change.",
  },
  plan: {
    label: "Plan",
    produces: "A master plan and one self-contained file per phase.",
    protects: "Architecture drift and a change that must land in five places landing in four.",
  },
  "fact-check-plan": {
    label: "Fact-check the plan",
    produces: "A PASS or FAIL verdict with findings.",
    protects: "Phase steps that name methods or files that do not exist.",
  },
  "plan-approval": {
    label: "Plan approval",
    produces: "Your approval, recorded only after a fact-check PASS.",
    protects: "Code written against a plan you have not seen.",
  },
  implement: {
    label: "Implement",
    produces: "The code for one phase and a change report.",
    protects: "Unverified code. Every step runs the project's build command.",
  },
  review: {
    label: "Review",
    produces: "Findings against the project's rule set and the phase's requirements, plus the review ledger.",
    protects: "Defects that compound across phases, and code that follows every convention but does the wrong thing.",
  },
  "implementation-review": {
    label: "Implementation review",
    produces: "Your feedback, built as reviewed revision phases, or your acceptance.",
    protects: "Tests and docs written for a result you have not tried yet.",
  },
  autofix: {
    label: "Auto-fix",
    produces: "Mechanical fixes applied from the reviewer's exact replacement text.",
    protects: "Spending an agent run on a one-token change.",
  },
  staging: {
    label: "Stage",
    produces: "The phase's files added to the git index.",
    protects: "Reviewing the same lines twice. Later reviews look only at unstaged changes.",
  },
  handoff: {
    label: "Handoff",
    produces: "A specialist's work, then the original agent resumes.",
    protects: "An agent authoring a prompt or skill it was not built to write.",
  },
  rescue: {
    label: "Rescue",
    produces: "The missing fact, from research or from you, then a re-run.",
    protects: "Retrying the same failing step with the same prompt and paying for it twice.",
  },
  format: {
    label: "Format",
    produces: "The project's format command, run once after its last phase.",
    protects: "Formatting noise in review and a formatter fighting later phases.",
  },
  "closing-gate": {
    label: "Closing gate",
    produces: "Your choice of optional stages: tests and documentation.",
    protects: "Spending tokens on stages you did not ask for.",
  },
  epa: {
    label: "Verification plan",
    produces:
      "Every branch through the changed functions, the flows that reach them, and the existing tests to re-run, each with a test level.",
    protects:
      "Tests that check each function alone and miss the route, the wiring, or the consumer that fails in production.",
  },
  "write-test": {
    label: "Write tests",
    produces:
      "Unit, integration, and end-to-end tests at the level each check needs, following the project's test skills, then a run of the existing tests that cover the change.",
    protects: "Testing as an afterthought. Tests run after every phase, so no later phase changes the code under test.",
  },
  "test-review": {
    label: "Test review",
    produces: "Findings on the tests, checked against the phase's acceptance criteria.",
    protects: "Tests that pass without asserting what the phase requires.",
  },
  documentation: {
    label: "Documentation",
    produces: "One project's part of the documentation book, grounded in the real source.",
    protects: "The how-it-works knowledge that is gone six months later.",
  },
  architecture: {
    label: "System architecture",
    produces: "How the book's projects communicate, fail, recover, and scale.",
    protects: "Changes that break a project the change never touched.",
  },
  "book-write": {
    label: "Write the book",
    produces: "The book in the workspace, as Markdown agents read and JSON the console renders.",
    protects: "Documentation that exists only in a session log.",
  },
  verify: {
    label: "Verify",
    produces: "The project's test command run and reported.",
    protects: "Claiming it works without running it.",
  },
  "prompt-gen": {
    label: "Prompt generation",
    produces: "Instruction files written to the prompt authoring standard.",
    protects: "Ambiguous instructions that a model follows in the wrong direction.",
  },
  "quick-answer": {
    label: "Quick answer",
    produces: "A direct answer, read-only.",
    protects: "Running the pipeline for a question.",
  },
  completion: {
    label: "Completion report",
    produces: "What was done, what did not run and how to run it, and every decision made for you.",
    protects: "Silent omissions that read as bugs later.",
  },
  detect: {
    label: "Detect",
    produces: "The project's stack and the slices to scout.",
    protects: "Generating skills for a stack the project does not use.",
  },
  scout: {
    label: "Scout",
    produces: "Real exemplar files for each recurring component pattern.",
    protects: "Skills based on invented framework patterns instead of this project's code.",
  },
  propose: {
    label: "Propose skills",
    produces: "A skill proposal for your approval.",
    protects: "Skills nobody asked for, or missing ones for components the project has.",
  },
  "skill-approval": {
    label: "Skill approval",
    produces: "Your choice per skill: generate, regenerate, reuse, or drop.",
    protects: "Overwriting a skill you wrote by hand.",
  },
  "generate-skill": {
    label: "Generate skills",
    produces: "One SKILL.md per approved skill.",
    protects: "New code that does not match the old.",
  },
  "generate-inventory": {
    label: "Generate inventory",
    produces: "INVENTORY.md and project.toml, the tables every agent routes by.",
    protects: "Routing work by skill descriptions instead of by name.",
  },
  custom: {
    label: "Workflow stage",
    produces: "The result of a stage your workflow adds: a verdict, a summary, and findings.",
    protects: "Skipping a check your team requires between Ostra's own stages.",
  },
};

/** The collapsible note shown the first time a spec or plan is viewed. */
export const EARS_NOTE = {
  title: "How to read requirements and acceptance criteria",
  body: `Requirements use **EARS** (Easy Approach to Requirements Syntax), which fixes the shape of each sentence so it can be tested:

- **Ubiquitous:** "The system shall ..." holds all the time.
- **Event-driven:** "When <trigger>, the system shall ..." holds after the trigger.
- **State-driven:** "While <state>, the system shall ..." holds during the state.
- **Unwanted behavior:** "If <condition>, then the system shall ..." covers errors and edge cases.
- **Optional feature:** "Where <feature is present>, the system shall ..."

Acceptance criteria use **Given / When / Then**: *Given* a starting state, *when* an action happens, *then* an observable outcome follows. Each criterion becomes at least one test, and the reviewer checks the code against them.`,
};
