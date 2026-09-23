import type {
  ActivityItem,
  CostReport,
  CostRow,
  DecisionView,
  ExecutionView,
  GateView,
  Lesson,
  PhaseInfo,
  SessionDetail,
  SessionSummary,
  Usage,
  WorkspaceDetail,
  WorkspaceSettings,
  WorkspaceSummary,
} from "../types";

const now = new Date("2026-09-22T10:00:00Z");
const at = (minutes: number) => new Date(now.getTime() + minutes * 60000).toISOString();

export const WS = "ws_demo";
export const SESSION = "s_demo";
const ROOT = "/home/me/code/shop";
const SROOT = `${ROOT}/.ostra/sessions/${SESSION}`;

const usage = (i: number, cost: number): Usage => ({
  input_tokens: 12000 * i,
  output_tokens: 1800 * i,
  cache_read_tokens: 90000 * i,
  cache_write_tokens: 8000 * i,
  cost_usd: cost,
  tool_calls: 14 * i,
  build_ms: 4200 * i,
});

export const settings: WorkspaceSettings = {
  name: "shop",
  projects: [
    { key: "backend", path: "/home/me/code/shop-backend", stack: "java-spring" },
    { key: "web", path: "/home/me/code/shop-web", stack: "typescript-node" },
  ],
  routing: {
    effort: {},
    executor: {
      byAgent: { implementer: "harness:codex", "write-test": "harness:codex" },
      byPhaseComplexity: { implementer: { low: "harness:codex", medium: "harness:codex", high: "native" } },
    },
    model: {
      byAgent: {
        explore: "advanced",
        "generate-spec": "advanced",
        plan: "advanced",
        "fact-check": "advanced",
        "code-reviewer": "balanced",
        "execution-path-analyzer": "balanced",
        "module-documentation": "advanced",
        "prompt-generation": "advanced",
        initializer: "balanced",
        judge: "fast",
        "quick-answer": "balanced",
      },
      byPhaseComplexity: {
        implementer: { low: "fast", medium: "fast", high: "balanced" },
        "write-test": { low: "fast", medium: "fast", high: "balanced" },
      },
    },
  },
  instructions: { all: "Write British English in comments and docs.", agents: { implementer: "Keep functions under 40 lines." } },
  yolo: { default: false },
  permissions: { mode: "default", allow: ["Bash(./mvnw *)", "Bash(npm run test *)"], ask: [], deny: ["Bash(git push *)"] },
  notifications: { push: true },
  limits: { max_parallel_executions: 3, session_budget_usd: 25 },
};

export const workspaces: WorkspaceSummary[] = [
  { id: WS, name: "shop", root: ROOT, projects: 2, active_sessions: 1, available: true },
  { id: "ws_notes", name: "notes", root: "/home/me/code/notes", projects: 0, active_sessions: 0, available: true },
];

export const workspaceDetail: WorkspaceDetail = {
  id: WS,
  root: ROOT,
  settings,
  projects: [
    {
      key: "backend",
      path: "/home/me/code/shop-backend",
      init_status: "initialized",
      ultracode_bootstrap: false,
      is_git: true,
      stack: "java-spring",
      profile: null,
    },
    {
      key: "web",
      path: "/home/me/code/shop-web",
      init_status: "not_initialized",
      ultracode_bootstrap: true,
      is_git: true,
      stack: "typescript-node",
      profile: null,
    },
  ],
  harnesses: [
    { harness: "claude", command: "claude", installed: true, version: "2.1.280", logged_in: true },
    { harness: "codex", command: "codex", installed: true, version: "0.153.4", logged_in: true },
    { harness: "grok", command: "grok", installed: true, version: "1.0.30", logged_in: null },
    { harness: "agy", command: "agy", installed: false, version: null, logged_in: null },
  ],
  providers: [
    { name: "anthropic", has_key: true, source: "env:ANTHROPIC_API_KEY" },
    { name: "openai", has_key: false, source: "none" },
  ],
  validation: [],
};

export const phases: PhaseInfo[] = [
  {
    id: 1,
    deliverable: "D1",
    project: "backend",
    title: "Order cancellation data layer",
    complexity: "medium",
    test_policy: "Required",
    depends_on: [],
    file: `${SROOT}/ostra-plan-20260922-100500-order-cancel-phase-1-data-layer.md`,
    test_rationale: null,
  },
  {
    id: 2,
    deliverable: "D1",
    project: "backend",
    title: "Cancellation service and endpoint",
    complexity: "high",
    test_policy: "Required",
    depends_on: [1],
    file: `${SROOT}/ostra-plan-20260922-100500-order-cancel-phase-2-service.md`,
    test_rationale: null,
  },
  {
    id: 3,
    deliverable: "D2",
    project: "web",
    title: "Cancellation request types",
    complexity: "low",
    test_policy: "Skip",
    depends_on: [2],
    file: `${SROOT}/ostra-plan-20260922-100500-order-cancel-phase-3-types.md`,
    test_rationale: "Declares two request types and one enum member with no branch to cover.",
  },
];

