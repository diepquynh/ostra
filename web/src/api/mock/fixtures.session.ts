// Mock sessions for the session board: one per gate kind the demo session does not already show, so
// `npm run dev:mock` can render every gate card. The demo session (fixtures.ts) covers the permission ask
// and the closing gate.

import type {
  AgentName,
  ArtifactRef,
  DecisionView,
  ExecPurpose,
  ExecutionView,
  GatePayload,
  GateView,
  Lane,
  PhaseView,
  SessionDetail,
  SessionEvent,
  SessionSummary,
  StageCard,
  StageKind,
  StageStatus,
  StoredEvent,
} from "../types";
import * as f from "./fixtures";

const now = new Date("2026-09-22T10:00:00Z");
const at = (minutes: number) => new Date(now.getTime() + minutes * 60000).toISOString();
const ROOT = "/home/me/code/shop";
const sroot = (id: string) => `${ROOT}/.ostra/sessions/${id}`;

type StageSpec = [StageKind, Lane, string, StageStatus, Partial<StageCard>?];

type ExecSpec = {
  id: string;
  agent: AgentName;
  stage: StageKind;
  project: string;
  status: ExecutionView["status"];
  start: number;
  end: number | null;
  purpose: ExecPurpose;
  run: string;
  cost: number;
  extra?: Partial<ExecutionView>;
};

type GateSpec = { id: string; title: string; explanation: string; payload: GatePayload; opened: number };

type SessionSpec = {
  id: string;
  title: string;
  request: string;
  category: SessionSummary["category"];
  kind?: SessionSummary["kind"];
  status: SessionSummary["status"];
  lane: Lane;
  stage_label: string;
  projects: string[];
  start: number;
  yolo?: boolean;
  stages: StageSpec[];
  executions: ExecSpec[];
  gates: GateSpec[];
  phases?: PhaseView[];
  decisions?: DecisionView[];
  artifacts?: ArtifactRef[];
  extraEvents?: [number, SessionEvent][];
};

function execution(session: string, x: ExecSpec): ExecutionView {
  const harness = x.extra?.executor?.startsWith("harness:") ?? false;
  return {
    id: x.id,
    session,
    agent: x.agent,
    purpose: x.purpose,
    stage: x.stage,
    project: x.project,
    executor: "native",
    model: "anthropic:claude-sonnet-5-5",
    status: x.status,
    started_at: at(x.start),
    ended_at: x.end === null ? null : at(x.end),
    usage: {
      input_tokens: 21000,
      output_tokens: 3100,
      cache_read_tokens: 140000,
      cache_write_tokens: 9000,
      cache_write_1h_tokens: 0,
      cost_usd: x.cost,
      tool_calls: 18,
      build_ms: 0,
      context_tokens: 0,
    },
    report_path: null,
    native_session_id: null,
    spawn_block: `Workspace root: ${ROOT}\nRepo root: /home/me/code/shop-${x.project}\nSession dir: ${sroot(session)}/${x.project}\nRepo key: ${x.project}`,
    error: null,
    can_resume: false,
    has_terminal: false,
    group: `${x.agent}:${x.project}`,
    run_label: x.run,
    stream: harness ? "terminal" : "activity",
    summary: null,
    has_transcript: false,
    pending_gate: null,
    repo_root: `/home/me/code/shop-${x.project}`,
    ...x.extra,
  };
}

function build(spec: SessionSpec): SessionDetail {
  const executions = spec.executions.map((x) => execution(spec.id, x));
  const gates: GateView[] = spec.gates.map((g) => ({
    id: g.id,
    session: spec.id,
    title: g.title,
    explanation: g.explanation,
    payload: g.payload,
    answer: null,
    source: null,
    reason: null,
    opened_at: at(g.opened),
    answered_at: null,
  }));
  const updated = Math.max(
    spec.start,
    ...spec.executions.map((x) => x.end ?? x.start),
    ...spec.gates.map((g) => g.opened),
  );
  const summary: SessionSummary = {
    id: spec.id,
    workspace: f.WS,
    kind: spec.kind ?? { kind: "pipeline" },
    request: spec.request,
    category: spec.category,
    status: spec.status,
    lane: spec.lane,
    stage_label: spec.stage_label,
    yolo: spec.yolo ?? false,
    open_gates: gates.length,
    projects: spec.projects,
    cost_usd: Math.round(executions.reduce((s, x) => s + x.usage.cost_usd, 0) * 100) / 100,
    created_at: at(spec.start),
    updated_at: at(updated),
    title: spec.title,
  };
  const stages: StageCard[] = spec.stages.map(([stage, lane, label, status, extra]) => ({
    stage,
    lane,
    label,
    status,
    project: null,
    phase: null,
    executions: executions
      .filter((x) => x.stage === stage && (extra?.project == null || x.project === extra.project))
      .map((x) => x.id),
    gate:
      gates.find(
        (g) => stageOfGate(g.payload) === stage && (extra?.phase == null || phaseOfGate(g.payload) === extra.phase),
      )?.id ?? null,
    detail: null,
    ...extra,
  }));
  return {
    summary,
    stages,
    phases: spec.phases ?? [],
    executions,
    gates,
    decisions: spec.decisions ?? [],
    artifacts: spec.artifacts ?? [],
    completion: null,
    session_root: sroot(spec.id),
    execution_groups: f.groupsFor(executions),
    fact_checks: [],
    files: [],
    uploads: [],
    additions: [],
  };
}

function stageOfGate(p: GatePayload): StageKind | null {
  switch (p.kind) {
    case "open_questions":
      return "open-questions";
    case "spec_approval":
      return "spec-approval";
    case "plan_approval":
      return "plan-approval";
    case "fact_check_recurring":
      return p.target === "spec" ? "fact-check-spec" : "fact-check-plan";
    case "review_cap":
      return "review";
    case "stuck":
      return "rescue";
    case "phase_blocked":
      return "implement";
    case "closing_gate":
      return "closing-gate";
    case "skill_approval":
      return "skill-approval";
    default:
      return null;
  }
}

