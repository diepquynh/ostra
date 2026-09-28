# The planner

Ostra's engine splits "deciding what to do" from "doing it". The planner looks at a session's state and returns
a list of steps. The runner performs those steps and appends events for what happened. The new events change
the state, and the planner runs again. This loop drives every session from the first Classify judge to the
completion report.

This page explains what the planner is, why it has no side effects, how the runner turns steps into work, and
how the conformance fixtures prove that each pipeline rule holds. It builds on [The event log](event-log.md),
which explains where the state comes from.

## A pure function

The whole planner is one function in `crates/ostra-engine/src/plan.rs`:

```rust
pub fn next_steps(s: &SessionState, ctx: &PlanCtx) -> Vec<Step>
```

It takes the folded session state and a small context, and returns steps. It reads no files, calls no model,
starts no process, writes no event, and checks no clock. Call it twice with the same inputs and you get the same
list.

This matters for three reasons:

- **The rules are in one place.** Every orchestration rule from HANDOVER section 8.2 and the Ultracode
  orchestrator (Rule D1 "full-track IMPLEMENT always passes through Spec", Rule M2 "one implement pipeline per project",
  Rule D9 "a failed phase removes its dependents", and the rest) is a branch in this file, with its rule ID in
  a comment next to it. When you want to know why Ostra did something, this is where you read.
- **Models do not orchestrate.** A model never decides that the spec is good enough to plan from, or that three
  review passes is enough. The planner decides from the structured data agents submitted. Models only answer
  the named judgment questions (Classify, Sufficiency, Stakes, and the other judges), and even those answers
  arrive as events the planner reads.
- **It can be tested without anything running.** A test builds an event list by hand, folds it, calls
  `next_steps`, and compares the result. No provider, no server, no database.

### What the planner may know

The session state is almost everything the planner needs, because the fold keeps every fact the log has. A few
facts live outside the log and matter to planning. They reach the planner through `PlanCtx` and nowhere else:

```rust
/// Facts from outside the event log the planner needs: each project's format command.
pub struct PlanCtx {
    pub format_commands: BTreeMap<String, Option<String>>,
    /// The session budget from workspace settings. `None` means no limit.
    pub budget_usd: Option<f64>,
}
```

The runner builds a fresh `PlanCtx` every time it plans, from the current workspace settings and each project's
`project.toml`. So if you change the session budget or a project's format command while a session runs, the
next planning round sees the change. Keeping this struct small is deliberate: every field is a way for the
planner's answer to depend on something a fixture does not control, so a new field needs a reason.

## Steps

A `Step` is one unit of work the runner knows how to perform:

| Step | What the runner does |
| --- | --- |
| `Judge` | Builds the judge's input from the state, calls the judge route, validates the JSON against the judge's schema, and appends `DecisionMade`. |
| `Spawn` | Resolves the agent's route (executor, model, effort), builds the typed spawn block, waits for an execution slot, runs the executor, and appends `ExecutionStarted` and later `ExecutionFinished`. |
| `OpenGate` | Appends `GateOpened`. The session then waits for you, or for YOLO. |
| `YoloAnswer` | Under YOLO, answers an open gate with a fixed answer or the YOLO judge, and appends `GateAnswered` with source `yolo` and the judge's reason. |
| `Command` | Runs the project's format command or `git add` on a phase's changed files, and appends `CommandRan`. |
| `Autofix` | Applies review findings whose fix text is exact (`Change \`x\` to \`y\` on line N`), and appends `AutofixApplied`. |
| `AnnounceBlocked` | Appends `PhaseBlocked`, which also sends a push notification. |
| `Complete` | Writes the completion report to the session folder and appends `SessionCompleted`. |
| `Fail` | Appends `SessionFailed`. |

Notice that every step ends in an append. A step's only lasting effect on the session is the events it adds.
Files agents write in the repository and the session folder are the other lasting effect, and those are the
work product, not pipeline state.

### Why the planner returns several steps

`next_steps` returns every step that can start now, not only the next one. When the Classify judge produces two
research tasks, the planner returns two explore spawns at once (Rule M1, parallel fan-out). When a plan has
ready phases in two different projects, both implement loops start (Rules D6 and M3). The runner starts them all,
and the slot limiter decides how many actually run at the same time.

### Keys: never start a step twice

The planner runs every time any event is appended, which is often. Between "the planner asked for the review of
phase 2" and "the review's `ExecutionStarted` is in the log", the planner may run several more times and ask for
the same review each time. The runner must not start it twice.