export const sessionSummary: SessionSummary = {
  id: SESSION,
  workspace: WS,
  kind: { kind: "pipeline" },
  request: "Add order cancellation: customers can cancel an order until it ships, and the web client shows a Cancel button.",
  category: "IMPLEMENT",
  status: "waiting",
  lane: "build",
  stage_label: "Phase 2 review, pass 2 of 3",
  yolo: false,
  open_gates: 2,
  projects: ["backend", "web"],
  cost_usd: 3.42,
  created_at: at(0),
  updated_at: at(48),
};

export const sessions: SessionSummary[] = [
  sessionSummary,
  {
    ...sessionSummary,
    id: "s_research",
    request: "How does the refund flow publish events?",
    category: "RESEARCH",
    status: "completed",
    lane: "done",
    stage_label: "Completed",
    open_gates: 0,
    projects: ["backend"],
    cost_usd: 0.41,
    created_at: at(-600),
    updated_at: at(-590),
  },
  {
    ...sessionSummary,
    id: "s_init",
    kind: { kind: "init", project: "web" },
    request: "Initialize project web",
    category: null,
    status: "running",
    lane: "research",
    stage_label: "Scouting 3 slices",
    open_gates: 0,
    projects: ["web"],
    cost_usd: 0.2,
    created_at: at(40),
    updated_at: at(47),
  },
];

const exec = (
  id: string,
  agent: ExecutionView["agent"],
  stage: ExecutionView["stage"],
  project: string,
  status: ExecutionView["status"],
  start: number,
  end: number | null,
  purpose: ExecutionView["purpose"],
  extra: Partial<ExecutionView> = {},
): ExecutionView => ({
  id,
  session: SESSION,
  agent,
  purpose,
  stage,
  project,
  executor: "native",
  model: "anthropic:claude-opus-5-5",
  status,
  started_at: at(start),
  ended_at: end === null ? null : at(end),
  usage: usage(1, 0.3),
  report_path: null,
  native_session_id: null,
  spawn_block: `Workspace root: ${ROOT}\nRepo root: /home/me/code/shop-${project}\nSession dir: ${SROOT}/${project}\nRepo key: ${project}`,
  error: null,
  can_resume: false,
  has_terminal: false,
  ...extra,
});

export const executions: ExecutionView[] = [
  exec("x_exp1", "explore", "explore", "backend", "ok", 1, 6, { kind: "explore", task: 0 }),
  exec("x_exp2", "explore", "explore", "web", "ok", 1, 5, { kind: "explore", task: 1 }),
  exec("x_spec", "generate-spec", "spec", "backend", "ok", 7, 12, { kind: "spec", round: 1 }),
  exec("x_fc1", "fact-check", "fact-check-spec", "backend", "ok", 14, 17, { kind: "fact_check", target: "spec", pass: 1 }),
  exec("x_plan", "plan", "plan", "backend", "ok", 20, 26, { kind: "plan", round: 1 }),
  exec("x_fc2", "fact-check", "fact-check-plan", "backend", "ok", 26, 29, { kind: "fact_check", target: "plan", pass: 1 }),
  exec("x_imp1", "implementer", "implement", "backend", "ok", 31, 36, { kind: "implement", phase: 1, work: "initial" }, {
    executor: "harness:codex",
    model: "gpt-5.6-luna",
    native_session_id: "019a-codex-thread",
    can_resume: true,
    report_path: `${SROOT}/backend/ostra-implementer-phase-1.md`,
  }),
  exec("x_rev1", "code-reviewer", "review", "backend", "ok", 36, 38, { kind: "review", phase: 1, tests: false, iteration: 1 }),
  exec("x_imp2", "implementer", "implement", "backend", "ok", 38, 44, { kind: "implement", phase: 2, work: "initial" }, {
    executor: "native",
    model: "anthropic:claude-sonnet-5",
  }),
  exec("x_rev2", "code-reviewer", "review", "backend", "running", 45, null, { kind: "review", phase: 2, tests: false, iteration: 2 }, {
    model: "anthropic:claude-sonnet-5",
  }),
];