const phaseOfGate = (p: GatePayload): number | null => ("phase" in p && typeof p.phase === "number" ? p.phase : null);

const decision = (
  id: string,
  judge: DecisionView["judge"],
  output: unknown,
  reason: string,
  basis: string,
  minute: number,
  canOverride = false,
): DecisionView => ({
  id,
  judge,
  subject: null,
  input_summary: basis,
  output,
  reason,
  overridden: false,
  can_override: canOverride,
  at: at(minute),
});

const classify = (id: string, category: string, projects: string[], reason: string, minute: number) =>
  decision(
    `d_${id}_classify`,
    "classify",
    { category, projects },
    reason,
    `Request text and ${projects.length} project${projects.length === 1 ? "" : "s"}`,
    minute,
  );

const phaseView = (
  id: number,
  project: string,
  title: string,
  status: PhaseView["status"],
  extra: Partial<PhaseView["info"]> = {},
  reviews = 0,
): PhaseView => ({
  info: {
    id,
    deliverable: null,
    project,
    title,
    complexity: "medium",
    test_policy: "Required",
    depends_on: id === 1 ? [] : [id - 1],
    file: null,
    test_rationale: null,
    ...extra,
  },
  status,
  review_iterations: reviews,
  tests: "none",
  security_block: false,
});

const research = (id: string, project: string, topic: string): ArtifactRef => ({
  path: `${sroot(id)}/${project}/ostra-research-20260922-${topic}.md`,
  kind: "research",
  label: `Research: ${topic.replace(/-/g, " ")}`,
  project,
});
const specRef = (id: string, slug: string): ArtifactRef => ({
  path: `${sroot(id)}/ostra-spec-20260922-${slug}.md`,
  kind: "spec",
  label: `Spec: ${slug.replace(/-/g, " ")}`,
  project: null,
});
const planRef = (id: string, slug: string): ArtifactRef => ({
  path: `${sroot(id)}/ostra-plan-20260922-${slug}.md`,
  kind: "plan",
  label: "Master plan",
  project: null,
});

const explore = (id: string, project: string, start: number, cost = 0.21): ExecSpec => ({
  id,
  agent: "explore",
  stage: "explore",
  project,
  status: "ok",
  start,
  end: start + 4,
  purpose: { kind: "explore", task: 0 },
  run: "Research",
  cost,
});

const reviewFinding = {
  severity: "HIGH" as const,
  file: "src/main/java/shop/admin/OrderSearchRepository.java",
  rule: "C7",
  description: "The search query concatenates the customer name into SQL on line 52.",
  fix: 'Change `"... LIKE \'" + name + "%\'"` to a bound parameter `LIKE :name` on line 52.',
  guidance: null,
};

