// Mock data for the Execution and Artifact screens: activity streams, live deltas, terminal transcripts, and
// artifact files. Used by mockApi.ts and mockSocket.ts under `VITE_MOCK=1`.

import { Slugger } from "../../screens/artifact/outline";
import type {
  ActivityItem,
  Artifact,
  DiffFile,
  ExecutionDelta,
  ExecutionView,
  PolicyDecision,
  ToolCall,
  Usage,
} from "../types";
import * as f from "./fixtures";
import { documentFor } from "./fixtures.documents";

const SROOT = `/home/me/code/shop/.ostra/sessions/${f.SESSION}`;
const BACKEND = "/home/me/code/shop-backend";
const WEB = "/home/me/code/shop-web";
const t0 = new Date("2026-09-22T10:38:00Z").getTime();

// ---------------------------------------------------------------------------------------------
// Activity scripts
// ---------------------------------------------------------------------------------------------

type ToolOpts = {
  policy?: PolicyDecision;
  output?: string;
  error?: boolean;
  ms?: number;
  chunks?: string[];
  pending?: boolean;
};

/** Builds an activity history one delta at a time, so seqs and call ids stay consistent. */
class Script {
  items: ActivityItem[] = [];
  private n = 0;
  constructor(private startMs: number) {}
  private push(delta: ExecutionDelta) {
    this.items.push({
      seq: this.items.length + 1,
      at: new Date(this.startMs + this.items.length * 4000).toISOString(),
      delta,
    });
  }
  status(message: string) {
    this.push({ kind: "status", message });
    return this;
  }
  think(text: string) {
    this.push({ kind: "thinking", text });
    return this;
  }
  text(text: string) {
    this.push({ kind: "text", text });
    return this;
  }
  usage(usage: Usage) {
    this.push({ kind: "usage", usage });
    return this;
  }
  tool(tool: string, input: unknown, o: ToolOpts = {}) {
    const id = `c${++this.n}`;
    const call: ToolCall = { tool, input };
    this.push({ kind: "tool_call", call_id: id, call });
    this.push({ kind: "policy", call_id: id, decision: o.policy ?? { decision: "allow", rule: null } });
    for (const chunk of o.chunks ?? []) this.push({ kind: "tool_output", call_id: id, chunk });
    if (!o.pending)
      this.push({
        kind: "tool_result",
        call_id: id,
        output: o.output ?? "",
        is_error: !!o.error,
        duration_ms: o.ms ?? 90,
      });
    return this;
  }
  /** Finish a call left pending earlier. */
  result(callId: string, output: string, ms: number, error = false) {
    this.push({ kind: "tool_result", call_id: callId, output, is_error: error, duration_ms: ms });
    return this;
  }
  get length() {
    return this.items.length;
  }
}

const allow = (layer: string, rule: string): PolicyDecision => ({ decision: "allow", rule: { layer, rule } });
const deny = (rule: string, reason: string): PolicyDecision => ({
  decision: "deny",
  reason,
  rule: { layer: "guard", rule },
});
const ask = (rule: string, reason: string): PolicyDecision => ({
  decision: "ask",
  reason,
  rule: { layer: "permission", rule },
});

const use = (i: number, o: number, cache: number, cost: number, calls: number, build: number): Usage => ({
  input_tokens: i,
  output_tokens: o,
  cache_read_tokens: cache,
  cache_write_tokens: Math.round(i / 20),
  cache_write_1h_tokens: 0,
  cost_usd: cost,
  tool_calls: calls,
  build_ms: build,
});

const SERVICE_BEFORE = `  public Order cancel(OrderId id, Actor by) {
    Order order = repo.find(id).orElseThrow(OrderNotFound::new);
    order.setStatus(OrderStatus.CANCELLED);
    return repo.save(order);
  }`;
const SERVICE_AFTER = `  public Order cancel(OrderId id, Actor by) {
    Order order = repo.find(id).orElseThrow(OrderNotFound::new);
    if (order.getStatus().isShippedOrLater()) {
      throw new CannotCancel(order.getId());
    }
    stateMachine.transition(order, OrderStatus.CANCELLED, by, clock.instant());
    return repo.save(order);
  }`;