const findings = [
  {
    severity: "HIGH" as const,
    file: "src/main/java/shop/order/OrderService.java",
    rule: "C3",
    description: "cancel() changes status without checking the shipped state on line 88.",
    fix: "Change `order.setStatus(CANCELLED);` to `order.requireNotShipped(); order.setStatus(CANCELLED);` on line 88.",
    guidance: null,
  },
  {
    severity: "MEDIUM" as const,
    file: "src/main/java/shop/order/OrderController.java",
    rule: "PHASE-REQ-2",
    description: "Phase step 2.3 requires a 409 response for a shipped order, but line 41 returns 400.",
    fix: "Change `HttpStatus.BAD_REQUEST` to `HttpStatus.CONFLICT` on line 41.",
    guidance: null,
  },
];

export const gates: GateView[] = [
  {
    id: "g_perm",
    session: SESSION,
    title: "Allow a shell command?",
    explanation: "The code reviewer wants to run a command no rule allows. Permission mode is default, so unlisted commands ask first.",
    payload: {
      kind: "permission",
      execution: "x_rev2",
      agent: "code-reviewer",
      call: { tool: "Bash", input: { command: "./gradlew :order:check", description: "Run the order module checks" } },
      reason: "Bash command `./gradlew :order:check` matches no allow rule.",
      rule: { layer: "permission", rule: "mode:default" },
      suggestion: "Bash(./gradlew *)",
    },
    answer: null,
    source: null,
    reason: null,
    opened_at: at(47),
    answered_at: null,
  },
  {
    id: "g_closing",
    session: SESSION,
    title: "Closing gate for web",
    explanation: "Every phase in web passed review and format ran. Tests and documentation are optional and run only if you ask.",
    payload: { kind: "closing_gate", items: [{ project: "web", phases: 1, ask_tests: true, ask_docs: true }] },
    answer: null,
    source: null,
    reason: null,
    opened_at: at(47),
    answered_at: null,
  },
  {
    id: "g_q",
    session: SESSION,
    title: "Open questions in the spec",
    explanation: "The spec lists questions only you can answer. Each answer is written back into the spec before any fact-check runs.",
    payload: {
      kind: "open_questions",
      artifact: "spec",
      artifact_path: `${SROOT}/ostra-spec-20260922-100700-order-cancel.md`,
      questions: [
        {
          id: "Q1",
          question: "Should a cancelled order refund automatically, or wait for a support agent?",
          tag: "Refunds",
          options: [
            { label: "Automatic refund", description: "Matches the existing refund flow in RefundService." },
            { label: "Manual review", description: "Support approves each refund; slower for customers." },
          ],
          recommended: 0,
          multi_select: false,
        },
      ],
    },
    answer: { kind: "questions", answers: [{ id: "Q1", question: "Should a cancelled order refund automatically?", answer: "Automatic refund" }] },
    source: "user",
    reason: null,
    opened_at: at(12),
    answered_at: at(13),
  },
  {
    id: "g_spec",
    session: SESSION,
    title: "Approve the spec",
    explanation: "The spec passed fact-check. Approving it lets the plan agent turn it into phases.",
    payload: {
      kind: "spec_approval",
      spec_path: `${SROOT}/ostra-spec-20260922-100700-order-cancel.md`,
      summary: "Two deliverables: the backend cancellation contract, then the web client types and button.",
      findings: [
        { severity: "LOW", location: "R4", claim: "The endpoint returns within 200 ms", issue: "No measurement backs this; it does not block approval." },
      ],
    },
    answer: { kind: "approval", approved: true, feedback: null },
    source: "user",
    reason: null,
    opened_at: at(18),
    answered_at: at(19),
  },
  {
    id: "g_cap",
    session: SESSION,
    title: "Review cap reached for phase 1",
    explanation: "Three review passes left findings open. A fourth pass is your call.",
    payload: {
      kind: "review_cap",
      project: "backend",
      phase: 1,
      tests: false,
      iterations: 3,
      findings,
      ledger_path: `${SROOT}/backend/ostra-review-ledger-phase-1.md`,
    },
    answer: { kind: "choice", option: "another-pass", text: null },
    source: "user",
    reason: null,
    opened_at: at(37),
    answered_at: at(37),
  },
];

export const decisions: DecisionView[] = [
  {
    id: "d_classify",
    judge: "classify",
    subject: null,
    input_summary: "Request text and 2 projects",
    output: { category: "IMPLEMENT", projects: ["backend", "web"], explore_tasks: [{ project: "backend", task: "Order lifecycle" }, { project: "web", task: "Order views" }] },
    reason: "The request adds behavior (cancel an order) in both the backend and the web client.",
    overridden: false,
    can_override: false,
    at: at(0),
  },
  {
    id: "d_stakes",
    judge: "stakes",
    subject: null,
    input_summary: "Approved spec with 2 deliverables",
    output: { stakes: "medium" },
    reason: "Two projects change and one contract crosses them, so the work needs a phased plan.",
    overridden: false,
    can_override: false,
    at: at(20),
  },
];