const specs: SessionSpec[] = [
  {
    id: "s_export",
    title: "Order export to CSV",
    request: "Let admins export the filtered order list as CSV from the admin orders page.",
    category: "IMPLEMENT",
    status: "waiting",
    lane: "requirements",
    stage_label: "Open questions",
    projects: ["backend", "web"],
    start: 5,
    stages: [
      ["classify", "research", "Classify the request", "done", { detail: "IMPLEMENT" }],
      ["explore", "research", "Explore backend", "done", { project: "backend" }],
      ["explore", "research", "Explore web", "done", { project: "web" }],
      ["sufficiency", "research", "Sufficiency check", "done", { detail: "no extra research" }],
      ["spec", "requirements", "Write the spec", "done", { detail: "v1" }],
      ["open-questions", "requirements", "Answer open questions", "waiting", { detail: "2 questions" }],
    ],
    executions: [
      explore("x_ex1", "backend", 5),
      explore("x_ex2", "web", 5),
      {
        id: "x_ex3",
        agent: "generate-spec",
        stage: "spec",
        project: "backend",
        status: "ok",
        start: 10,
        end: 14,
        purpose: { kind: "spec", round: 1 },
        run: "Spec v1",
        cost: 0.34,
      },
    ],
    gates: [
      {
        id: "g_ex_q",
        title: "Questions about the requirements",
        explanation:
          "The spec could not settle these from the research. Each answer is written back into the spec, then the spec is fact-checked.",
        opened: 14,
        payload: {
          kind: "open_questions",
          artifact: "spec",
          artifact_path: `${sroot("s_export")}/ostra-spec-20260922-order-export.md`,
          questions: [
            {
              id: "Q1",
              question: "How many orders may one export contain?",
              tag: "Limits",
              options: [
                {
                  label: "Up to 10,000 rows",
                  description:
                    "Streams the file directly; the request finishes in about 4 seconds on the staging data.",
                },
                {
                  label: "No limit, as a background job",
                  description: "Adds a job table and an email when the file is ready.",
                },
              ],
              recommended: 0,
              multi_select: false,
            },
            {
              id: "Q2",
              question: "Which columns does the file include?",
              tag: "Columns",
              options: [
                { label: "Order id and status", description: "Always present in the list view." },
                {
                  label: "Customer email",
                  description: "Personal data; the export is then limited to the admin role.",
                },
                { label: "Line items", description: "One row per item instead of one row per order." },
              ],
              recommended: 0,
              multi_select: true,
            },
          ],
        },
      },
    ],
    decisions: [
      classify(
        "export",
        "IMPLEMENT",
        ["backend", "web"],
        "The request adds an endpoint in the backend and a button in the web client.",
        5,
      ),
    ],
    artifacts: [
      research("s_export", "backend", "admin-orders"),
      research("s_export", "web", "orders-page"),
      specRef("s_export", "order-export"),
    ],
  },
  {
    id: "s_guest",
    title: "Guest checkout",
    request: "Allow checkout without an account; guests enter an email and get an order link.",
    category: "IMPLEMENT",
    status: "waiting",
    lane: "verification",
    stage_label: "Spec approval",
    projects: ["backend"],
    start: -120,
    stages: [
      ["classify", "research", "Classify the request", "done", { detail: "IMPLEMENT" }],
      ["explore", "research", "Explore backend", "done", { project: "backend" }],
      ["spec", "requirements", "Write the spec", "done", { detail: "v2" }],
      ["open-questions", "requirements", "Answer open questions", "done", { detail: "1 answered" }],
      ["fact-check-spec", "verification", "Fact-check the spec", "done", { detail: "PASS · 2 passes" }],
      ["spec-approval", "verification", "Approve the spec", "waiting"],
    ],
    executions: [
      explore("x_gu1", "backend", -120, 0.28),
      {
        id: "x_gu2",
        agent: "generate-spec",
        stage: "spec",
        project: "backend",
        status: "ok",
        start: -114,
        end: -110,
        purpose: { kind: "spec", round: 1 },
        run: "Spec v1",
        cost: 0.31,
      },
      {
        id: "x_gu3",
        agent: "fact-check",
        stage: "fact-check-spec",
        project: "backend",
        status: "ok",
        start: -109,
        end: -106,
        purpose: { kind: "fact_check", target: "spec", pass: 1 },
        run: "Spec · pass 1",
        cost: 0.12,
      },
      {
        id: "x_gu4",
        agent: "generate-spec",
        stage: "spec",
        project: "backend",
        status: "ok",
        start: -105,
        end: -102,
        purpose: { kind: "spec", round: 2 },
        run: "Spec v2",
        cost: 0.22,
      },
      {
        id: "x_gu5",
        agent: "fact-check",
        stage: "fact-check-spec",
        project: "backend",
        status: "ok",
        start: -101,
        end: -99,
        purpose: { kind: "fact_check", target: "spec", pass: 2 },
        run: "Spec · pass 2",
        cost: 0.1,
      },
    ],
    gates: [
      {
        id: "g_gu_spec",
        title: "Approve the spec",
        explanation:
          "The spec passed its fact-check. Approve it to continue, or describe what to change. Every change is written back into the spec and checked again.",
        opened: -99,
        payload: {
          kind: "spec_approval",
          spec_path: `${sroot("s_guest")}/ostra-spec-20260922-guest-checkout.md`,
          summary:
            "Six requirements across one deliverable: a guest order model, an email-signed order link, and rate limits on guest checkout.",
          findings: [
            {
              severity: "LOW",
              location: "R5",
              element: null,
              claim: "Order links expire after 30 days",
              issue: "The research names no retention rule for this; the number comes from the request.",
            },
            {
              severity: "LOW",
              location: "External Evidence, row 2",
              element: null,
              claim: "Spring Security 7 supports one-time tokens",
              issue: "Cited page is dated 2026-03; a newer minor release exists.",
            },
          ],
        },
      },
    ],
    decisions: [
      classify("guest", "IMPLEMENT", ["backend"], "The request changes checkout behavior in the backend only.", -120),
      decision(
        "d_guest_suff",
        "sufficiency",
        { items: [{ needed: false }] },
        "The one Not covered item, email deliverability, is outside the request.",
        "1 research document",
        -115,
      ),
    ],
    artifacts: [research("s_guest", "backend", "checkout-flow"), specRef("s_guest", "guest-checkout")],
  },
  {
    id: "s_stock",
    title: "Inventory reservations",
    request: "Reserve stock when an order is placed and release it when the order is cancelled or expires.",
    category: "IMPLEMENT",
    status: "waiting",
    lane: "design",
    stage_label: "Plan approval",
    projects: ["backend", "web"],
    start: -240,
    stages: [
      ["classify", "research", "Classify the request", "done", { detail: "IMPLEMENT" }],
      ["explore", "research", "Explore backend", "done", { project: "backend" }],
      ["spec", "requirements", "Write the spec", "done", { detail: "v1" }],
      ["fact-check-spec", "verification", "Fact-check the spec", "done", { detail: "PASS" }],
      ["spec-approval", "verification", "Approve the spec", "done", { detail: "approved" }],
      ["stakes", "design", "Judge the stakes", "done", { detail: "high" }],
      ["plan", "design", "Plan the phases", "done", { detail: "4 phases" }],
      ["fact-check-plan", "verification", "Fact-check the plan", "done", { detail: "PASS" }],
      ["plan-approval", "design", "Approve the plan", "waiting"],
    ],
    executions: [
      explore("x_st1", "backend", -240),
      {
        id: "x_st2",
        agent: "generate-spec",
        stage: "spec",
        project: "backend",
        status: "ok",
        start: -234,
        end: -230,
        purpose: { kind: "spec", round: 1 },
        run: "Spec v1",
        cost: 0.36,
      },
      {
        id: "x_st3",
        agent: "fact-check",
        stage: "fact-check-spec",
        project: "backend",
        status: "ok",
        start: -229,
        end: -227,
        purpose: { kind: "fact_check", target: "spec", pass: 1 },
        run: "Spec · pass 1",
        cost: 0.11,
      },
      {
        id: "x_st4",
        agent: "plan",
        stage: "plan",
        project: "backend",
        status: "ok",
        start: -220,
        end: -212,
        purpose: { kind: "plan", round: 1 },
        run: "Plan",
        cost: 0.52,
      },
      {
        id: "x_st5",
        agent: "fact-check",
        stage: "fact-check-plan",
        project: "backend",
        status: "ok",
        start: -211,
        end: -209,
        purpose: { kind: "fact_check", target: "plan", pass: 1 },
        run: "Plan · pass 1",
        cost: 0.14,
      },
    ],
    gates: [
      {
        id: "g_st_plan",
        title: "Approve the plan",
        explanation:
          "The plan passed its fact-check. Approve it to start building, or describe what to change. A change goes into the spec first, then a new plan is written (Rule D10).",
        opened: -209,
        payload: {
          kind: "plan_approval",
          plan_path: `${sroot("s_stock")}/ostra-plan-20260922-inventory-reservations.md`,
          summary:
            "Four phases: the reservation table, the reserve and release service, the expiry job, and the web stock badge.",
          phases: [
            {
              id: 1,
              deliverable: "D1",
              project: "backend",
              title: "Reservation table and repository",
              complexity: "low",
              test_policy: "Required",
              depends_on: [],
              file: `${sroot("s_stock")}/ostra-plan-20260922-inventory-reservations-phase-1.md`,
              test_rationale: null,
            },
            {
              id: 2,
              deliverable: "D1",
              project: "backend",
              title: "Reserve and release service",
              complexity: "high",
              test_policy: "Required",
              depends_on: [1],
              file: `${sroot("s_stock")}/ostra-plan-20260922-inventory-reservations-phase-2.md`,
              test_rationale: null,
            },
            {
              id: 3,
              deliverable: "D1",
              project: "backend",
              title: "Reservation expiry job",
              complexity: "medium",
              test_policy: "Required",
              depends_on: [2],
              file: `${sroot("s_stock")}/ostra-plan-20260922-inventory-reservations-phase-3.md`,
              test_rationale: null,
            },
            {
              id: 4,
              deliverable: "D2",
              project: "web",
              title: "Stock badge on the product page",
              complexity: "low",
              test_policy: "Skip",
              depends_on: null,
              file: `${sroot("s_stock")}/ostra-plan-20260922-inventory-reservations-phase-4.md`,
              test_rationale: "Renders one existing field with no branch.",
            },
          ],
          findings: [
            {
              severity: "LOW",
              location: "Phase 3, step 2",
              element: null,
              claim: "The scheduler runs every minute",
              issue: "The existing @Scheduled jobs use a 5 minute cron; the plan does not say why this one differs.",
            },
          ],
        },
      },
    ],
    decisions: [
      classify(
        "stock",
        "IMPLEMENT",
        ["backend", "web"],
        "The request changes the order flow in the backend and shows stock in the web client.",
        -240,
      ),
      decision(
        "d_stock_stakes",
        "stakes",
        { stakes: "high" },
        "The change adds a table, a scheduled job, and a lock on the stock row that every order touches.",
        "Approved spec, 9 requirements",
        -213,
        true,
      ),
    ],
    artifacts: [
      research("s_stock", "backend", "stock-model"),
      specRef("s_stock", "inventory-reservations"),
      planRef("s_stock", "inventory-reservations"),
    ],
  },
  {
    id: "s_payments",
    title: "Payment provider switch",
    request: "Move card payments from the old gateway to the new provider's Payment Intents API.",
    category: "IMPLEMENT",
    status: "waiting",
    lane: "verification",
    stage_label: "Fact-check the spec, pass 3",
    projects: ["backend"],
    start: -420,
    stages: [
      ["classify", "research", "Classify the request", "done", { detail: "IMPLEMENT" }],
      ["explore", "research", "Explore backend", "done", { project: "backend" }],
      ["spec", "requirements", "Write the spec", "done", { detail: "v3" }],
      ["fact-check-spec", "verification", "Fact-check the spec", "waiting", { detail: "FAIL · 3 passes" }],
    ],
    executions: [
      explore("x_pa1", "backend", -420, 0.44),
      {
        id: "x_pa2",
        agent: "generate-spec",
        stage: "spec",
        project: "backend",
        status: "ok",
        start: -410,
        end: -405,
        purpose: { kind: "spec", round: 1 },
        run: "Spec v1",
        cost: 0.3,
      },
      {
        id: "x_pa3",
        agent: "fact-check",
        stage: "fact-check-spec",
        project: "backend",
        status: "ok",
        start: -404,
        end: -401,
        purpose: { kind: "fact_check", target: "spec", pass: 1 },
        run: "Spec · pass 1",
        cost: 0.16,
      },
      {
        id: "x_pa4",
        agent: "generate-spec",
        stage: "spec",
        project: "backend",
        status: "ok",
        start: -400,
        end: -396,
        purpose: { kind: "spec", round: 2 },
        run: "Spec v2",
        cost: 0.24,
      },
      {
        id: "x_pa5",
        agent: "fact-check",
        stage: "fact-check-spec",
        project: "backend",
        status: "ok",
        start: -395,
        end: -392,
        purpose: { kind: "fact_check", target: "spec", pass: 2 },
        run: "Spec · pass 2",
        cost: 0.15,
      },
      {
        id: "x_pa6",
        agent: "generate-spec",
        stage: "spec",
        project: "backend",
        status: "ok",
        start: -391,
        end: -388,
        purpose: { kind: "spec", round: 3 },
        run: "Spec v3",
        cost: 0.22,
      },
      {
        id: "x_pa7",
        agent: "fact-check",
        stage: "fact-check-spec",
        project: "backend",
        status: "ok",
        start: -387,
        end: -384,
        purpose: { kind: "fact_check", target: "spec", pass: 3 },
        run: "Spec · pass 3",
        cost: 0.15,
      },
    ],
    gates: [
      {
        id: "g_pa_fc",
        title: "The spec fact-check keeps failing",
        explanation:
          "The fact-check has failed 3 times in a row. Another round may not converge. Choose whether to run another round, add guidance, or stop.",
        opened: -384,
        payload: {
          kind: "fact_check_recurring",
          target: "spec",
          passes: 3,
          findings: [
            {
              severity: "HIGH",
              location: "R3",
              element: null,
              claim: "The provider refunds a Payment Intent with POST /v1/intents/{id}/refund",
              issue: "The cited reference page documents refunds as POST /v1/refunds with a payment_intent field.",
            },
            {
              severity: "MEDIUM",
              location: "External Evidence, row 1",
              element: null,
              claim: "Webhook signatures use HMAC-SHA1",
              issue: "The provider's current page says HMAC-SHA256.",
            },
          ],
        },
      },
    ],
    decisions: [
      classify("pay", "IMPLEMENT", ["backend"], "The request replaces the payment client in the backend.", -420),
    ],
    artifacts: [research("s_payments", "backend", "payment-gateway"), specRef("s_payments", "payment-provider")],
  },
  {
    id: "s_search",
    title: "Admin order search",
    request: "Add a free-text search box to the admin order list that matches customer name, email and order id.",
    category: "IMPLEMENT",
    status: "waiting",
    lane: "review",
    stage_label: "Phase 1 review cap",
    projects: ["backend"],
    start: -300,
    stages: [
      ["classify", "research", "Classify the request", "done", { detail: "IMPLEMENT" }],
      ["explore", "research", "Explore backend", "done", { project: "backend" }],
      ["spec", "requirements", "Write the spec", "done"],
      ["fact-check-spec", "verification", "Fact-check the spec", "done", { detail: "PASS" }],
      ["spec-approval", "verification", "Approve the spec", "done", { detail: "approved" }],
      ["stakes", "design", "Judge the stakes", "done", { detail: "low, plan skipped" }],
      ["implement", "build", "Phase 1", "done", { project: "backend", phase: 1 }],
      ["review", "review", "Review phase 1", "waiting", { project: "backend", phase: 1, detail: "pass 3 of 3" }],
    ],
    executions: [
      explore("x_se1", "backend", -300),
      {
        id: "x_se2",
        agent: "generate-spec",
        stage: "spec",
        project: "backend",
        status: "ok",
        start: -294,
        end: -291,
        purpose: { kind: "spec", round: 1 },
        run: "Spec v1",
        cost: 0.2,
      },
      {
        id: "x_se3",
        agent: "fact-check",
        stage: "fact-check-spec",
        project: "backend",
        status: "ok",
        start: -290,
        end: -288,
        purpose: { kind: "fact_check", target: "spec", pass: 1 },
        run: "Spec · pass 1",
        cost: 0.09,
      },
      {
        id: "x_se4",
        agent: "implementer",
        stage: "implement",
        project: "backend",
        status: "ok",
        start: -280,
        end: -272,
        purpose: { kind: "implement", phase: 1, work: "initial" },
        run: "Phase 1",
        cost: 0.38,
        extra: { executor: "harness:codex", model: "gpt-5.6-luna" },
      },
      {
        id: "x_se5",
        agent: "code-reviewer",
        stage: "review",
        project: "backend",
        status: "ok",
        start: -271,
        end: -268,
        purpose: { kind: "review", phase: 1, tests: false, iteration: 1 },
        run: "Phase 1 · pass 1",
        cost: 0.12,
      },
      {
        id: "x_se6",
        agent: "implementer",
        stage: "implement",
        project: "backend",
        status: "ok",
        start: -267,
        end: -262,
        purpose: { kind: "implement", phase: 1, work: "fix" },
        run: "Phase 1 · fix pass",
        cost: 0.21,
        extra: { executor: "harness:codex", model: "gpt-5.6-luna" },
      },
      {
        id: "x_se7",
        agent: "code-reviewer",
        stage: "review",
        project: "backend",
        status: "ok",
        start: -261,
        end: -258,
        purpose: { kind: "review", phase: 1, tests: false, iteration: 2 },
        run: "Phase 1 · pass 2",
        cost: 0.11,
      },
      {
        id: "x_se8",
        agent: "code-reviewer",
        stage: "review",
        project: "backend",
        status: "ok",
        start: -250,
        end: -247,
        purpose: { kind: "review", phase: 1, tests: false, iteration: 3 },
        run: "Phase 1 · pass 3",
        cost: 0.11,
      },
    ],
    gates: [
      {
        id: "g_se_cap",
        title: "Review of phase 1 reached its cap",
        explanation:
          "3 review passes ran and 2 findings are still open. Choose another fix-and-review pass, or stop and leave the phase blocked.",
        opened: -247,
        payload: {
          kind: "review_cap",
          project: "backend",
          phase: 1,
          tests: false,
          iterations: 3,
          ledger_path: `${sroot("s_search")}/backend/ostra-review-ledger-phase-1.md`,
          findings: [
            reviewFinding,
            {
              severity: "MEDIUM",
              file: "src/main/java/shop/admin/AdminOrderController.java",
              rule: "PHASE-REQ-3",
              description:
                "The requirement asks for a 300 ms debounce, but the endpoint accepts every keystroke without a minimum length.",
              fix: "Add `if (q.length() < 2) return Page.empty();` above line 30: `var page = search.find(q, pageable);`",
              guidance: null,
            },
          ],
        },
      },
    ],
    phases: [phaseView(1, "backend", "Order search endpoint", "reviewing", { file: null }, 3)],
    decisions: [
      classify("search", "IMPLEMENT", ["backend"], "The request adds a query endpoint in the backend.", -300),
      decision(
        "d_search_stakes",
        "stakes",
        { stakes: "low" },
        "One endpoint and one repository method change, with no migration.",
        "Approved spec, 3 requirements",
        -287,
      ),
    ],
    artifacts: [
      research("s_search", "backend", "admin-orders"),
      specRef("s_search", "admin-order-search"),
      {
        path: `${sroot("s_search")}/backend/ostra-review-ledger-phase-1.md`,
        kind: "ledger",
        label: "Review ledger, phase 1",
        project: "backend",
      },
    ],
    extraEvents: [
      [
        -258,
        {
          type: "security_block",
          project: "backend",
          phase: 1,
          tests: false,
          findings: [
            {
              severity: "BLOCKER",
              file: "src/main/java/shop/admin/OrderSearchRepository.java",
              rule: "SEC-BLOCK-SQLI",
              description: "User input from the search box reaches a native SQL string without a bound parameter.",
              fix: "Remove the string concatenation on line 52.",
              guidance:
                "Any admin who types a quote can read or change other tables. Research parameter binding for Spring Data native queries before writing the replacement.",
            },
          ],
        },
      ],
    ],
  },
  {
    id: "s_labels",
    title: "Shipping label printing",
    request:
      "Print shipping labels from the packing screen through the carrier's label API, and store the tracking number.",
    category: "IMPLEMENT",
    status: "waiting",
    lane: "build",
    stage_label: "Phase 2 stuck",
    projects: ["backend", "web"],
    start: -600,
    stages: [
      ["classify", "research", "Classify the request", "done", { detail: "IMPLEMENT" }],
      ["explore", "research", "Explore backend", "done", { project: "backend" }],
      ["spec", "requirements", "Write the spec", "done"],
      ["fact-check-spec", "verification", "Fact-check the spec", "done", { detail: "PASS" }],
      ["spec-approval", "verification", "Approve the spec", "done", { detail: "approved" }],
      ["plan", "design", "Plan the phases", "done", { detail: "3 phases" }],
      ["plan-approval", "design", "Approve the plan", "done", { detail: "approved" }],
      ["implement", "build", "Phase 1", "done", { project: "backend", phase: 1 }],
      ["review", "review", "Review phase 1", "done", { project: "backend", phase: 1, detail: "passed" }],
      ["implement", "build", "Phase 2", "failed", { project: "backend", phase: 2, detail: "STUCK" }],
      ["rescue", "build", "Rescue phase 2", "waiting", { project: "backend", phase: 2 }],
      ["implement", "build", "Phase 3", "blocked", { project: "web", phase: 3, detail: "blocked" }],
    ],
    executions: [
      explore("x_la1", "backend", -600),
      {
        id: "x_la2",
        agent: "plan",
        stage: "plan",
        project: "backend",
        status: "ok",
        start: -580,
        end: -572,
        purpose: { kind: "plan", round: 1 },
        run: "Plan",
        cost: 0.47,
      },
      {
        id: "x_la3",
        agent: "implementer",
        stage: "implement",
        project: "backend",
        status: "ok",
        start: -560,
        end: -552,
        purpose: { kind: "implement", phase: 1, work: "initial" },
        run: "Phase 1",
        cost: 0.33,
        extra: { executor: "harness:codex", model: "gpt-5.6-luna" },
      },
      {
        id: "x_la4",
        agent: "code-reviewer",
        stage: "review",
        project: "backend",
        status: "ok",
        start: -551,
        end: -548,
        purpose: { kind: "review", phase: 1, tests: false, iteration: 1 },
        run: "Phase 1 · pass 1",
        cost: 0.1,
      },
      {
        id: "x_la5",
        agent: "implementer",
        stage: "implement",
        project: "backend",
        status: "stuck",
        start: -547,
        end: -530,
        purpose: { kind: "implement", phase: 2, work: "initial" },
        run: "Phase 2",
        cost: 0.61,
        extra: {
          executor: "harness:codex",
          model: "gpt-5.6-luna",
          has_transcript: true,
          summary: "Returned STUCK after 5 failed builds",
        },
      },
      {
        id: "x_la6",
        agent: "implementer",
        stage: "implement",
        project: "web",
        status: "error",
        start: -545,
        end: -540,
        purpose: { kind: "implement", phase: 3, work: "initial" },
        run: "Phase 3",
        cost: 0.18,
        extra: { executor: "harness:claude", model: "haiku" },
      },
    ],
    gates: [
      {
        id: "g_la_stuck",
        title: "Phase 2 is stuck",
        explanation:
          "The agent hit its retry ceiling on the same failure and needs a fact only you can give. State the missing fact, or leave the phase blocked.",
        opened: -529,
        payload: {
          kind: "stuck",
          execution: "x_la5",
          agent: "implementer",
          project: "backend",
          phase: 2,
          diagnostic:
            "./mvnw -q compile\n[ERROR] LabelClient.java:[41,17] cannot find symbol\n  symbol:   class LabelRequest\n  location: package com.carrier.sdk.v3\n[ERROR] 1 error",
          need: "Which carrier SDK version the project should use: the research cites v3, the lock file pins v2.8.",
        },
      },
      {
        id: "g_la_blocked",
        title: "Phase 3 is blocked",
        explanation:
          "Phases that depend on it are removed from the queue; independent phases keep running. Retry with instructions, or leave it blocked.",
        opened: -539,
        payload: {
          kind: "phase_blocked",
          project: "web",
          phase: 3,
          reason:
            "the implementer run ended with an error twice and the label preview component it needs does not exist yet.",
        },
      },
    ],
    phases: [
      phaseView(1, "backend", "Carrier client and credentials", "passed", { complexity: "medium" }, 1),
      phaseView(2, "backend", "Label endpoint and tracking number", "blocked", { complexity: "high" }),
      phaseView(3, "web", "Print button on the packing screen", "blocked", {
        complexity: "low",
        test_policy: "Skip",
        depends_on: [],
        test_rationale: "Wires one button to an existing API call.",
      }),
    ],
    decisions: [
      classify(
        "labels",
        "IMPLEMENT",
        ["backend", "web"],
        "The request calls a carrier API from the backend and adds a button in the web client.",
        -600,
      ),
      decision(
        "d_labels_rescue",
        "rescue",
        { action: "gate" },
        "The failure needs a version choice that neither the research nor the plan makes.",
        "STUCK report from the implementer, phase 2",
        -530,
      ),
    ],
    artifacts: [
      research("s_labels", "backend", "carrier-api"),
      specRef("s_labels", "shipping-labels"),
      planRef("s_labels", "shipping-labels"),
    ],
  },
  {
    id: "s_receipts",
    title: "Email receipts",
    request: "Send an HTML email receipt after payment succeeds, using the existing mail templates.",
    category: "IMPLEMENT",
    status: "waiting",
    lane: "build",
    stage_label: "Implement phase 1",
    projects: ["backend", "web"],
    start: -90,
    stages: [
      ["classify", "research", "Classify the request", "done", { detail: "IMPLEMENT" }],
      ["explore", "research", "Explore backend", "done", { project: "backend" }],
      ["spec", "requirements", "Write the spec", "done"],
      ["fact-check-spec", "verification", "Fact-check the spec", "done", { detail: "PASS" }],
      ["spec-approval", "verification", "Approve the spec", "done", { detail: "approved" }],
      ["stakes", "design", "Judge the stakes", "done", { detail: "low, plan skipped" }],
      [
        "implement",
        "build",
        "Phase 1",
        "waiting",
        { project: "backend", phase: 1, detail: "harness failed", gate: "g_rc_harness" },
      ],
      [
        "implement",
        "build",
        "Phase 2",
        "waiting",
        { project: "web", phase: 2, detail: "execution failed", gate: "g_rc_failed" },
      ],
    ],
    executions: [
      explore("x_rc1", "backend", -90),
      {
        id: "x_rc2",
        agent: "implementer",
        stage: "implement",
        project: "backend",
        status: "error",
        start: -70,
        end: -70,
        purpose: { kind: "implement", phase: 1, work: "initial" },
        run: "Phase 1",
        cost: 0,
        extra: {
          executor: "harness:claude",
          model: "sonnet",
          error: "claude exited with status 1: not logged in",
          has_transcript: true,
        },
      },
      {
        id: "x_rc3",
        agent: "implementer",
        stage: "implement",
        project: "web",
        status: "error",
        start: -68,
        end: -61,
        purpose: { kind: "implement", phase: 2, work: "initial" },
        run: "Phase 2",
        cost: 0.19,
        extra: { error: "The provider closed the stream after 3 retries (HTTP 529, overloaded)." },
      },
    ],
    gates: [
      {
        id: "g_rc_harness",
        title: "Claude Code could not run",
        explanation:
          "The harness failed to start or is not logged in. Log in from its terminal and retry, or run this execution on the native executor.",
        opened: -70,
        payload: {
          kind: "harness_failure",
          execution: "x_rc2",
          harness: "claude",
          error: "claude exited with status 1: Invalid API key. Please run /login.",
        },
      },
      {
        id: "g_rc_failed",
        title: "Implementer failed",
        explanation: "The execution ended without a usable result. Retry it, or abandon this step.",
        opened: -61,
        payload: {
          kind: "execution_failed",
          execution: "x_rc3",
          agent: "implementer",
          project: "web",
          error: "The provider closed the stream after 3 retries (HTTP 529, overloaded).",
        },
      },
    ],
    phases: [
      phaseView(1, "backend", "Receipt mail after payment", "implementing", { file: null, complexity: "low" }),
      phaseView(2, "web", "Receipt link on the order page", "implementing", {
        file: null,
        complexity: "low",
        depends_on: [],
      }),
    ],
    decisions: [
      classify(
        "receipts",
        "IMPLEMENT",
        ["backend", "web"],
        "The request adds a mail in the backend and a link in the web client.",
        -90,
      ),
    ],
    artifacts: [research("s_receipts", "backend", "mail-templates"), specRef("s_receipts", "email-receipts")],
  },
  {
    id: "s_images",
    title: "Product image resizing",
    request: "Generate 3 thumbnail sizes for every uploaded product image and serve them from the CDN path.",
    category: "IMPLEMENT",
    status: "waiting",
    lane: "build",
    stage_label: "Budget reached",
    projects: ["backend"],
    start: -200,
    stages: [
      ["classify", "research", "Classify the request", "done", { detail: "IMPLEMENT" }],
      ["explore", "research", "Explore backend", "done", { project: "backend" }],
      ["spec", "requirements", "Write the spec", "done"],
      ["fact-check-spec", "verification", "Fact-check the spec", "done", { detail: "PASS" }],
      ["spec-approval", "verification", "Approve the spec", "done", { detail: "approved" }],
      ["plan", "design", "Plan the phases", "done", { detail: "2 phases" }],
      ["plan-approval", "design", "Approve the plan", "done", { detail: "approved" }],
      ["implement", "build", "Phase 1", "done", { project: "backend", phase: 1 }],
      ["review", "review", "Review phase 1", "done", { project: "backend", phase: 1, detail: "passed" }],
      ["implement", "build", "Phase 2", "pending", { project: "backend", phase: 2, detail: "waiting on budget" }],
    ],
    executions: [
      explore("x_im1", "backend", -200, 0.9),
      {
        id: "x_im2",
        agent: "generate-spec",
        stage: "spec",
        project: "backend",
        status: "ok",
        start: -190,
        end: -184,
        purpose: { kind: "spec", round: 1 },
        run: "Spec v1",
        cost: 0.8,
      },
      {
        id: "x_im3",
        agent: "plan",
        stage: "plan",
        project: "backend",
        status: "ok",
        start: -180,
        end: -170,
        purpose: { kind: "plan", round: 1 },
        run: "Plan",
        cost: 1.1,
      },
      {
        id: "x_im4",
        agent: "implementer",
        stage: "implement",
        project: "backend",
        status: "ok",
        start: -160,
        end: -140,
        purpose: { kind: "implement", phase: 1, work: "initial" },
        run: "Phase 1",
        cost: 1.6,
      },
      {
        id: "x_im5",
        agent: "code-reviewer",
        stage: "review",
        project: "backend",
        status: "ok",
        start: -139,
        end: -135,
        purpose: { kind: "review", phase: 1, tests: false, iteration: 1 },
        run: "Phase 1 · pass 1",
        cost: 0.7,
      },
    ],
    gates: [
      {
        id: "g_im_budget",
        title: "The session reached its budget",
        explanation:
          "This session has spent $5.10 of its $5.00 budget, so no new execution starts. Raise the budget to continue, or stop the session.",
        opened: -135,
        payload: { kind: "budget_reached", spent_usd: 5.1, budget_usd: 5 },
      },
    ],
    phases: [
      phaseView(1, "backend", "Thumbnail generator", "passed", {}, 1),
      phaseView(2, "backend", "CDN paths in the product API", "queued"),
    ],
    decisions: [
      classify("images", "IMPLEMENT", ["backend"], "The request adds an image pipeline in the backend.", -200),
    ],
    artifacts: [
      research("s_images", "backend", "media-storage"),
      specRef("s_images", "image-resizing"),
      planRef("s_images", "image-resizing"),
    ],
  },
  {
    id: "s_init",
    title: "Initialize web",
    request: "Initialize project web",
    category: null,
    kind: { kind: "init", project: "web" },
    status: "waiting",
    lane: "design",
    stage_label: "Skill approval",
    projects: ["web"],
    start: 40,
    stages: [
      ["detect", "research", "Detect the stack", "done", { project: "web", detail: "typescript-node" }],
      ["scout", "research", "Scout 3 slices", "done", { project: "web", detail: "3 slices" }],
      ["propose", "design", "Propose skills", "done", { project: "web", detail: "5 skills" }],
      ["skill-approval", "design", "Approve the skills", "waiting", { project: "web" }],
      ["generate-skill", "build", "Generate skills", "pending", { project: "web" }],
      ["generate-inventory", "build", "Generate inventory", "pending", { project: "web" }],
    ],
    executions: [
      {
        id: "x_in1",
        agent: "initializer",
        stage: "detect",
        project: "web",
        status: "ok",
        start: 40,
        end: 41,
        purpose: { kind: "init", mode: "detect", item: null },
        run: "Detect",
        cost: 0.03,
      },
      {
        id: "x_in2",
        agent: "initializer",
        stage: "scout",
        project: "web",
        status: "ok",
        start: 41,
        end: 44,
        purpose: { kind: "init", mode: "scout", item: "src/components" },
        run: "Scout · src/components",
        cost: 0.05,
      },
      {
        id: "x_in3",
        agent: "initializer",
        stage: "scout",
        project: "web",
        status: "ok",
        start: 41,
        end: 44,
        purpose: { kind: "init", mode: "scout", item: "src/api" },
        run: "Scout · src/api",
        cost: 0.04,
      },
      {
        id: "x_in4",
        agent: "initializer",
        stage: "scout",
        project: "web",
        status: "ok",
        start: 41,
        end: 45,
        purpose: { kind: "init", mode: "scout", item: "src/pages" },
        run: "Scout · src/pages",
        cost: 0.04,
      },
      {
        id: "x_in5",
        agent: "initializer",
        stage: "propose",
        project: "web",
        status: "ok",
        start: 45,
        end: 47,
        purpose: { kind: "init", mode: "propose", item: null },
        run: "Propose",
        cost: 0.04,
      },
    ],
    gates: [
      {
        id: "g_in_skills",
        title: "Approve the skills",
        explanation:
          "Each skill is grounded in a real exemplar the scouts captured. Choose, per skill, whether to generate it, regenerate an existing one from the current code, reuse it as it is, or drop it.",
        opened: 47,
        payload: {
          kind: "skill_approval",
          project: "web",
          skills: [
            {
              name: "react-component",
              kind: "creation",
              description: "Adds a function component with its CSS module and story.",
              disposition: "generate",
              exemplars: ["src/components/OrderActions.tsx", "src/components/Price.tsx"],
            },
            {
              name: "api-client-call",
              kind: "creation",
              description: "Adds a typed fetch wrapper in src/api with its error mapping.",
              disposition: "generate",
              exemplars: ["src/api/orders.ts"],
            },
            {
              name: "page-route",
              kind: "creation",
              description: "Adds a routed page with its loader.",
              disposition: "generate",
              exemplars: ["src/pages/OrderPage.tsx"],
            },
            {
              name: "web-conventions",
              kind: "convention",
              description: "Naming, imports, and state rules for the web client.",
              disposition: "regenerate",
              exemplars: [],
            },
            {
              name: "module-hub",
              kind: "module-hub",
              description: "The area map every agent routes by.",
              disposition: "reuse",
              exemplars: [],
            },
          ],
        },
      },
    ],
  },
];

