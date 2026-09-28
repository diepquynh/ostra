# Gates and judges

Ostra's pipeline is driven by code, but two kinds of decisions cannot be made by code alone. Some need the
user: approving a spec, choosing whether to write tests, raising a budget. Those are **gates**. Others need
judgment about text: which category a request belongs to, whether research is complete, what to do with a
stuck agent. Those are **judges**. This page covers every gate kind, every judge, how YOLO mode answers gates
without the user, and which gates YOLO is never allowed to answer.

## How a gate works

A gate is a question the session waits on. The planner emits an `OpenGate` step with a title, an explanation,
and a typed payload (`GatePayload` in [`crates/ostra-core/src/event.rs`](../../crates/ostra-core/src/event.rs)).
The runner records a `GateOpened` event, the board shows a card, and the part of the pipeline behind the gate
stops. Other work keeps going: a review-cap gate on phase 2 does not hold up an independent phase in another
project.

The user's answer is checked before it is recorded. `runner::validate_answer` refuses an answer whose shape does
not fit the gate ("That answer does not fit this gate."), an open-questions answer that leaves a question blank,
and a rejection of a spec or plan with no feedback ("Say what to change."). A valid answer becomes a
`GateAnswered` event, and the fold in [`state.rs`](../../crates/ostra-engine/src/state.rs) (`on_gate_answered`)
turns it into new state that the planner acts on.

Two properties follow from gates being events.

- **A stale answer does nothing.** If the spec changed after the approval gate opened, approving the old card
  is ignored, because the fold checks that the gate still belongs to the current version. An approval also
  requires that the current version passed its fact-check. The fold enforces this, so no client, script, or
  judge can approve an unchecked spec.
- **Gates survive restarts.** An open gate is part of the folded state. After a server restart the card is
  still there, and answering it continues the session.

A paused session starts nothing, but its gates can still be answered; the answers take effect on continue
(Rule P1).

A gate appears on the session board under Waiting for you. This spec approval gate shows the fact-check result and
the LOW findings, and takes an optional change request:

![The spec approval gate with a Fact-check PASS badge, two LOW findings, and a change request field](../images/console/gate-spec-approval.png)

## Every gate kind

| Gate | Opens when | The user answers |
| --- | --- | --- |
| `open_questions` | The spec or the plan lists questions it could not settle | Every question, by choosing an option or writing text |
| `spec_approval` | The spec passed its fact-check | Approve, or reject with what to change |
| `plan_approval` | The plan passed its fact-check | Approve, or reject with what to change |
| `fact_check_recurring` | The spec or plan fact-check failed three times in a row | `another-round` (optionally with guidance) or `stop` |
| `review_cap` | A review loop used its passes with HIGH or MEDIUM findings open | `another-pass` (optionally with guidance) or leave it blocked |
| `stuck` | The Rescue judge decided only the user has the missing fact | `fact` with the fact as text, or `block` |
| `phase_blocked` | A phase ended blocked | `retry` (optionally with instructions) or `leave` |
| `implementation_review` | Every phase of an `IMPLEMENT` session finished and nothing runs | `done`, or `feedback` with what to change as text |
| `closing_gate` | A project's phases are done and, for `IMPLEMENT`, the implementation was accepted | Tests yes or no, docs yes or no, per project |
| `permission` | A tool call needs approval under the permission rules | Allow once, allow always in this workspace, or deny |
| `harness_failure` | A harness CLI failed to start or is not signed in | `retry` after signing in, `native` to re-run on the native executor, or abandon |
| `execution_failed` | An execution failed after its automatic retry | `retry` or `abandon` |
| `skill_approval` | The init flow proposed skills | Per skill: generate, regenerate, reuse, or drop |
| `budget_reached` | The session spent its budget | `raise` (with the extra dollars as text) or `stop` |

### Open questions