/** A completed native implementer run with every kind of item the Activity stream shows. */
const implementerPhase2 = new Script(t0)
  .status(
    "Started on native · anthropic:claude-sonnet-5-5 · phase file ostra-plan-20260922-100500-order-cancel-phase-2-service.md",
  )
  .think(
    "Phase 2 wires the cancel endpoint. R2 says a shipped order must return 409 with cannot_cancel, and the phase names OrderStateMachine for every status change. Load the controller skill, then read the controller and the service.",
  )
  .tool("Skill", { name: "spring-controller" }, { output: "Loaded .ostra/skills/spring-controller/SKILL.md", ms: 40 })
  .tool(
    "Read",
    { file_path: `${BACKEND}/src/main/java/shop/order/OrderController.java` },
    {
      output:
        '  1\tpackage shop.order;\n  2\t\n  3\timport org.springframework.http.HttpStatus;\n  …\n 38\t  @PostMapping("/orders/{id}/cancel")\n 39\t  public ResponseEntity<OrderDto> cancel(@PathVariable String id, Principal who) {\n 40\t    return ResponseEntity.ok(OrderDto.of(service.cancel(OrderId.of(id), Actor.of(who))));\n 41\t  }',
      ms: 12,
    },
  )
  .tool(
    "Grep",
    { pattern: "isShippedOrLater", path: "src" },
    { output: "src/main/java/shop/order/OrderStatus.java:21:  public boolean isShippedOrLater() {", ms: 60 },
  )
  .text(
    "`OrderService.cancel` sets the status directly, which skips the event publisher (lesson 1 in project memory). Routing it through `OrderStateMachine` and refusing shipped orders with `CannotCancel`, which the controller maps to 409.",
  )
  .tool(
    "Edit",
    {
      file_path: `${BACKEND}/src/main/java/shop/order/OrderService.java`,
      old_string: SERVICE_BEFORE,
      new_string: SERVICE_AFTER,
    },
    { policy: allow("permission", "mode:acceptEdits"), output: "The file was updated.", ms: 8 },
  )
  .tool(
    "Edit",
    {
      file_path: `${BACKEND}/src/main/java/shop/order/OrderController.java`,
      old_string: "  @ExceptionHandler(OrderNotFound.class)",
      new_string:
        '  @ExceptionHandler(CannotCancel.class)\n  public ResponseEntity<ApiError> cannotCancel(CannotCancel e) {\n    return ResponseEntity.status(HttpStatus.CONFLICT).body(ApiError.of("cannot_cancel"));\n  }\n\n  @ExceptionHandler(OrderNotFound.class)',
    },
    { policy: allow("permission", "mode:acceptEdits"), output: "The file was updated.", ms: 7 },
  )
  .tool(
    "Bash",
    { command: "./gradlew :order:compileJava", description: "Compile the order module" },
    {
      policy: allow("permission", "Bash(./gradlew *)"),
      chunks: ["> Task :order:compileJava\n"],
      output: "> Task :order:compileJava\n\nBUILD SUCCESSFUL in 8s\n1 actionable task: 1 executed",
      ms: 8400,
    },
  )
  .tool(
    "Write",
    {
      file_path: `${BACKEND}/src/test/java/shop/order/OrderCancelTest.java`,
      content: "package shop.order;\n\nclass OrderCancelTest {\n  @Test void shippedOrderCannotBeCancelled() { }\n}\n",
    },
    {
      policy: deny(
        "no-tests-from-implementer",
        "The implementer may not write a path that matches the project's test patterns (src/test/**).",
      ),
      output: "Denied by guard no-tests-from-implementer.",
      error: true,
      ms: 0,
    },
  )
  .tool(
    "Bash",
    { command: "./gradlew :order:check" },
    {
      policy: allow("permission", "Bash(./gradlew *)"),
      output:
        "> Task :order:checkstyleMain FAILED\n[ant:checkstyle] OrderService.java:47:5: 'clock' is not final. [FinalField]\n\nFAILURE: Build failed with an exception.\nBUILD FAILED in 6s",
      error: true,
      ms: 6200,
    },
  )
  .text("Checkstyle wants the injected `clock` field final. One-line fix, then the check again.")
  .tool(
    "Edit",
    {
      file_path: `${BACKEND}/src/main/java/shop/order/OrderService.java`,
      old_string: "  private Clock clock;",
      new_string: "  private final Clock clock;",
    },
    { policy: allow("permission", "mode:acceptEdits"), output: "The file was updated.", ms: 6 },
  )
  .tool(
    "Bash",
    { command: "./gradlew :order:check" },
    {
      policy: allow("permission", "Bash(./gradlew *)"),
      output: "> Task :order:check\n\nBUILD SUCCESSFUL in 7s\n6 actionable tasks: 3 executed, 3 up-to-date",
      ms: 6800,
    },
  )
  .tool(
    "Memory",
    {
      area: "build",
      lesson: "Checkstyle FinalField fails ./gradlew :order:check for injected fields that are not final.",
      source: "implementer",
    },
    { output: "Recorded lesson 3 in build.", ms: 20 },
  )
  .tool(
    "Write",
    {
      file_path: `${SROOT}/backend/ostra-implementer-phase-2.md`,
      content:
        "# Implementer report, phase 2\n\n## Changes\n\n- `OrderService.cancel` refuses shipped orders and goes through `OrderStateMachine`.\n- `OrderController` maps `CannotCancel` to 409 `cannot_cancel`.\n\n## Tests to write\n\n- A shipped order returns 409 and stays unchanged (AC-R2-1).\n",
    },
    { output: "Wrote 9 lines.", ms: 5 },
  )
  .tool(
    "submit_implementer",
    {
      status: "complete",
      report: `${SROOT}/backend/ostra-implementer-phase-2.md`,
      files_changed: ["src/main/java/shop/order/OrderService.java", "src/main/java/shop/order/OrderController.java"],
    },
    { output: "Accepted.", ms: 3 },
  )
  .usage(use(184200, 12700, 151000, 0.41, 14, 21400));