export const sessionDetail: SessionDetail = {
  summary: sessionSummary,
  session_root: SROOT,
  stages: [
    { stage: "classify", lane: "research", label: "Classify the request", status: "done", project: null, phase: null, executions: [], gate: null, detail: "IMPLEMENT" },
    { stage: "explore", lane: "research", label: "Explore backend", status: "done", project: "backend", phase: null, executions: ["x_exp1"], gate: null, detail: null },
    { stage: "explore", lane: "research", label: "Explore web", status: "done", project: "web", phase: null, executions: ["x_exp2"], gate: null, detail: null },
    { stage: "spec", lane: "requirements", label: "Write the spec", status: "done", project: null, phase: null, executions: ["x_spec"], gate: null, detail: "2 deliverables" },
    { stage: "open-questions", lane: "requirements", label: "Answer open questions", status: "done", project: null, phase: null, executions: [], gate: "g_q", detail: "1 answered" },
    { stage: "fact-check-spec", lane: "verification", label: "Fact-check the spec", status: "done", project: null, phase: null, executions: ["x_fc1"], gate: null, detail: "PASS" },
    { stage: "spec-approval", lane: "verification", label: "Approve the spec", status: "done", project: null, phase: null, executions: [], gate: "g_spec", detail: "approved" },
    { stage: "plan", lane: "design", label: "Plan the phases", status: "done", project: null, phase: null, executions: ["x_plan"], gate: null, detail: "3 phases" },
    { stage: "fact-check-plan", lane: "verification", label: "Fact-check the plan", status: "done", project: null, phase: null, executions: ["x_fc2"], gate: null, detail: "PASS" },
    { stage: "implement", lane: "build", label: "Phase 1", status: "done", project: "backend", phase: 1, executions: ["x_imp1"], gate: null, detail: null },
    { stage: "review", lane: "review", label: "Review phase 1", status: "done", project: "backend", phase: 1, executions: ["x_rev1"], gate: "g_cap", detail: "passed" },
    { stage: "implement", lane: "build", label: "Phase 2", status: "done", project: "backend", phase: 2, executions: ["x_imp2"], gate: null, detail: null },
    { stage: "review", lane: "review", label: "Review phase 2", status: "waiting", project: "backend", phase: 2, executions: ["x_rev2"], gate: "g_perm", detail: "pass 2 of 3" },
    { stage: "closing-gate", lane: "test", label: "Closing gate for web", status: "waiting", project: "web", phase: null, executions: [], gate: "g_closing", detail: null },
    { stage: "module-docs", lane: "docs", label: "Module documentation", status: "pending", project: null, phase: null, executions: [], gate: null, detail: null },
    { stage: "completion", lane: "done", label: "Completion report", status: "pending", project: null, phase: null, executions: [], gate: null, detail: null },
  ],
  phases: [
    { info: phases[0], status: "passed", review_iterations: 3, tests: "none", security_block: false },
    { info: phases[1], status: "reviewing", review_iterations: 2, tests: "none", security_block: false },
    { info: phases[2], status: "queued", review_iterations: 0, tests: "skipped", security_block: false },
  ],
  executions,
  gates,
  decisions,
  artifacts: [
    { path: `${SROOT}/backend/ostra-research-20260922-100100-order-lifecycle.md`, kind: "research", label: "Research: order lifecycle", project: "backend" },
    { path: `${SROOT}/ostra-spec-20260922-100700-order-cancel.md`, kind: "spec", label: "Spec: order cancellation", project: null },
    { path: `${SROOT}/ostra-plan-20260922-100500-order-cancel.md`, kind: "plan", label: "Master plan", project: null },
    { path: `${SROOT}/backend/ostra-implementer-phase-1.md`, kind: "report", label: "Implementer report, phase 1", project: "backend" },
    { path: `${SROOT}/backend/ostra-review-ledger-phase-1.md`, kind: "ledger", label: "Review ledger, phase 1", project: "backend" },
  ],
  completion: null,
};

export const completedDetail: SessionDetail = {
  ...sessionDetail,
  summary: { ...sessionSummary, id: "s_research", status: "completed", lane: "done", stage_label: "Completed", open_gates: 0 },
  gates: [],
  completion:
    "# Completion report\n\nThe refund flow publishes `RefundIssued` from `RefundService.issue` after the transaction commits.\n\n## Stages not run\n\n- Tests: not requested. Ask \"write the tests now\" to run them.\n\n## Decided for you\n\n- **Closing gate:** no tests, no docs (recommended defaults). [Re-run from here](#)\n",
};