The spec's open questions are asked before any fact-check (Rule D3). Each answer is added to the spec's pending
input, and generate-spec runs again to write the answers into the spec. The plan's clarifying questions are
handled differently: the answers are treated as a requirement change and go into the spec first (Rule D10),
because the plan agent reads only the spec.

![The open questions gate with a single-choice and a multiple-choice question](../images/console/gate-open-questions.png)

### Spec and plan approval

Approving records the approved version. For the plan, approving also turns the Phase Index into the build queue.
Rejecting with feedback adds the feedback to the spec as a change. For a plan, that is a requirement change:
both approvals are revoked, the spec is revised and re-approved, and the plan is revised in place.

![The plan approval gate with four phases and one LOW finding](../images/console/gate-plan-approval.png)

### Fact-check recurring

The fact-check loop between an author and its checker can fail to converge. After three FAILs in a row
(`FACTCHECK_RECURRING_LIMIT`) the engine asks instead of spending another round. `another-round` allows three
more FAILs before the next ask, and any guidance text goes to the author as a change. `stop` stops the stage and
the session fails.

![The fact-check-recurring gate with a HIGH and a MEDIUM finding and a guidance field](../images/console/gate-factcheck-recurring.png)

### Review cap

Three review passes run per loop (`REVIEW_CAP`). If HIGH or MEDIUM findings are still open, the card shows them
with the ledger path. `another-pass` adds one pass and sends the open findings to the fix agent, plus any text
the user wrote. Anything else marks the phase blocked, with the finding count and ledger path as the reason.

BLOCKER findings never reach this gate. They loop without a cap until removed (Hard rule 21), and no answer can
waive them.

A review cap gate lists the findings still open after the last pass. Above it, a BLOCKER from the reviewer shows
its Guidance text and has no dismiss button:

![A security BLOCKER notice above the review cap gate of phase 1](../images/console/gate-review-cap.png)

### Stuck

A stuck agent's diagnostic and need are shown. A `fact` answer goes to the Route-answer judge first, because a
stated fact might be a requirement change rather than a detail. Any other answer blocks the phase.

A stuck gate names the execution and asks for the missing fact. Above it is a phase blocked gate for another phase:

![A phase blocked gate with Retry the phase and a stuck gate for phase 2](../images/console/gate-stuck.png)

### Phase blocked

A blocked phase removes its dependents from the queue (Rule D9). The gate asks whether to try again. `retry`
restores the review budget and runs the fix agent again. With instructions, they go through the Route-answer
judge; without, the fix agent gets the last review's BLOCKER, HIGH, and MEDIUM findings.

### Implementation review