/** A running native fact-check. The mock socket streams the rest after the page loads. */
const factCheck = new Script(t0 - 20 * 60000)
  .status("Started on native · anthropic:claude-opus-5-5 · target ostra-spec-refund-retries.md · Source check: refetch")
  .think(
    "First pass over a spec whose External Evidence table has rows, so every cited source is refetched. Prior findings: none. Claim surface: 5 requirements, 3 contracts, 2 external claims.",
  )
  .tool(
    "Read",
    { file_path: "/home/me/code/shop/.ostra/sessions/s_refund/ostra-spec-refund-retries.md" },
    {
      output:
        "## Requirements\nR1 When a refund webhook fails, the system shall retry it with exponential backoff …\nR3 The system shall move an event to the webhook_events dead-letter table after 8 attempts.",
      ms: 10,
    },
  )
  .tool(
    "Grep",
    { pattern: "fn handle_refund_webhook", path: "src" },
    { output: "src/payments/webhooks.rs:57  pub async fn handle_refund_webhook(", ms: 70 },
  )
  .tool(
    "WebFetch",
    { url: "https://docs.stripe.com/webhooks#retries", prompt: "How long does Stripe retry a failed webhook?" },
    { output: "Automatic retries: up to 3 days with exponential backoff in live mode.", ms: 1400 },
  )
  .text(
    "E1 holds: the provider retries for up to 3 days, so Ostra's own backoff must dedupe on `event.id`. Checking R3's table next.",
  )
  .tool("Grep", { pattern: "webhook_events", path: "migrations/ src/" }, { pending: true })
  .usage(use(61000, 3100, 48000, 0.19, 5, 0));
const factCheckBase = factCheck.length;
factCheck
  .result("c4", "(no matches)", 210)
  .text(
    "R3 is a FAIL finding: no `webhook_events` table exists in migrations or code. The spec must add it as a new migration or name the existing `payment_events` table.",
  )
  .tool(
    "Read",
    { file_path: "/home/me/code/shop-backend/migrations/20260611_payment_events.sql" },
    {
      output:
        "CREATE TABLE payment_events (\n  id TEXT PRIMARY KEY,\n  kind TEXT NOT NULL,\n  payload JSONB NOT NULL\n);",
      ms: 9,
    },
  )
  .think(
    "payment_events has no attempts column, so it cannot serve as the dead-letter table without a change. That is a second finding against R3's contract.",
  )
  .tool(
    "Grep",
    { pattern: "attempts", path: "src/payments" },
    { output: "src/payments/retry.rs:12  const MAX_ATTEMPTS: u32 = 5;", ms: 55 },
  )
  .text("MAX_ATTEMPTS is 5 in code, while R3 says 8. Recording both findings with their evidence.")
  .usage(use(79000, 4600, 63000, 0.26, 8, 0));

