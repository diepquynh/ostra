# The planner

Ostra's engine keeps two jobs apart: the decision about what to do, and the work itself. The planner reads the
state of a session and returns a list of steps. The runner does those steps and appends events for the results.
The new events change the state, and then the planner runs again. This loop moves each session from the first
Classify judge to the completion report.

This page explains these topics:

- What the planner is.
- Why the planner has no side effects.
- How the runner changes steps into work.
- How the conformance fixtures prove that each pipeline rule holds.

Read [The event log](event-log.md) first, because it explains the source of the state.

## A pure function

The whole planner is one function in `crates/ostra-engine/src/plan.rs`:

```rust
pub fn next_steps(s: &SessionState, ctx: &PlanCtx) -> Vec<Step>
```

The function takes the folded session state and a small context, and returns steps. It reads no files and
calls no model. It starts no process, writes no event, and reads no clock. If you call it two times with the
same inputs, you get the same list.

This design is important for three reasons:

- **The rules are in one place.** Each orchestration rule from HANDOVER section 8.2 and the Ultracode
  orchestrator is a branch in this file. A comment next to the branch gives its rule ID. Examples are Rule D1
  "full-track IMPLEMENT always passes through Spec", Rule M2 "one implement pipeline per project", and Rule D9
  "a failed phase removes its dependents". To find why Ostra did something, read this file.
- **Models do not control the pipeline.** A model never decides that the spec is good enough for a plan. A
  model never decides that three review passes are enough. The planner decides from the structured data that
  agents submitted. Models only answer the named judgment questions (Classify, Sufficiency, Stakes, and the
  other judges). These answers also come to the planner as events.
- **A test needs no running parts.** A test builds an event list by hand, folds it, calls `next_steps`, and
  compares the result. The test needs no provider, no server, and no database.

### What the planner may know

The session state holds almost all the facts that the planner needs, because the fold keeps each fact in the
log. Some facts that are important to the plan are not in the log. These facts go to the planner only through
`PlanCtx`:

```rust
/// Facts from outside the event log the planner needs: each project's format command.
pub struct PlanCtx {
    pub format_commands: BTreeMap<String, Option<String>>,
    /// The session budget from workspace settings. `None` means no limit.
    pub budget_usd: Option<f64>,
}
```

Each time the runner plans, it builds a new `PlanCtx` from the current workspace settings and the
`project.toml` of each project. Thus, if you change the session budget or a format command during a session,
the next planning round sees the change. The struct is small on purpose. With each field, the result of the
planner can depend on a fact that a fixture does not control. Thus, a new field needs a reason.

## Steps

A `Step` is one unit of work that the runner can do:

| Step | What the runner does |
| --- | --- |
| `Judge` | Builds the input of the judge from the state and calls the judge route. Validates the JSON against the schema of the judge, and appends `DecisionMade`. |
| `Spawn` | Resolves the route of the agent (executor, model, effort) and builds the typed spawn block. Waits for an execution slot, runs the executor, and appends `ExecutionStarted` and then `ExecutionFinished`. |
| `OpenGate` | Appends `GateOpened`. Then the session waits for you, or for YOLO. |
| `YoloAnswer` | In YOLO mode, answers an open gate with a fixed answer or with the YOLO judge. Appends `GateAnswered` with source `yolo` and the reason of the judge. |
| `PlanDocs` | Measures the tracked source of a project for each module-map area, and puts the areas into groups for the writers. Appends `DocsPlanned` with these areas, the areas that the current part of the book records, and the areas that the changes of this session touched (rule B9). |
| `WriteBook` | Merges the documentation parts and the architecture of the session into the workspace book. Writes the book files under `.ostra/docs/<book>/`, then appends `BookWritten` (rule B5). |
| `Command` | Runs the format command of the project or `git add` on the changed files of a phase. Appends `CommandRan`. |
| `Autofix` | Applies the review findings that have an exact fix text (`Change \`x\` to \`y\` on line N`). Appends `AutofixApplied`. |
| `AnnounceBlocked` | Appends `PhaseBlocked`. This event also sends a push notification. |
| `FinishInit` | Ends the init of a project that the session created (rule O4). Makes sure that `INVENTORY.md` and a valid `project.toml` exist. Then it marks the project as initialized and appends `ProjectInitFinished`. If a file is missing or not valid, it appends `InitStepFailed` against the generate-inventory run. Then the advisor examines the problem (rule O5). |
| `RecordInitProblem` | Appends `InitStepFailed` for an init step that finished but gave no usable result. An example is a detect step with no slices. Then the advisor examines the problem (rule O5). |
| `ResolveWorkflow` | Reads the workflow files of the workspace and resolves the named workflow, or the default workflow for the category (Rule WF1). Checks that the workflow can run (`check_runnable`, Rule WF8), then appends `WorkflowResolved`. If the workflow cannot run, it appends `SessionFailed`. See [Workflows](workflows.md). |
| `HandleResult` | Asks the plugin that owns the result contract of a run to handle the result. Appends `ResultHandled`, or a `fail` outcome when the plugin cannot handle it (Rule PL5). See [Plugins](plugins.md). |
| `DecideStage` | Asks the stage logic of a plugin for the next decision about its workflow stage. Appends `StageDecided`, or a `fail` decision when the plugin cannot decide (Rule PL3). See [Plugins](plugins.md). |
| `SkipStage` | Appends `StageSkipped` for a workflow node whose conditions are false after the nodes that it waits for are done (Rule WB5). See [Workflows](workflows.md#conditions-and-skipping). |
| `RunNode` | Runs the function of a transform node, or the model call of a prompt node through the judge path, on the resolved inputs of the node. Appends `NodeRan` with the output or the error, and the cost of the call (Rules WB2 and WB3). The budget guard holds the step of a prompt node, the same as a spawn. See [Workflows](workflows.md#transform-nodes). |
| `Complete` | Writes the completion report to the session folder and appends `SessionCompleted`. |
| `Fail` | Appends `SessionFailed`. |

Each step ends with an append. The only permanent effect of a step on the session is the events that it adds.
The files that agents write in the repository and the session folder are the other permanent effect. These files
are the work product, not pipeline state.

### Why the planner returns several steps

`next_steps` returns each step that can start now, not only the next step. If the Classify judge gives two
research tasks, the planner returns two explore spawns together (Rule M1, parallel fan-out). If a plan has ready
phases in two different projects, the two implement loops start (Rules D6 and M3). The runner starts all the
steps. The slot limiter decides how many of them run at the same time.

### Keys: never start a step twice

The planner runs each time the runner appends an event, and this occurs frequently. The planner can ask for the
review of phase 2. Before the `ExecutionStarted` event of that review is in the log, the planner can run more
times. Each time, it asks for the same review. The runner must not start the review two times.

Each step has a key that identifies it:

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

The key of a spawn is its purpose, for example `{"kind":"review","phase":2,"tests":false,"iteration":1}`. Two
requests for the same review pass of the same phase have the same key. The second review pass has a different
key. The key of an initializer spawn also names its project. This is necessary because a session that created
two projects runs two inits, and their detect steps have the same purpose.

The runner keeps a set of in-flight keys for each session. It skips each step whose key is in the set. The key
leaves the set only after the step finishes and the runner appends its result. At that time, the state is
different, and the planner does not ask for the step again.

The planner also uses keys in its own output. `push` drops a step whose key is already in the output list.
Thus, two branches of the planner that reach the same result give one step.

### Guards applied to every spawn

`Planner::push` is the only entry point for steps. Thus, the rules that apply to each spawn are in it:

- **Budget (CLAUDE.md pattern 8).** This rule applies when `PlanCtx.budget_usd` is set. The spend of the session
  comes from its finished executions. If this spend reaches the budget plus your increases, the planner changes
  a spawn into a `BudgetReached` gate. The running executions finish, and nothing new starts. YOLO never answers
  this gate, because the decision to spend more is yours.
- **A created project waits for its init (rule O4).** The planner drops each spawn in a project that the session
  created until the init of that project ends. The initializer and advisor runs of the init are the exceptions.
  The planner also drops the format, staging, and autofix steps of the project. Research in the project also
  waits, but work in other projects does not wait. The planner asks again on its next pass. Thus, a held step
  starts after the init ends.
- **Resume after pause (Rule P2).** If the session has a paused run for the same purpose, the planner marks the
  spawn to resume that run. Then the runner continues that execution and does not start a new one.
- **A run that waits for a message (Rule SM3).** A native run that paused itself ended with status `waiting`.
  Its stage sees this run as a paused run. The planner drops the next spawn of the stage until a message for the
  run is ready. Then the planner marks the spawn to resume the run in place. The runner gives the messages to the
  run as the resume note.
- **Pair loops continue conversations (Rules H5 to H7).** The planner marks a fact-check round, a re-pass, a fix,
  and a re-review to continue the conversation of the run before them. The source is
  `SessionState::continuation`. A rule can tell the planner to start a new conversation. The planner drops a
  spawn that continues a conversation with a live run until that run ends. Thus, a conversation never has two
  live runs.

After the stage logic, a coordination pass handles the messages between subagents (HANDOVER 10.8) and the
plugin results:

- A run that returned a plugin contract and has no handled outcome gets a `HandleResult` step (Rule PL5).
  Nothing reads such a result before its plugin handles it.
- If a native run ended `waiting` and a message for it is ready, the planner resumes the run in place (Rule SM3).
  The exception is a stage spawn in the same pass that already resumes the run.
- A subagent whose run ended `ok`, `stuck`, or `handoff` and that got a message gets a new run. This run
  continues its conversation with its own tools (`Message` purpose, Rule SM4).
- A message that starts a helper spawns that helper (`Helper` purpose, Rule SM7). If the contract of the helper
  is `research`, the helper runs as a research task with that agent.

A running run needs no step. Its executor takes its messages at its next turn boundary. The executor of a
waiting harness or programmatic run wakes that run (Rule SM2).

At the top of `run`, before all stage logic, this check occurs:

```rust
// Rule P1: a paused session starts nothing, not even a YOLO answer.
if !s.created || s.is_terminal() || s.paused {
    return;
}
```

## How the planner walks a session

`Planner::run` uses the same order as the pipeline diagram in HANDOVER section 8.1. The order is:

1. **YOLO first.** In YOLO mode, each open gate gets a `YoloAnswer` step. The exceptions are budget gates, the
   failure gate of an execution that you stopped (Rule P4), and a custom stage that failed in its last round.
   These gates stay open for you. Permission asks also get no step, because the runner allows them first.
2. **Init sessions** use their own flow: detect, scouts, propose, skill approval, generate skills, and generate
   the inventory.
3. **A named workflow** gets a `ResolveWorkflow` step first, because its base sets the category (Rule WF1).
4. **No category yet** means a `Classify` judge. Answers that wait for the Route answer judge get their judge step
   here. If context that you added waits for this judge, all other work stops. The planner returns only those
   judge steps until the judge decides (Rule C2).
5. **The workflow.** A new session that has no recorded workflow gets only a `ResolveWorkflow` step. A QUICK
   ANSWER session is the exception, because it runs no workflow. A session from a log that Ostra wrote before
   workflows existed runs the built-in workflow of its category.
6. **Explore tasks** spawn in each stage, because a rescue can add a research task in the middle of a build.
7. **The workflow walk.** QUICK ANSWER keeps its own flow. All other categories walk the stages of their workflow
   in dependency order (`workflow_flow`). A stage runs when all stages in its `after` list are done. A built-in
   stage calls the same function that the fixed pipeline called (`builtin_stage`):
   - Research runs `explore_complete`.
   - Track and stakes ask their judges.
   - Spec and plan run `spec_flow` and `plan_flow`.
   - Build runs `phases` and the phase stages.
   - Feedback runs `implementation_review` (Rule F1).
   - Closing runs `closing_stages`.

   A custom stage runs its agent or asks its plugin. See [Workflows](workflows.md).

   Each agent that a built-in stage spawns comes from `SessionState::agent_for(stage, contract)` (Rules WF8 and
   PL4). This is the agent that the workflow binds to that contract in that stage. If the workflow binds no
   agent, it is the agent of the standard plugin for the contract (`Standard::default_for`). The planner never
   names an agent. A work loop records the contracts of its work and fix runs (`WorkLoop::work` and `fix`), not
   agents. Examples are `implementation`, `prompt`, and `tests`. The fold creates loops at Classify time, before
   Ostra resolves the workflow, so the planner binds the agent when it spawns. Steps outside all built-in stages
   use the standard agent for their contract (`default_agent`). Examples are a quick answer and the init of a
   created project.
8. **Completion** occurs when all stages are done, nothing runs, and no gate is open. First the `Completion`
   judge runs. Then `Complete` runs with the report that the judge wrote.

Each stage function returns whether its stage is finished. Later stages run only when the earlier stages are
finished. This is how the planner enforces a rule such as D1. `spec_flow` does not start without a research
document. A workflow must also keep research before the spec (Rule WF2). Thus, no path goes from explore to plan
without research. The built-in workflow of each category chains its built-in stages in the order of the fixed
pipeline. Thus, a workspace without workflow files plans exactly as before, and the conformance fixtures prove it.

The phases function shows the style. This is the full scheduler for implement loops:

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

A phase with a failed dependency is in `removed` (Rule D9). A ready phase starts only if no other loop runs in
its project (Rule M2). Phases in different projects start together (Rule M3). A phase that is already in its
loop gets the next item that its loop needs: a review, an autofix, a fix pass, a `git add`, or a gate. The fold
finds this item from the findings of the last review. Thus, `loop_steps` usually reads the `next` field of the
loop and changes it into a step.

## The runner and the driver loop

The runner (`crates/ostra-engine/src/runner.rs`) does the steps. Each live session has one driver. The driver is
a Tokio task that runs this loop:

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

The loop does these actions:

1. It plans.
2. It starts each new step in its own task.
3. It sleeps until a change occurs.

A change is an append or the end of a step. An append wakes the loop because `Inner::append` ends with
`wake.notify_one()`. When the session ends, the loop stops.

This design has these results:

- **The driver never blocks on work.** A spawn can run for an hour. During that time the driver sleeps. It wakes
  for each event that the other executions of the session append.
- **A failed step does not stop the session.** Its error becomes a `Note` event that you can read on the board,
  and then the planner runs again. If the failure did not change the state, the next planning round asks for
  the same step again.
- **The runner has no hidden state machine.** The runner holds in-flight keys, cancellation tokens, and
  permission waiters. All of them describe work in progress in this process. They are lost at a restart. This
  is not a problem, because recovery derives all the state again from the log.

### Execution slots

`limits.max_parallel_executions` in the workspace settings sets the maximum number of executions that run at
the same time in the workspace. `perform_spawn` takes a slot before all other work. It holds the slot until the
execution ends. When the slot is dropped, the slot becomes free and the waiters wake.

A harness or programmatic run that waits for a message is the only exception (Rule SM3). It gives back its slot
during the wait. It takes a slot again before it continues. Without this exception, one slot can cause a deadlock
between the sender and the subagent that it waits for.

The planner can ask for six scouts together. The runner starts them when slots become free. For this reason,
fan-out stages also keep their own caps (`init::MAX_SCOUTS` is 6). The slot limiter sets the maximum number at the
same time, and the caps set the maximum total.

### What a spawn does

`perform_spawn` is the longest step. It does these actions in this order:

1. It waits for a slot. Then it makes sure that the session did not end or pause during the wait.
2. It resolves the route. The route gives the executor and the model. The executor is native or a harness, such
   as Claude Code or Codex. The route comes from the tier of the agent, the complexity of the phase, and your
   settings. If a route does not resolve, the execution becomes `denied` with the reason. It is never a silent
   fallback.
3. It builds the typed spawn parameters through the spawn factory. It records in `params` other facts that the
   fold will need later, for example the auto-fixable rule IDs for a review.
4. It appends `ExecutionStarted` with the parameters and the rendered spawn block. A spawn that resumes a paused
   run of the same agent appends `ExecutionResumed` with the id of that run (Rule P2). The run keeps its row,
   executor, model, report path, and parameters. The fold opens the run again, and does not count a new run, a
   new fact-check round, or new work in its loop. The new usage of the run adds to its earlier spend.
5. It runs the executor with a cancellation token. If a pause or "send now" context cancelled the execution, the
   runner records the result as `interrupted`, not `cancelled`. Then the fold knows that it must resume or run
   the execution again.
6. It appends `ExecutionFinished` with the result and its submit payload.

### Gate answers from you

When you answer a gate in the browser, the API calls the engine. The engine makes sure that the shape of the
answer agrees with the gate (`validate_answer`). For example, an approval gate needs an approval, and a
review-cap gate needs a choice. Then the engine appends `GateAnswered`. The append wakes the driver, and the
planner continues from the new state. Your answer uses the same path as all other facts.

## Proving the rules: conformance fixtures

Each rule that the planner implements has a fixture in `tests/conformance/main.rs`. The file holds 160 fixtures
today. A fixture builds an event history, folds it, and checks the summaries of the steps that the planner
returns. The planner is pure. Thus, a fixture runs in microseconds and needs no model, executor, or database.

With the `H` helper, a history is short to write:

- `H::new` appends a `SessionCreated` for the projects that you name.
- `h.classify("IMPLEMENT", &["p"])` appends the Classify decision.
- `h.run(prefix, submit)` finds the spawn step whose summary starts with `prefix`. It appends the
  `ExecutionStarted` of that step, and an `ExecutionFinished` with the submit payload that you give.
- `h.open_gate(kind)` and `h.answer(gate, answer)` do the same for gates.
- `h.summaries()` folds the history and returns the steps of the planner in a short form. Examples are
  `spawn generate-spec spec#1` and `gate review_cap`.
- Ready-made starting points (`H::explored`, `H::spec_approved`, `H::plan_approved`) build the common prefixes
  from the same calls.

The fixture below is for the review loop. It covers these Step 4 rules:

- The findings go into three groups.
- The engine applies the auto-fixable findings.
- Only HIGH and MEDIUM findings go to the fix agent.
- The fourth pass is a gate.

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

The fixture shows this sequence of events. The user approves a plan with one phase. The implementer finishes,
and the reviewer reports three findings. The only step of the planner is the autofix, because the rule of the
`C1` finding is in the auto-fixable list of the project. The helper records `auto_fixable_ids: ["C1"]` in the
params of each review, at the same location as the real runner. After the autofix, the fix pass gets
`PHASE-REQ-1` but not the LOW `C9`. After two more rounds, the loop used its three passes. Then the planner
opens a gate and does not start a fourth review.

The next fixture in the same file is `hard21_blocker_has_no_cap`. It runs five review passes with a BLOCKER
finding. It asserts that no review-cap gate opens. A security block has no cap, and no gate answer can cancel it
(Hard rule 21).

Fixtures are the contract for changes to the engine. A new rule or a changed rule needs a new or changed fixture
in the same change. The rule ID must be in the section comment of the fixture. It must also be at the line in
`plan.rs` or `state.rs` that implements the rule. To see what Ostra does in a situation, write the history as a
fixture and print `h.summaries()`. This runs the planner without a server or a model.

### Beyond fixtures

Fixtures test the planner and the fold. Other tests examine the runner around them:

- `crates/ostra-engine/tests/runner.rs` runs a real `Engine` with fake services and executors. Thus, the driver
  loop, the slots, and the appends are real.
- `crates/ostra-engine/tests/recover.rs` and `pause.rs` test restart recovery, and pause and continue.
- `crates/ostra-server/tests/e2e.rs` runs the full server. A scripted provider acts as each agent.

## Adding behavior

Put new pipeline behavior in the planner and the fold. The contributor notes give this procedure:

1. If the behavior depends on a past occurrence, make sure that an event records it. If the behavior depends on
   a fact from outside the log, record that fact in an event when it is used.
2. Change the fold to derive the necessary state from those events.
3. Add a branch to the planner that changes that state into a step.
4. Add a fixture that builds the history and asserts the steps.

Change the runner only for a new kind of step. Do not put logic in the runner that decides the next action.
Fixtures cannot see such logic, and a restart loses it.

## Where to look in the code

| What | Where |
| --- | --- |
| `next_steps`, `PlanCtx`, `Step`, `key()` | `crates/ostra-engine/src/plan.rs` |
| Loop state the planner reads (`WorkLoop`, `LoopNext`) | `crates/ostra-engine/src/state.rs` |
| Driver loop, `perform`, slots, `validate_answer` | `crates/ostra-engine/src/runner.rs` |
| Spawn parameters from planner inputs | `crates/ostra-engine/src/factory.rs` |
| The workflow walk (`workflow_flow`, `builtin_stage`, `build_done`, `phase_stages`) | `crates/ostra-engine/src/plan.rs` |
| Custom stage fold and actions | `crates/ostra-engine/src/workflow.rs` |
| Plugin stage planning and the stage view | `crates/ostra-engine/src/plugin_stage.rs` |
| Messages in the fold, `wakes_due`, `continuations_due`, `helpers_due` | `crates/ostra-engine/src/coord.rs` |
| Conformance fixtures and the `H` helper | `tests/conformance/main.rs` |