The review gate sits between the last phase and the closing stages (Rule F1). `done` accepts the implementation
and lets format, the closing gate, tests, and docs run. `feedback` needs text; it starts a feedback round that
builds revision phases, and the gate opens again once they finish, as round 2, 3, and so on. The card links the
session context file the runner writes before it opens (Rule F2) and every implementer report, and lists any
phase that ended blocked. Under YOLO the fixed answer is `done`. See
[Implementation review](pipeline.md#implementation-review-feedback-until-you-accept) for how a round is routed and
built.

### Closing gate

One gate per batch of projects that finished together (Rule T6). A project the request already opted in or out
of is not asked about that stage (Rule T3). The answers decide which of the test and docs stages run; they never
change requirements (Rule T5).

![The closing gate with Write tests and Update the module documentation checkboxes](../images/console/closing-gate.png)

### Permission

A permission gate comes from the policy's layer 2, Claude Code's permission model, not from the planner. It holds
a live execution's tool call. The card shows the canonical tool call, the reason, the rule that asked, and the
rule "always in this workspace" would add (for example `Bash(cargo test *)` or `Edit(src/api/**)`, built by
`runner::suggest_rule`). Layer 1 guards never produce a gate: they deny, and no answer can override them.

![A permission gate for a Bash command with Allow once, Always in this workspace, and Deny](../images/console/permission-gate.png)

The execution shows the same ask above its activity while the call waits:

![A code reviewer run paused on a permission ask](../images/console/reviewer-ask.png)

### Harness failure and execution failed

Both hold a failed execution. `retry` re-runs it from its spawn block. For a harness failure, `native` re-runs it
on the native executor and keeps that agent on native for the rest of the session. Abandoning marks the step
abandoned so the pipeline can continue where that is possible.

![A harness failure gate for Claude Code and an execution failed gate](../images/console/gate-harness-failure.png)

### Budget reached

Every spawn passes a budget check in `Planner::push`. Once spent money reaches the budget plus any raises, the
planner opens this gate instead of the spawn. Running executions finish; nothing new starts. `raise` adds the
dollars written in the answer (a leading `$` is accepted), or the original budget again when the text is empty or
not a positive number. `stop` fails the session with the amount spent.

![The budget gate with the amount spent, the budget, and Raise the budget and Stop the session buttons](../images/console/gate-budget.png)

## YOLO: the engine answers

YOLO means the orchestrator decides everything. It can be the workspace default (`yolo.default`) or toggled per
session at any time, taking effect from the next gate or tool call.

With YOLO on, the planner emits a `YoloAnswer` step for every open gate except permission asks and the budget
gate. For each gate, `judge_input::yolo_plan` in
[`judge_input.rs`](../../crates/ostra-engine/src/judge_input.rs) returns one of three things: a fixed answer with a
stated reason, a call to the YOLO-answer judge with a JSON schema for that gate's answer, or nothing.

| Gate | YOLO answer |
| --- | --- |
| `open_questions` | YOLO judge. Takes each question's recommended option unless the research gives a specific reason for another. |
| `spec_approval`, `plan_approval` | YOLO judge. Approves only with a fact-check PASS; rejects with feedback when the artifact plainly omits part of the request. |
| `stuck` | YOLO judge. States a fact only if the context supplies one, else leaves the phase blocked. |
| `closing_gate` | Fixed: no tests and no docs, unless the request already asked for them (Rules T2, T3). |
| `fact_check_recurring` | Fixed: `another-round` while fewer than six FAILs in a row, then `stop`. |
| `review_cap` | Fixed: `another-pass`. In practice the loop rarely gets here, because the YOLO review budget is ten passes and then the Resolve judge takes over. |
| `phase_blocked` | Fixed: `leave`. Independent work continues (Rule D9). |
| `execution_failed` | Fixed: `retry`, until the same agent has failed three times; then `abandon`. |
| `harness_failure` | Fixed: `native`. |
| `skill_approval` | Fixed: the proposal's default dispositions. |
| `permission` | Answered inside the execution: the session behaves as `bypass`, so the ask never waits. |
| `budget_reached` | None. The gate stays open for the user. |

The judge's answer is not trusted blindly. `yolo_answer_from_judge` turns it into a gate answer and enforces what
must stay true: every question has an answer, and an approval stands only when the fact-check passed. The fold
then applies the same checks it applies to a human answer.

YOLO changes who answers, never what must be true:

- Layer 1 guards (write scope, state ownership, report path, lesson gate, build streak, self-protection) still
  deny.
- The user's explicit deny rules still apply.
- Approval still requires a fact-check PASS.
- BLOCKER security findings are still removed before the session can complete.
- The budget is never raised, because spending more is the user's decision.

Every YOLO answer is an event with its reason. The completion report ends with a "Decided for you" section listing
each one, and a push notification fires at completion and when a phase is blocked.

A session with YOLO on shows a banner that names what still applies:

![A running session with the YOLO is on banner](../images/console/yolo.png)

At the end, the completion report lists each decision YOLO made:

![A completion report with Stages not run and Decided for you](../images/console/completion.png)

### The YOLO review loop

Under YOLO a review loop gets ten passes (`YOLO_REVIEW_BUDGET`) instead of three. At the budget the planner asks
the Resolve-review judge rather than opening a gate. The judge reads the phase file, the full review ledger, the
open findings, and the counts of recent passes, and picks one:

- `fix`: it writes one exact instruction per open finding (file, line, change, and why earlier attempts failed).
  The engine runs one fix pass with only those instructions and allows one verification review.
- `block`: the findings are not converging, or need a fact only the user has. The phase is blocked.

After a `fix` round, if findings are still open, the engine compares the count with the count before the round.
If it went down, the judge is asked again. If it did not, the phase is blocked with the ledger path, and
independent work continues. This follows Ultracode's `hooks/review-cap.js`.

## Every judge

Judges are the only places a model makes an orchestration decision. Each is a short prompt in
[`assets/judges/`](../../assets/judges/), an output struct with a JSON schema in
[`crates/ostra-engine/src/judge.rs`](../../crates/ostra-engine/src/judge.rs), and an input builder in
`judge_input.rs` that decides exactly what the judge sees. Judges run on the `judge` route, which resolves to the
`advanced` tier by default. Each decision is stored as an event with its input summary and reason, and the board shows
it as "Ostra chose X because Y".

| Judge | Asked when | Decides | Prompt |
| --- | --- | --- | --- |
| Classify | The session starts | Category, projects in scope, research tasks, opt-ins, title | `classify.md` |
| Sufficiency | Research finished with `Not covered` items | For each item: needed or not, plus a research task when needed | `sufficiency.md` |
| Track | Research for an `IMPLEMENT` request finished and no track was forced | `light` (build from the research) or `full` (spec and plan first) | `track.md` |
| Stakes | A full-track `IMPLEMENT` spec is approved | `low` (skip the plan), `medium`, or `high` | `stakes.md` |
| Feedback | A feedback round needs routing: the session has a spec or several projects in scope | `requirement_change` or `implementation_detail`, and one `{project, instruction}` target per project it changes | `feedback.md` |
| Route answer | A user's free text lands at a phase (stuck fact, blocked retry) | `requirement_change`, `implementation_detail`, or `stage_choice` | `route-answer.md` |
| Rescue | An agent returned `stuck` | `rerun` with a stated fact, `explore` to find it, or `gate` to ask the user | `rescue.md` |
| Resolve review | A review loop hit its budget under YOLO | `fix` with per-finding instructions, or `block` | `resolve-review.md` |
| YOLO answer | A gate opens under YOLO and its plan is `Judge` | The gate's answer, in the schema for that gate | `yolo-answer.md` |
| Completion | Nothing is left to run | The completion report in Markdown | `completion.md` |

Each prompt tells the judge which way to lean when unsure, and why:

- **Classify** picks the category that runs more of the pipeline, because a needed stage that was skipped costs a
  wrong result and an unneeded stage costs one round.
- **Sufficiency** marks an item needed, because a missed dependency shows up as a wrong spec after approval.
- **Track** resolves to `light`, because the user reviews the built result and can send feedback, while a spec
  round costs several approvals. It picks `full` only on a research finding it can name.
- **Stakes** resolves upward, because skipping the plan removes the phase review that catches a wrong sequence.
- **Feedback** resolves to `requirement_change` when a spec exists, because a spec that disagrees with the code
  misleads every later stage.
- **Route answer** resolves to `requirement_change`, because a stale spec corrupts every stage after it.
- **Rescue** never picks a plain retry, and after two rescues of the same phase with the same diagnostic it picks
  `gate`, because neither rescue changed the failure.
- **YOLO answer** never invents a requirement, business rule, or fact, and picks the answer that sets work aside
  over one that guesses.

The fold guards judge output as it guards agent output. A Classify result that does not match its schema fails
the session with a clear message; a Rescue or Resolve decision that arrives for a loop that has since moved on is
ignored.

### What a route decision does

The Route-answer judge exists because an answer only reaches a later agent if it lands in the artifact that agent
reads.

- `requirement_change`: the phase stops, the answer goes into the spec, and Rule D10 runs: spec revision, spec
  approval, plan revision.
- `implementation_detail`: the text goes to the implementer or fix agent as an instruction. The spec does not
  change.
- `stage_choice`: the text only chooses stages. The spec does not change (Rule T5).

## Overriding a judge

Some decisions can be overridden from the board, with a replacement answer, while overriding is still safe.
`SessionState::can_override` decides:

| Judge | Can be overridden until |
| --- | --- |
| Classify | The first research task starts and before any phase has done work |
| Track | The spec has run or any phase has started |
| Stakes | The plan stage has run or any phase has started |
| Sufficiency | The spec has run |
| All others | Never |

The limits exist because an override replays the decision into the fold. Replacing the category after research
started, or the stakes after phases were built from them, would leave work in the log that the new decision says
should not exist. The fold checks the same condition again when the override event arrives, so a late override
is ignored even if a client sent it.

Overriding Track to `full` clears the inline phases the light track created; overriding it to `light` creates
them. Overriding Stakes clears the inline phases a `low` decision created. Overriding Sufficiency removes the research
tasks the earlier decision added that have not started.

The Decisions tab of a session lists each judge decision with its reason and the input it was based on:

![The Decisions tab with a Classify decision and a Stakes decision](../images/console/decisions.png)

## Measuring the judges

A conformance fixture proves what the engine does with a judge's answer; it cannot say whether a model gives
the right answer. The routing evals do that. [`tests/evals/judges.toml`](../../tests/evals/judges.toml) holds
cases for the Classify, Track, and Feedback judges, each a request about Ostra's own source with the route a
careful engineer would pick. Track and Feedback cases carry research documents, a spec, and phase reports
written from the real code, and some are counter-cases that pull the other way, so a prompt change that fixes
one case and breaks its opposite shows up.

[`crates/ostra-server/tests/judge_evals.rs`](../../crates/ostra-server/tests/judge_evals.rs) folds each case into
the session state the engine would have when it asks the judge, builds the input with the same `judge_input`,
and calls the judge several times per model, because one run of a model says little about the next:

```bash
OSTRA_EVAL_MODELS=anthropic:claude-opus-5-5,anthropic:claude-sonnet-5 OSTRA_EVAL_RUNS=3 \
  cargo test -p ostra-server --test judge_evals -- --ignored --nocapture
```

It prints the pass count and the spread of answers per case and model, then accuracy, the number of cases every
run agreed on, and cost per model, and writes the answers with each judge's reasons to `target/evals/`. The
live test is ignored by default because it calls paid models; it fails only when calls fail, since a model's
answers vary from run to run. An offline test in the same file checks that every case still builds its session,
so a case broken by an engine change fails in the normal suite. `OSTRA_EVAL_CASES` runs the cases whose id
contains its value.

## Where to look in the code

| To see | Read |
| --- | --- |
| Where each gate opens | `Planner` methods in [`plan.rs`](../../crates/ostra-engine/src/plan.rs): `spec_flow`, `plan_flow`, `loop_steps`, `closing_stages`, `exec_failed_gate`, `push` for the budget |
| What an answer does | `on_gate_answered` in [`state.rs`](../../crates/ostra-engine/src/state.rs) |
| What a judge decision does | `on_decision` in `state.rs` |
| YOLO answers per gate | `yolo_plan` and `yolo_answer_from_judge` in [`judge_input.rs`](../../crates/ostra-engine/src/judge_input.rs) |
| Answer validation | `validate_answer` in [`runner.rs`](../../crates/ostra-engine/src/runner.rs) |
| Fixtures | `yolo_answers_gates_and_extends_review_budget`, `t2_yolo_answers_the_closing_gate`, `d9_blocked_phase_removes_dependents`, and `approval_without_pass_is_ignored_by_the_fold` in [`tests/conformance/main.rs`](../../tests/conformance/main.rs) |