Each step has a key that names it:

```rust
/// Identity used by the runner so a step already in flight is not started twice.
pub fn key(&self) -> String {
    match self {
        Step::Judge { judge, subject } => format!("judge:{}:{}", judge.as_str(), subject...),
        Step::Spawn(s) => format!("spawn:{}", serde_json::to_string(&s.purpose)...),
        Step::OpenGate { payload, .. } => format!("gate:{}:{}", payload.kind_str(), gate_owner(payload)),
        Step::YoloAnswer { gate } => format!("yolo:{gate}"),
        ...
    }
}
```

A spawn's key is its purpose, for example `{"kind":"review","phase":2,"tests":false,"iteration":1}`. Two
requests for the same review pass of the same phase have the same key, while the second review pass has a
different one. The runner keeps a set of in-flight keys per session and skips any step whose key is already
there. The key leaves the set only when the step's work has finished and been appended, at which point the
state has changed and the planner no longer asks for it.

The planner also uses keys on itself: `push` drops a step whose key is already in its output list, so two
branches of the planner that reach the same conclusion produce one step.

### Guards applied to every spawn

`Planner::push` is the single entry point for steps, so rules that apply to every spawn live there:

- **Budget (CLAUDE.md pattern 8).** When `PlanCtx.budget_usd` is set and the session's spend from finished
  executions reaches the budget plus whatever you raised it by, a spawn becomes a `BudgetReached` gate instead.
  Running executions finish; nothing new starts. YOLO never answers this gate, because spending more is your
  decision.
- **Resume after pause (Rule P2).** When the session has a paused run for the same purpose, the spawn is marked
  to resume it, so the runner continues that execution instead of starting a new one.

And at the top of `run`, before any stage logic:

```rust
// Rule P1: a paused session starts nothing, not even a YOLO answer.
if !s.created || s.is_terminal() || s.paused {
    return;
}
```

## How the planner walks a session

`Planner::run` follows the same order the pipeline diagram in HANDOVER section 8.1 does. In outline:

1. **YOLO first.** Under YOLO, every open gate gets a `YoloAnswer` step, except permission asks (the live
   execution answers those) and budget gates.
2. **Init sessions** take their own flow: detect, scouts, propose, skill approval, generate skills, generate the
   inventory.
3. **No category yet** means a `Classify` judge.
4. **Explore tasks** spawn whatever the stage, because a rescue can add a research task in the middle of a
   build.
5. **The category's path.** RESEARCH completes after explore. SPEC adds the spec flow. PLAN adds the plan flow.
   IMPLEMENT asks the `Track` judge after research. The full track adds the spec flow, the Stakes judge, and
   the plan flow unless stakes are low; the light track goes to the phases directly. Both then run the phases,
   the implementation review gate with its feedback rounds (Rule F1), and, once you accept, the closing stages.
   VERIFY, PROMPT, and QUICK CHANGE go straight to phases. UNIT TEST goes to the closing stages.
6. **Completion** once nothing is running and no gate is open: first the `Completion` judge, then `Complete`
   with the report it wrote.

Each stage function returns whether its stage is finished, and later stages run only when earlier ones say yes.
That is how a rule like D1 is enforced: `spec_flow` refuses to start with no research document, and there is no
code path from explore to plan that skips it.

Phases show the style. This is the whole scheduler for implement loops:

```rust
fn phases(&mut self) {
    let removed = removed_phases(s);
    let mut busy: BTreeSet<String> = /* projects with an implement loop in progress */;
    for p in s.phases.values() {
        if removed.contains(&p.info.id) { continue; }
        let l = &p.impl_loop;
        if l.is_idle() {
            // Rule M2: one implement pipeline per project at a time.
            if busy.contains(&p.info.project) || !deps_passed(s, p) { continue; }
            busy.insert(p.info.project.clone());
            self.loop_work(p, false, WorkKind::Initial, None);
            continue;
        }
        self.loop_steps(p, false);
    }
}
```

A phase whose dependency failed is in `removed` (Rule D9). A ready phase starts only if its project has no other
loop running (Rule M2). Phases in different projects start together (Rule M3). A phase already in its loop gets
whatever its loop needs next: a review, an autofix, a fix pass, a `git add`, or a gate. The fold has already
worked out what that is, from the last review's findings, so `loop_steps` mostly reads the loop's `next` field
and turns it into a step.

## The runner and the driver loop

The runner (`crates/ostra-engine/src/runner.rs`) performs steps. Each live session has one driver, a Tokio task
that runs this loop:

```rust
async fn drive(self: Arc<Self>, id: SessionId, live: Arc<Live>) {
    loop {
        let steps = {
            let st = lock(&live.state);
            if st.is_terminal() { break; }
            let ctx = self.plan_ctx(&st);
            next_steps(&st, &ctx)
        };
        for step in steps {
            let key = step.key();
            if !lock(&live.inflight).insert(key.clone()) { continue; }
            tokio::spawn(async move {
                if let Err(e) = inner.perform(&sid, step).await {
                    let _ = inner.append(&sid, SessionEvent::Note { message: format!("A step failed: {e}") });
                }
                lock(&l.inflight).remove(&key);
                l.wake.notify_one();
            });
        }
        live.wake.notified().await;
    }
}
```

Read it as: plan, start every new step in its own task, then sleep until something changes. "Something changes"
means an append, because `Inner::append` ends with `wake.notify_one()`, or a step finishing. When the session
ends, the loop exits.

A few consequences of this shape:

- **The driver never blocks on work.** A spawn can run for an hour; the driver is asleep, and wakes for every
  event that execution's siblings append.
- **A failed step does not crash the session.** Its error becomes a `Note` event you can read on the board, and
  the planner runs again. If the failure left the state unchanged, the next planning round asks for the same step
  again.
- **There is no hidden state machine in the runner.** The runner holds in-flight keys, cancellation tokens, and
  permission waiters, all of which describe work in progress in this process. None of it survives a restart,
  and none of it needs to, because recovery re-derives everything from the log.

### Execution slots

`limits.max_parallel_executions` in workspace settings caps how many executions run at once across the whole
workspace. `perform_spawn` takes a slot before it does anything else and holds it for the life of the execution;
dropping the slot frees it and wakes waiters. The planner can ask for six scouts at once, and the runner will start
them as slots free up. This is why fan-out stages keep their own caps too (`init::MAX_SCOUTS` is 6): the slot
limiter bounds concurrency, and the caps bound the total.

### What a spawn does

`perform_spawn` is the longest step. In order, it:

1. Waits for a slot, then checks the session has not ended or paused in the meantime.
2. Resolves the route: which executor (native, or a harness such as Claude Code or Codex) and which model, from
   the agent's tier, the phase's complexity, and your settings. A route that does not resolve becomes a
   `denied` execution with the reason, never a silent fallback.
3. Builds the typed spawn parameters through the spawn factory, and records extra facts the fold will need
   later in `params` (the auto-fixable rule IDs for a review).
4. Appends `ExecutionStarted` with the parameters and the rendered spawn block. A spawn that resumes a paused
   run of the same agent appends `ExecutionResumed` with that run's id instead (Rule P2): the run keeps its
   row, executor, model, report path, and parameters, and the fold reopens it without counting a new run, a new
   fact-check round, or new work in its loop. What the run already spent is the base its new usage adds to.
5. Runs the executor with a cancellation token. If a pause or "send now" context cancelled it, the result is
   recorded as `interrupted` rather than `cancelled`, so the fold knows to resume or re-run it.
6. Appends `ExecutionFinished` with the result and its submit payload.

### Gate answers from you

When you answer a gate in the browser, the API calls into the engine, which checks that the answer's shape
matches the gate (`validate_answer`: an approval for an approval gate, a choice for a review-cap gate, and so on)
and appends `GateAnswered`. The append wakes the driver, and the planner picks up from the new state. Your
answer takes the same path as every other fact.

## Proving the rules: conformance fixtures

Every rule the planner implements has a fixture in `tests/conformance/main.rs`, which holds 76 of them today.
A fixture builds an event history, folds it, and checks the summaries of the steps the planner returns. Because
the planner is pure, a fixture runs in microseconds and needs no model, executor, or database.

The `H` helper makes histories short to write. `H::new` appends a `SessionCreated` for the projects you name.
`h.classify("IMPLEMENT", &["p"])` appends the Classify decision. `h.run(prefix, submit)` finds the spawn step
whose summary starts with `prefix`, appends its `ExecutionStarted`, and appends an `ExecutionFinished` with the
submit payload you give. `h.open_gate(kind)` and `h.answer(gate, answer)` do the same for gates. `h.summaries()`
folds the history and returns the planner's steps in compact form, such as `spawn generate-spec spec#1` or
`gate review_cap`. Canned starting points (`H::explored`, `H::spec_approved`, `H::plan_approved`) build the
common prefixes, each out of the same calls.