export const gateSessions: SessionDetail[] = specs.map(build);

const extraEvents = new Map(specs.map((s) => [s.id, s.extraEvents ?? []]));

/** A plausible event log for a session detail: creation, decisions, executions, gates, plus fixture extras. */
export function eventsFor(d: SessionDetail): StoredEvent[] {
  const out: { at: string; event: SessionEvent }[] = [];
  const s = d.summary;
  out.push({
    at: s.created_at,
    event: {
      type: "session_created",
      kind: s.kind,
      request: s.request,
      options: { tests: false, docs: false, yolo: s.yolo },
      projects: s.projects.map((key) => ({ key, path: `/home/me/code/shop-${key}` })),
      workspace_root: ROOT,
      session_root: d.session_root,
      files: d.files,
      uploads: d.uploads,
    },
  });
  for (const x of d.decisions)
    out.push({
      at: x.at,
      event: {
        type: "decision_made",
        id: x.id,
        judge: x.judge,
        subject: x.subject,
        input_summary: x.input_summary,
        output: x.output,
        reason: x.reason,
      },
    });
  for (const x of d.executions) {
    out.push({
      at: x.started_at,
      event: {
        type: "execution_started",
        id: x.id,
        agent: x.agent,
        purpose: x.purpose!,
        stage: x.stage!,
        project: x.project,
        executor: x.executor,
        model: x.model,
        params: {},
        spawn_block: x.spawn_block,
        report_path: x.report_path,
        resumes: null,
      },
    });
    if (x.ended_at)
      out.push({
        at: x.ended_at,
        event: {
          type: "execution_finished",
          id: x.id,
          result: {
            status: x.status,
            submit: null,
            final_text: "",
            usage: x.usage,
            native_session_id: null,
            error: x.error,
          },
        },
      });
  }
  for (const g of d.gates) {
    out.push({
      at: g.opened_at,
      event: { type: "gate_opened", id: g.id, title: g.title, explanation: g.explanation, payload: g.payload },
    });
    if (g.answer && g.answered_at)
      out.push({
        at: g.answered_at,
        event: {
          type: "gate_answered",
          id: g.id,
          source: g.source ?? "user",
          answer: g.answer,
          reason: g.reason,
          routed: false,
        },
      });
  }
  for (const [minute, event] of extraEvents.get(s.id) ?? []) out.push({ at: at(minute), event });
  return out.sort((a, b) => a.at.localeCompare(b.at)).map((e, i) => ({ seq: i + 1, ...e }));
}