/** The hook bridge's record of the codex phase 1 run. */
const codexPhase1 = new Script(t0 - 7 * 60000)
  .status("Started on harness:codex · gpt-5.6-luna · hook bridge on")
  .tool("Read", { file_path: `${BACKEND}/src/main/java/shop/order/OrderStatus.java` }, { ms: 20 })
  .tool("Skill", { path: ".ostra/skills/spring-service/SKILL.md" }, { ms: 15 })
  .tool(
    "Edit",
    {
      file_path: `${BACKEND}/src/main/java/shop/order/OrderStatus.java`,
      old_string: "  SHIPPED,",
      new_string: "  SHIPPED,\n  CANCELLED,",
    },
    { policy: allow("permission", "mode:acceptEdits"), ms: 30 },
  )
  .tool(
    "Write",
    {
      file_path: `${BACKEND}/src/main/resources/db/migration/V42__order_cancelled_at.sql`,
      content:
        "ALTER TABLE orders ADD COLUMN cancelled_at TIMESTAMP NULL;\nALTER TABLE orders ADD COLUMN cancelled_by TEXT NULL;\n",
    },
    { policy: allow("permission", "mode:acceptEdits"), ms: 25 },
  )
  .tool(
    "Bash",
    { command: "./gradlew :order:compileJava" },
    { policy: allow("permission", "Bash(./gradlew *)"), ms: 7900 },
  )
  .tool(
    "Write",
    { file_path: `${BACKEND}/src/test/java/shop/order/OrderCancelTest.java`, content: "class OrderCancelTest {}\n" },
    {
      policy: deny(
        "no-tests-from-implementer",
        "The implementer may not write a path that matches the project's test patterns (src/test/**).",
      ),
      error: true,
      ms: 0,
    },
  )
  .tool("submit_implementer", { status: "complete" }, { ms: 5 })
  .usage(use(142000, 9800, 120000, 0.18, 9, 7900));

/** The hook bridge's record of the live Claude Code phase 3 run, paused on an ask. */
const claudePhase3 = new Script(t0 + 6 * 60000)
  .status("Started on harness:claude · haiku · hook bridge on")
  .tool("Skill", { path: ".ostra/skills/react-component/SKILL.md" }, { ms: 12 })
  .tool("Read", { file_path: `${WEB}/src/pages/OrderPage.tsx` }, { ms: 18 })
  .tool(
    "Edit",
    {
      file_path: `${WEB}/src/components/OrderActions.tsx`,
      old_string: "      <Button onClick={track}>Track</Button>",
      new_string:
        '      {canCancel(order) && (\n        <Button variant="danger" onClick={cancel} disabled={pending}>Cancel order</Button>\n      )}\n      <Button onClick={track}>Track</Button>',
    },
    { policy: allow("permission", "mode:acceptEdits"), ms: 22 },
  )
  .tool("Bash", { command: "npm run typecheck" }, { policy: allow("permission", "Bash(npm run *)"), ms: 6100 })
  .tool(
    "Write",
    { file_path: `${WEB}/src/components/OrderActions.test.tsx`, content: "test('cancel', () => {});\n" },
    {
      policy: deny(
        "no-tests-from-implementer",
        "The implementer may not write a path that matches the project's test patterns (**/*.test.tsx).",
      ),
      error: true,
      ms: 0,
    },
  )
  .tool(
    "Bash",
    { command: "npx eslint --fix src/components" },
    {
      policy: ask("mode:default", "Bash command `npx eslint --fix src/components` matches no allow rule."),
      pending: true,
    },
  )
  .usage(use(38000, 2100, 30000, 0.07, 6, 6100));