Here is the fixture for the review loop, which covers the Step 4 rules: findings split three ways, auto-fixable
ones are applied by the engine, only HIGH and MEDIUM reach the fix agent, and the fourth pass is a gate.

```rust
#[test]
fn review_loop_splits_autofix_fix_and_caps_at_three() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    h.run("spawn implementer", impl_submit(1, &["src/a.rs"]));
    // C1 is auto-fixable in this project; PHASE-REQ never is, whatever its Fix text.
    h.run(
        "spawn code-reviewer",
        review(&[
            finding("LOW", "C1"),
            finding("HIGH", "PHASE-REQ-1"),
            finding("LOW", "C9"),
        ]),
    );
    assert_eq!(h.summaries(), vec!["autofix phase 1"]);
    h.ev(SessionEvent::AutofixApplied { project: "p".into(), phase: 1, tests: false,
        applied: vec!["x".into()], failed: vec![] });
    let fix = h.spawn_step("spawn implementer phase 1 fix");
    let text = fix.inputs.instructions.unwrap();
    assert!(
        text.contains("PHASE-REQ-1") && !text.contains("C9"),
        "only HIGH and MEDIUM go to the fix agent"
    );
    h.run("spawn implementer phase 1 fix", impl_submit(1, &["src/a.rs"]));
    h.run("spawn code-reviewer review phase 1 #2", review(&[finding("HIGH", "R2")]));
    h.run("spawn implementer phase 1 fix", impl_submit(1, &["src/a.rs"]));
    h.run("spawn code-reviewer review phase 1 #3", review(&[finding("MEDIUM", "R3")]));
    assert_eq!(h.summaries(), vec!["gate review_cap"], "the 4th pass is a gate");
    let g = h.open_gate("review_cap");
    h.answer(&g, GateAnswer::Choice { option: "another-pass".into(), text: None });
    assert_eq!(h.summaries(), vec!["spawn implementer phase 1 fix"]);
}
```

Read it as a story: a plan with one phase is approved, the implementer finishes, the reviewer reports three
findings. The planner's only step is the autofix, because the `C1` finding's rule is in the project's
auto-fixable list (the helper records `auto_fixable_ids: ["C1"]` in every review's params, the same place the
real runner puts it). After the autofix, the fix pass gets `PHASE-REQ-1` and not the LOW `C9`. Two more rounds
later the loop has used its three passes, and the planner opens a gate rather than a fourth review.

The same fixture file has a neighbor, `hard21_blocker_has_no_cap`, which runs five review passes with a BLOCKER
finding and asserts that no review-cap gate ever opens, because a security block has no cap and no gate answer
can waive it (Hard rule 21).

Fixtures are the contract for changing the engine. A new rule or a changed rule means a new or changed fixture
in the same change, and the rule's ID appears both in the fixture's section comment and at the line in
`plan.rs` or `state.rs` that implements it. If you want to see what Ostra does in a situation, the fastest way
is often to write the history as a fixture and print `h.summaries()`.

### Beyond fixtures

Fixtures test the planner and the fold. Other tests cover the runner around them:

- `crates/ostra-engine/tests/runner.rs` runs a real `Engine` with fake services and executors, so the driver
  loop, slots, and appends run for real.
- `crates/ostra-engine/tests/recover.rs` and `pause.rs` cover restart recovery and pause and continue.
- `crates/ostra-server/tests/e2e.rs` runs the whole server with a scripted provider playing every agent.

## Adding behavior

The planner and fold are where new pipeline behavior goes. The pattern from the contributor notes:

1. If the behavior depends on something that happened, make sure an event records it. If it depends on a fact
   from outside the log, record that fact in an event when it is used.
2. Teach the fold to derive the state you need from those events.
3. Add a branch to the planner that turns that state into a step.
4. Add a fixture that builds the history and asserts the steps.

The runner changes only when there is a new kind of step to perform. Logic inside the runner that decides what
happens next would be invisible to fixtures and lost on restart, so it does not go there.

## Where to look in the code

| What | Where |
| --- | --- |
| `next_steps`, `PlanCtx`, `Step`, `key()` | `crates/ostra-engine/src/plan.rs` |
| Loop state the planner reads (`WorkLoop`, `LoopNext`) | `crates/ostra-engine/src/state.rs` |
| Driver loop, `perform`, slots, `validate_answer` | `crates/ostra-engine/src/runner.rs` |
| Spawn parameters from planner inputs | `crates/ostra-engine/src/factory.rs` |
| Conformance fixtures and the `H` helper | `tests/conformance/main.rs` |