export const activity: ActivityItem[] = [
  { seq: 1, at: at(45), delta: { kind: "status", message: "Started on anthropic:claude-sonnet-5" } },
  { seq: 2, at: at(45), delta: { kind: "thinking", text: "The phase file asks for a 409 on a shipped order. I will read the controller first." } },
  { seq: 3, at: at(45), delta: { kind: "tool_call", call_id: "c1", call: { tool: "Read", input: { file_path: "/home/me/code/shop-backend/src/main/java/shop/order/OrderController.java" } } } },
  { seq: 4, at: at(45), delta: { kind: "policy", call_id: "c1", decision: { decision: "allow", rule: null } } },
  { seq: 5, at: at(45), delta: { kind: "tool_result", call_id: "c1", output: "     1\tpackage shop.order;\n     2\t\n     3\timport org.springframework.http.HttpStatus;", is_error: false, duration_ms: 3 } },
  { seq: 6, at: at(46), delta: { kind: "tool_call", call_id: "c2", call: { tool: "Write", input: { file_path: "/home/me/code/shop-backend/src/main/java/shop/order/Notes.java", content: "x" } } } },
  { seq: 7, at: at(46), delta: { kind: "policy", call_id: "c2", decision: { decision: "deny", reason: "Write only inside the session dir. The code reviewer never modifies project source, so review findings go in the ledger instead.", rule: { layer: "guard", rule: "write-scope" } } } },
  { seq: 8, at: at(46), delta: { kind: "tool_result", call_id: "c2", output: "Denied by guard write-scope.", is_error: true, duration_ms: 0 } },
  { seq: 9, at: at(47), delta: { kind: "tool_call", call_id: "c3", call: { tool: "Bash", input: { command: "./gradlew :order:check" } } } },
  { seq: 10, at: at(47), delta: { kind: "policy", call_id: "c3", decision: { decision: "ask", reason: "Bash command `./gradlew :order:check` matches no allow rule.", rule: { layer: "permission", rule: "mode:default" } } } },
  { seq: 11, at: at(47), delta: { kind: "text", text: "Waiting for permission to run the module checks." } },
  { seq: 12, at: at(47), delta: { kind: "usage", usage: usage(1, 0.31) } },
];

export const lessons: Lesson[] = [
  { id: 1, area: "order::OrderService", lesson: "Status changes must go through OrderStateMachine; setStatus skips the event publisher.", source: "implementer", created_at: at(-3000) },
  { id: 2, area: "build", lesson: "./mvnw needs JAVA_HOME pointing at JDK 21; the system JDK 17 fails with class file version 65.", source: "explore", created_at: at(-2000) },
];

const row = (key: string, n: number, cost: number): CostRow => ({
  key,
  executions: n,
  usage: usage(n, cost),
  cache_reads_per_tool_call: 90000 / 14,
});

export const cost: CostReport = {
  by_session: [row(SESSION, 10, 3.42), row("s_research", 1, 0.41)],
  by_stage: [row("explore", 2, 0.6), row("spec", 1, 0.5), row("implement", 2, 1.1), row("review", 2, 0.6)],
  by_agent: [row("explore", 2, 0.6), row("implementer", 2, 1.1), row("code-reviewer", 2, 0.6)],
  by_executor: [row("native", 9, 3.0), row("harness:codex", 1, 0.42)],
  total: row("total", 11, 3.83),
};

export const markdownSpec = `# Spec: order cancellation

**Date:** 2026-09-22 · **Projects:** backend, web

## Requirements

| ID | Requirement (EARS) |
| --- | --- |
| R1 | When a customer requests cancellation of an order that has not shipped, the system shall set the order status to CANCELLED. |
| R2 | If the order has shipped, then the system shall refuse the cancellation with HTTP 409. |

### Acceptance criteria

- **AC-R1-1** Given an order in status PAID, when the customer cancels it, then its status is CANCELLED and a refund starts.
- **AC-R2-1** Given an order in status SHIPPED, when the customer cancels it, then the response is 409 and the status is unchanged.

## External Evidence

None.
`;

export const markdownLedger = `# Code Review Ledger

## Iteration 1 (context: implementation)

| ID | Severity | File | Rule | Description | Fix Suggestion |
| --- | --- | --- | --- | --- | --- |
| F1 | HIGH | OrderService.java | C3 | Missing shipped check | Add requireNotShipped() |
`;