const stuckRun = new Script(t0 - 3000 * 60000)
  .status("Started on harness:codex · gpt-5.6-luna · hook bridge on")
  .tool(
    "Bash",
    { command: "./mvnw -q -pl order compile" },
    { policy: allow("permission", "Bash(./mvnw *)"), error: true, ms: 4100 },
  )
  .tool(
    "Bash",
    { command: "./mvnw -q -pl order compile" },
    { policy: allow("permission", "Bash(./mvnw *)"), error: true, ms: 4000 },
  )
  .tool(
    "Bash",
    { command: "./mvnw -q -pl order compile" },
    {
      policy: deny(
        "build-streak",
        "Five builds in a row failed. Return STUCK with the diagnostic instead of building again.",
      ),
      error: true,
      ms: 0,
    },
  );

function genericActivity(x: ExecutionView): ActivityItem[] {
  const s = new Script(new Date(x.started_at).getTime()).status(`Started on ${x.executor} · ${x.model}`);
  if (x.status === "running") return s.text("Reading the inputs named in the spawn block.").items;
  return s
    .text("Done. The report is written and the result is submitted.")
    .tool(`submit_${x.agent.replace(/-/g, "_")}`, { status: "complete" }, { ms: 4 })
    .usage(x.usage).items;
}

const ACTIVITY: Record<string, ActivityItem[]> = {
  x_imp2: implementerPhase2.items,
  x_rev2: f.activity,
  x_r3: factCheck.items.slice(0, factCheckBase),
  x_imp1: codexPhase1.items,
  x_imp3: claudePhase3.items,
  x_n1: stuckRun.items,
};

/** Deltas the mock socket streams on `execution:<id>` after the snapshot, one every couple of seconds. */
export const LIVE_DELTAS: Record<string, ActivityItem[]> = {
  x_r3: factCheck.items.slice(factCheckBase),
};

export function activityFor(id: string): ActivityItem[] {
  const known = ACTIVITY[id];
  if (known) return known;
  const x = f.executions.find((e) => e.id === id);
  return x ? genericActivity(x) : [];
}

// ---------------------------------------------------------------------------------------------
// Executions
// ---------------------------------------------------------------------------------------------

const OVERRIDES: Record<string, Partial<ExecutionView>> = {
  x_imp1: { has_transcript: true, usage: use(142000, 9800, 120000, 0.18, 9, 7900) },
  x_imp2: {
    usage: use(184200, 12700, 151000, 0.41, 14, 21400),
    report_path: `${SROOT}/backend/ostra-implementer-phase-2.md`,
  },
  x_imp3: { usage: use(38000, 2100, 30000, 0.07, 6, 6100) },
  x_r3: {
    usage: use(61000, 3100, 48000, 0.19, 5, 0),
    spawn_block:
      "Workspace root: /home/me/code/shop\nRepo root: /home/me/code/shop-backend\nTarget: ostra-spec-refund-retries.md\nSource check: refetch",
  },
  x_n1: {
    can_resume: true,
    native_session_id: "019a-codex-stuck",
    error: "Five builds in a row failed: `./mvnw` needs JAVA_HOME pointing at JDK 21.",
  },
};

/** Executions the mock has resumed; their terminals go live. */
const resumed = new Set<string>();

export function executionView(id: string): ExecutionView {
  const x = f.executions.find((e) => e.id === id) ?? f.executions[0];
  return {
    ...x,
    ...OVERRIDES[x.id],
    ...(resumed.has(x.id) ? { has_terminal: true, status: "running" as const, ended_at: null } : {}),
  };
}

export function resume(id: string): ExecutionView {
  resumed.add(id);
  return executionView(id);
}

export const isResumed = (id: string) => resumed.has(id);

// ---------------------------------------------------------------------------------------------
// Terminal transcripts (raw PTY bytes, ANSI colors, CRLF line ends)
// ---------------------------------------------------------------------------------------------

const DIM = "\x1b[2m";
const RESET = "\x1b[0m";
const BOLD = "\x1b[1m";
const GREEN = "\x1b[32m";
const RED = "\x1b[31m";
const YELLOW = "\x1b[33m";
const BLUE = "\x1b[34m";
const ORANGE = "\x1b[38;5;173m";
const tty = (lines: string[]) => lines.map((l) => `${l}\r\n`).join("");

const CODEX_PHASE1 = tty([
  `${DIM}╭─ codex 0.153.4 · gpt-5.6-luna · ~/code/shop-backend${RESET}`,
  `${DIM}╰─ ostra hook bridge on · mcp: ostra (report, memory, submit_implementer)${RESET}`,
  "",
  `${BLUE}› Implement phase 1: order cancellation data layer (ostra-plan-20260922-100500-order-cancel-phase-1-data-layer.md)${RESET}`,
  "",
  "  Reading src/main/java/shop/order/OrderStatus.java, src/main/java/shop/order/Order.java",
  "  Loading skill .ostra/skills/spring-service/SKILL.md",
  "",
  "  Edit src/main/java/shop/order/OrderStatus.java  (+1)",
  `${GREEN}  +  CANCELLED,${RESET}`,
  "",
  "  Write src/main/resources/db/migration/V42__order_cancelled_at.sql  (+2)",
  `${GREEN}  +ALTER TABLE orders ADD COLUMN cancelled_at TIMESTAMP NULL;${RESET}`,
  `${GREEN}  +ALTER TABLE orders ADD COLUMN cancelled_by TEXT NULL;${RESET}`,
  "",
  "  $ ./gradlew :order:compileJava",
  `${DIM}    > Task :order:compileJava${RESET}`,
  `${GREEN}    BUILD SUCCESSFUL in 7s${RESET}`,
  "",
  `${RED}  ✗ Write src/test/java/shop/order/OrderCancelTest.java${RESET}`,
  `${RED}    Denied by Ostra guard no-tests-from-implementer. Leave tests to the write-test stage.${RESET}`,
  "",
  `${GREEN}  ✓ submit_implementer  report ostra-implementer-phase-1.md${RESET}`,
  "",
  `${DIM}  Session ended. codex session 019a-codex-thread · resumable${RESET}`,
]);

const CODEX_STUCK = tty([
  `${DIM}╭─ codex 0.153.4 · gpt-5.6-luna · ~/code/shop-backend${RESET}`,
  `${BLUE}› Implement phase 1: N+1 query in the order list${RESET}`,
  "",
  "  $ ./mvnw -q -pl order compile",
  `${RED}    error: release version 21 not supported${RESET}`,
  "  $ ./mvnw -q -pl order compile",
  `${RED}    error: release version 21 not supported${RESET}`,
  `${RED}  ✗ Bash ./mvnw -q -pl order compile${RESET}`,
  `${RED}    Denied by Ostra guard build-streak. Return STUCK with the diagnostic instead of building again.${RESET}`,
  "",
  `${YELLOW}  STUCK: the build needs JDK 21, and JAVA_HOME points at JDK 17.${RESET}`,
  `${DIM}  Session ended. codex session 019a-codex-stuck · resumable${RESET}`,
]);

const CLAUDE_PHASE3 = tty([
  `${ORANGE}✻${RESET} ${BOLD}Claude Code${RESET} · haiku · ~/code/shop-web`,
  `${DIM}  ostra hooks: PreToolUse, PostToolUse → ostra hook --execution x_imp3 · mcp: ostra${RESET}`,
  "",
  "> Implement phase 3: cancellation request types and the Cancel button",
  "",
  "⏺ Skill(.ostra/skills/react-component/SKILL.md)",
  `${DIM}  ⎿  Loaded react-component${RESET}`,
  "⏺ Read(src/pages/OrderPage.tsx)",
  `${DIM}  ⎿  Read 88 lines${RESET}`,
  "⏺ Update(src/components/OrderActions.tsx)",
  `${DIM}  ⎿  Updated src/components/OrderActions.tsx with 3 additions${RESET}`,
  `${GREEN}       +  {canCancel(order) && (${RESET}`,
  `${GREEN}       +    <Button variant="danger" onClick={cancel} disabled={pending}>Cancel order</Button>${RESET}`,
  `${GREEN}       +  )}${RESET}`,
]);

/** Chunks the mock streams into the live Claude Code terminal, ending on the paused ask. */
const CLAUDE_PHASE3_LIVE = [
  tty(["⏺ Bash(npm run typecheck)"]),
  tty([`${GREEN}  ⎿  tsc --noEmit  (6.1s, no errors)${RESET}`]),
  tty([
    `${RED}⏺ Write(src/components/OrderActions.test.tsx)${RESET}`,
    `${RED}  ⎿  Denied by Ostra guard no-tests-from-implementer. Leave tests to the write-test stage.${RESET}`,
    "",
  ]),
  tty(["⏺ Tests belong to write-test. Formatting the component before the report."]),
  tty([
    "⏺ Bash(npx eslint --fix src/components)",
    `${YELLOW}  ⏸ Waiting for approval in Ostra (permission · mode:default)${RESET}`,
  ]),
];

const RESUMED = tty(["", `${DIM}  Resumed with \`codex resume\` in a new terminal.${RESET}`, `${BLUE}› ${RESET}`]);

/** The backlog frame the server sends on `term:<id>`: the screen so far, or the stored transcript. */
export function terminalBacklog(id: string): string | null {
  const base = id === "x_imp1" ? CODEX_PHASE1 : id === "x_n1" ? CODEX_STUCK : id === "x_imp3" ? CLAUDE_PHASE3 : null;
  if (base === null) return null;
  return resumed.has(id) ? base + RESUMED : base;
}

export const terminalLive = (id: string): string[] => (id === "x_imp3" ? CLAUDE_PHASE3_LIVE : []);

// ---------------------------------------------------------------------------------------------
// Artifacts
// ---------------------------------------------------------------------------------------------

const SPEC = `# Spec: order cancellation

**Date:** 2026-09-22 · **Projects:** backend, web · **Deliverables:** D1 (backend), D2 (web)

## Problem statement

Customers cannot cancel an order. Support cancels by hand in the database, 40 to 60 times a week, and a hand-cancelled order never publishes \`OrderCancelled\`, so refunds start late.

## Requirements

| ID | Type | Requirement |
| --- | --- | --- |
| R1 | Event-driven | When a customer requests cancellation of an order in PENDING or PAID, the system shall set the order status to CANCELLED and record \`cancelled_at\` and \`cancelled_by\`. |
| R2 | Unwanted behavior | If the order is SHIPPED or DELIVERED, then the system shall refuse the cancellation with 409 and error code \`cannot_cancel\`. |
| R3 | Ubiquitous | The system shall publish \`OrderCancelled\` for every cancelled order. |
| R4 | State-driven | While an order is CANCELLED, the web client shall hide the Cancel button on the order page. |

### Acceptance criteria

- **AC-R1-1** Given an order in status PAID, when its owner cancels it, then its status is CANCELLED and \`cancelled_by\` is the customer.
- **AC-R2-1** Given an order in status SHIPPED, when its owner posts to \`/orders/{id}/cancel\`, then the response is 409 with code \`cannot_cancel\` and the order is unchanged.
- **AC-R3-1** Given a cancelled order, when the transaction commits, then exactly one \`OrderCancelled\` event is published.

## Contracts

\`\`\`http
POST /orders/{id}/cancel
200 { "id": "…", "status": "CANCELLED", "cancelled_at": "…" }
403 { "error": "not_owner" }
409 { "error": "cannot_cancel" }
\`\`\`

## Deliverables

1. **D1** Backend: the state change, the endpoint, and the event.
2. **D2** Web: the request types and the Cancel button. See [Requirements](#requirements) R4.

## External Evidence

| Claim | Source | Fetched |
| --- | --- | --- |
| Spring \`@ExceptionHandler\` maps an exception to a status | docs.spring.io/spring-framework/reference/web/webmvc/mvc-controller/ann-exceptionhandler.html | 2026-09-22 |

## Open questions

None. The one question (who may cancel) was answered: the order's owner only.
`;

const PLAN = `# Plan: order cancellation

Three phases in two projects. Phase 3 depends on phase 2's contract.

## Phase index

| Phase | Project | Title | Complexity | Tests |
| --- | --- | --- | --- | --- |
| P1 | backend | Order cancellation data layer | medium | Required |
| P2 | backend | Cancellation service and endpoint | high | Required |
| P3 | web | Cancellation request types | low | Skip |

## Phase 1: data layer

Add \`CANCELLED\` to \`OrderStatus\` and a migration for \`cancelled_at\` and \`cancelled_by\`. Covers R1.

## Phase 2: service and endpoint

Route \`OrderService.cancel\` through \`OrderStateMachine\`, refuse shipped orders, and map \`CannotCancel\` to 409. Covers R2 and R3.

## Phase 3: web types

Declare the request and response types and the \`cancelled\` member of \`OrderStatus\`. Covers R4.

## Risks

- A cancelled order that was already refunded by hand. The state machine refuses a second refund.
`;

const RESEARCH = `# Research: order lifecycle

## Summary

Orders move through \`OrderStateMachine\` (\`src/main/java/shop/order/OrderStateMachine.java\`), which publishes one event per transition. \`OrderService\` has three places that call \`setStatus\` directly.

## Findings

### State machine

- Transitions are declared in a static table; CANCELLED is not in it yet.
- Every transition publishes after commit through \`TransactionalEventPublisher\`.

### Callers that bypass it

| File | Line | Call |
| --- | --- | --- |
| OrderService.java | 88 | \`order.setStatus(CANCELLED)\` |
| AdminTools.java | 41 | \`order.setStatus(REFUNDED)\` |

## Sources

- Code only. No external documentation was needed.
`;

const REPORT = `# Implementer report, phase 1

## Changes

- \`OrderStatus\` gains \`CANCELLED\`.
- Migration \`V42__order_cancelled_at.sql\` adds \`cancelled_at\` and \`cancelled_by\`.

## Verification

- \`./gradlew :order:compileJava\` passes.

## Tests to write

- A PAID order cancels (AC-R1-1).
`;

const LEDGER = `# Code Review Ledger

Phase 1 of backend, context: implementation.

## Iteration 1 (context: implementation)

| ID | Severity | File | Rule | Description | Fix Suggestion |
| --- | --- | --- | --- | --- | --- |
| F1 | HIGH | src/main/java/shop/order/OrderService.java | C3 | cancel() changes status without checking the shipped state on line 3. | Call \`order.requireNotShipped()\` before the status change on line 3. |
| F2 | MEDIUM | src/main/java/shop/order/OrderService.java | PHASE-REQ-3 | No OrderCancelled event is published after the change on line 4. | Publish \`OrderCancelled\` after the status change on line 5. |

### Fix pass

- F1 FIXED: \`requireNotShipped()\` added.
- F2 FIXED: the event is published.

## Iteration 2 (context: implementation)

| ID | Severity | File | Rule | Description | Fix Suggestion |
| --- | --- | --- | --- | --- | --- |
| F3 | LOW | src/main/java/shop/order/OrderService.java | C1 | \`events\` is a vague field name on line 5. | Rename it to \`publisher\`. |

### Fix pass

- F3 WONTFIX: \`events\` is the name used across the module.

## Iteration 3 (context: implementation)

No findings. The loop stopped.
`;

export const ledgerDiff: DiffFile[] = [
  {
    path: "src/main/java/shop/order/OrderService.java",
    original: "class OrderService {\n  void cancel(Order order) {\n    order.setStatus(CANCELLED);\n  }\n}\n",
    modified:
      "class OrderService {\n  void cancel(Order order) {\n    order.requireNotShipped();\n    order.setStatus(CANCELLED);\n    events.publish(new OrderCancelled(order.id()));\n  }\n}\n",
  },
];

function inline(raw: string): string {
  return raw
    .replace(/`([^`]*)`/g, "$1")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\*/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

/** ATX headings outside fenced code, with the server's slugs. Enough for the mock artifacts. */
export function mockHeadings(markdown: string): Artifact["headings"] {
  const slugs = new Slugger();
  const out: Artifact["headings"] = [];
  let fence = false;
  for (const line of markdown.split("\n")) {
    if (/^\s{0,3}(```|~~~)/.test(line)) fence = !fence;
    if (fence) continue;
    const m = /^\s{0,3}(#{1,6})\s+(.*?)(\s+#+)?\s*$/.exec(line);
    if (!m) continue;
    const title = inline(m[2]);
    if (title) out.push({ level: m[1].length, title, id: slugs.slug(title) });
  }
  return out;
}

export function artifactFor(path: string): Artifact {
  const name = path.split("/").pop() ?? "";
  const content = name.includes("ledger")
    ? LEDGER
    : name.startsWith("ostra-spec")
      ? SPEC
      : name.startsWith("ostra-plan")
        ? PLAN
        : name.startsWith("ostra-research")
          ? RESEARCH
          : name.includes("implementer")
            ? REPORT
            : SPEC;
  return {
    path,
    content,
    headings: mockHeadings(content),
    document: documentFor(path),
    binary: false,
    size: content.length,
  };
}
