# The event log

Ostra keeps every session as an append-only list of events. Nothing else records the work of a session. Ostra
reads this list back to get these items:

- The board that you see in the browser.
- The stage label in the session list.
- The cost figure.
- The next step that the engine starts.
- The recovery after a crash.

This page tells how the log works and why Ostra uses it. It also tells what occurs when a server stops in the
middle of a build.

## Why a log and not a status column

A pipeline run lasts hours. It goes through dozens of model executions, judge calls, gates, and shell
commands. Many problems can occur during a run. For example, the laptop sleeps, the provider rate-limits, or
you kill the server to change a setting. Ostra does not keep its progress as a set of mutable fields, such as
"current stage: review, iteration: 2". With such fields, a crash leaves some fields updated and some fields
not updated, and nothing tells which.

Ostra records facts. "Execution X started with this spawn block." "Execution X finished with status `ok` and
this submit payload." "The user answered gate G with approve." Ostra writes each fact one time and never
changes it. The current state of a session is the result of a replay of these facts from the first one. This
method is event sourcing. It gives Ostra three properties, and the rest of the design uses them:

- **Restart is replay.** After a crash, the server reads the events and gets the exact state that it had
  before. This is true because Ostra stores the state in no other place.
- **History is complete.** The log keeps each decision of a judge and each gate answer. It also records if a
  person or YOLO gave the answer. The "Ostra chose X because Y" cards in the UI are events, not reconstructions.
- **Tests need no model.** A test can write a list of events by hand, fold it, and ask for the next step.
  Ostra tests each pipeline rule in this way (see [The planner](planner.md)).

The pipeline state is also not in the context of a model. HANDOVER section 11.2 states that no compaction
checkpoint is necessary. The reason is that the engine holds the state, and each agent gets only the inputs
that its step needs.

## What an event looks like

Events are the `SessionEvent` enum in `crates/ostra-core/src/event.rs`. Ostra stores them as JSON rows in the
`events` table of the workspace database (`.ostra/workspace.db`). Each event has one row. The key of a row is
the session and a sequence number:

```sql
INSERT INTO events (session_id, seq, type, payload, at) VALUES (?1, ?2, ?3, ?4, ?5)
```

Ostra calculates the sequence number in an immediate SQLite transaction. The number is the current maximum of
the session plus one. Thus, two events in one session cannot have the same number
(`crates/ostra-store/src/workspace/events.rs`, `Events::append`).

There are 41 event kinds. This table puts them in groups by the data that they record:

| Group | Events | What they record |
| --- | --- | --- |
| Session lifecycle | `SessionCreated`, `SessionCompleted`, `SessionFailed` | At the start: the request, options, projects, folders, and the workflow that the session asked for. At the end: the completion report or the reason for the stop. |
| Workflow | `WorkflowResolved`, `StageDecided`, `StageSkipped`, `NodeRan` | The workflow that the session runs, with each stage resolved (Rule WF1). The decision of a plugin about the next step of its stage (Rule PL3). A node that Ostra skipped because its conditions were false (Rule WB5). The output or error of a transform node or a prompt node, its round, and the cost of a prompt node (Rules WB2 and WB3). |
| Messages | `MessageSent`, `AgentWaiting`, `MessagesDelivered` | A message between subagents, and whether its sender waits. A run that paused and sent no message. The messages that Ostra gave to a run, with a notice when no message will come (Rule SM8). |
| What you did | `RequestAmended`, `AmendmentWithdrawn`, `SessionPaused`, `SessionResumed`, `YoloSet` | Context that you added during the session. It is queued or sent now, with `routed` when the Route answer judge decides it (Rule C2). Queued context that you took back before a step read it. Pause and continue (Rules P1 and P2). YOLO on or off. |
| Judgment | `DecisionMade`, `DecisionOverridden` | The output of a judge, its reason, and the input summary that it saw. An override of it by a user. |
| Executions | `ExecutionStarted`, `ExecutionResumed`, `ExecutionFinished`, `ExecutionSkipped`, `ExecutionSteered`, `SteerWithdrawn`, `ResultHandled` | Agent, purpose, stage, result contract, executor, model, the full spawn parameters, and the rendered spawn block. Later, the result with its submit payload, token usage, and cost. `ResultHandled` records what a plugin did with a result of its own contract (Rule PL5). `ExecutionResumed` opens again an execution that the pause interrupted (Rule P2). Thus, one execution can have more than one result in the log. The last result counts, and it includes the spend of the earlier parts. |
| Gates | `GateOpened`, `GateAnswered` | A question from the pipeline, and its answer with its source: `user`, `yolo`, or `engine`. |
| Engine work | `CommandStarted`, `CommandRan`, `AutofixApplied`, `DocsPlanned`, `BookWritten` | Format and `git add` commands with their exit code and output tail. Review findings that the engine applied itself. The split of the part of a project in the book among writers, measured from the disk because the fold cannot read it (rule B9). The documentation book that the engine wrote, with the projects whose parts it holds and the write error, if there is one (rule B5). |
| Projects | `ProjectCreated`, `ProjectInitFinished`, `InitStepFailed` | A project that an implementer created with `ProjectCreate`, with each fact of the call (rule O3). The successful end of its init in the session (rule O4). An init step whose result the engine cannot use, for example no slices or a missing inventory. The advisor examines this step next (rule O5). |
| Outcomes | `SecurityBlock`, `PhaseBlocked`, `Note` | A BLOCKER finding (Hard rule 21), a phase that cannot continue, and free-text notes, for example "A step failed". |

Two fields are more important than they look. The first is `ExecutionStarted.purpose`, an `ExecPurpose` such
as `Implement { phase: 2, work: Fix }` or `Review { phase: 2, tests: false, iteration: 3 }`. The fold uses this
field to find the loop of an execution. The second is `ExecutionFinished.result.submit`. It is the structured
payload that the agent gave to its `submit_<agent>` tool. The engine reads only this payload, never the final
chat message of an agent. Thus, the log holds all the data that the engine used.

`ExecutionStarted.contract` records the result contract the run submits (Rule CA5), because the fold reads a
result by its contract and the catalog that says which contract an agent returns is outside the log. A run logged
before contracts has none, and the fold gives it its agent's contract from the standard plugin, or `stage` for a
custom agent (`workflow::legacy_contract`), which is what those runs were. A run of a plugin contract is read only
after its plugin handled it: the planner asks with `Step::HandleResult`, the runner records the outcome as
`ResultHandled`, and the fold applies that outcome where a verdict would apply.

## The fold

A session's state is `SessionState::fold(events)` in `crates/ostra-engine/src/state.rs`:

```rust
pub fn fold(id: SessionId, events: &[StoredEvent]) -> Self {
    let mut s = SessionState::new(id);
    for e in events {
        s.apply(e);
    }
    s
}
```

`apply` is one large `match` on the event kind. It sets the data that the rest of the engine reads:

- The explore tasks and their research documents.
- The spec and plan tracks: runs, fact-check passes, pending answers, and approval.
- The implement loop and the test loop of each phase.
- Open gates, decisions, and executions.
- The paused flag and the running cost.

The `SessionState` struct is large because it holds the full state of a session. Ostra derives this state and
never stores it.

No other part of the engine changes a `SessionState`. The runner never sets "stage = review" directly. When a
review finishes, the runner appends `ExecutionFinished`. Then the fold calculates the new state. For example,
the review loop for phase 2 now has three HIGH findings, and one of them is auto-fixable. The next step is the
autofix, and then a fix pass.

### The fold must be pure

The fold is a pure function of the log. It reads no files, no settings, no clock, and no environment. For the
same events, it makes the same state each time, on all machines, and also one year later.

This rule has a cost. Some decisions use facts from outside the log. For such a decision, Ostra copies the
fact into an event at the time that it uses the fact. Then the fold can read the fact later.

The best example is auto-fixable review findings. The `project.toml` of each project lists review rule IDs.
The engine can fix findings for these rules itself, from the exact `Change \`x\` to \`y\` on line N` text of
the finding. When a review finishes, the fold must put its findings into three groups: auto-fixable,
sent-to-fix-agent, and BLOCKER. Think of a fold that reads `project.toml` at that time. A user can edit the file
between a crash and a restart. Then the edit changes the fold of an old review, and the replay gives a different
answer from the original run.

Thus, the runner records the list in the `params` of the execution when it starts the review:

```rust
// crates/ostra-engine/src/runner.rs, perform_spawn
if matches!(req.purpose, ExecPurpose::Review { .. }) && let Value::Object(map) = &mut params {
    let ids = profile.as_ref().map(|p| p.auto_fixable_ids()).unwrap_or_default();
    map.insert(AUTO_FIXABLE_PARAM.into(), serde_json::to_value(ids).unwrap_or_default());
}
```

The fold reads the list from the event, not from the file:

```rust
// crates/ostra-engine/src/state.rs, loop_finished
// Recorded at spawn from the project's Review Rule Set, so the fold stays a pure function
// of the event log.
let autofix_ids: BTreeSet<String> = rec.params.get(AUTO_FIXABLE_PARAM) ...
```

Ostra uses the same method in other places:

- Ostra copies the projects of a session and their paths into `SessionCreated`.
- A project that Ostra creates during the session comes in the same way. `ProjectCreated` holds its key, path,
  stack, purpose, base requirements, and the execution that created it. The fold cannot read `workspace.toml`
  to find the project. The init that runs later makes its `User focus:` from these recorded facts.
- Ostra calculates the absolute location of an attached file from these recorded paths.
- Ostra moves uploads into the session folder and records their names, paths, and sizes in the event. Thus, a
  later change to the workspace does not change the data that an old session saw.

Workflows follow the same rule. `SessionCreated.workflow` records what the session asked for: a workflow by name,
or the workspace's workflow for whatever category Classify picks. The runner reads the workflow files once, when
the planner asks for it, and records the resolved workflow, with `extends` and `remove` applied and every stage
listed, in `WorkflowResolved`. From then on the fold and the planner read only that event, so editing a workflow
file never changes a session already running. A plugin's stage logic is outside the log too, so its answers are
recorded as `StageDecided` events and the fold never calls the plugin. A transform node's output is recorded in
`NodeRan` rather than computed in the fold, so a later Ostra whose function behaves differently still folds an
old log the same way; a prompt node's answer and cost are recorded there for the same reason. A node skipped by
its conditions is recorded as `StageSkipped`. A session written before workflows has no `workflow` in
`SessionCreated`, and it runs Ostra's default workflow of its category, which is the same chain of stages the
pipeline always ran.

## One place appends

Each event goes through one function, `Inner::append` in `crates/ostra-engine/src/runner.rs`. It holds the
state lock of the session and does five steps in this sequence:

1. **Store.** Write the event to SQLite and get its sequence number.
2. **Materialize.** Update the query tables that copy the log: `gates`, `decisions`, and the session row
   (category, status, lane, stage label, projects, cost, title). These tables make list views and the API fast.
   The fold never reads them.
3. **Fold.** Apply the stored event to the in-memory `SessionState`.
4. **Broadcast.** Send the event on the broadcast channel of the engine. The WebSocket hub routes it, with its
   sequence number, to each browser that subscribes to `session:<id>`. Thus, the UI can apply the events in
   sequence. Some events are important to a person who is away. For these events, Ostra also sends a Web Push
   notice: a gate opened (but not when YOLO will answer it), a phase blocked, or the session completed or failed.
5. **Wake.** Tell the driver of the session that the state changed. Then the planner runs again.

```rust
/// Append an event: store it, fold it, broadcast it, refresh the session row, wake the driver.
fn append(&self, session: &SessionId, event: SessionEvent) -> Result<StoredEvent, EngineError> {
    let live = self.load(session)?;
    let stored = {
        // The state lock serializes appends per session, so seq order is fold order.
        let mut st = lock(&live.state);
        let stored = self.db.append_event(session, &event)?;
        self.materialize(session, &event)?;
        st.apply(&stored);
        ...
    };
    let _ = self.tx.send(EngineNotice::Event { session: session.clone(), stored: stored.clone() });
    self.push_for(session, &event);
    live.wake.notify_one();
    Ok(stored)
}
```

Ostra keeps all five steps in one function, and this prevents a full class of bugs. No path stores an event
but leaves it out of the in-memory state. No path tells the UI about a change that is not in the database. The
comment on the lock states the invariant. The state lock serializes the appends in one session. Thus, the
sequence in the database is the sequence that the fold saw.

## Crash recovery

When `ostra serve` starts, it calls `Engine::recover` before it accepts requests. For each session in the
workspace, it does these steps:

1. Loads the session's events and folds them.
2. Finds each execution that is still running in the fold. The process of that execution does not exist
   now. Thus, Ostra appends `ExecutionFinished` with status `interrupted` and the error "The server restarted
   while this execution ran." Ostra stored the usage of the execution during the stream. Thus, the interrupted
   result keeps its spend, and your cost figure does not decrease after a restart. A special case is a harness
   run that waited for a message with a live process. Ostra records this run as `waiting`. The message then
   resumes its harness session and does not run it again (Rule SM3).
3. Finds each open permission gate. The execution that asked does not exist now. Thus, the engine answers the
   gate with deny, source `engine`, and gives the reason.
4. Starts the driver of the session again if the session did not end.

After this, Ostra does nothing special. The fold uses an `interrupted` execution as a usual result, and the
planner decides the next step. For the work loop of a phase, the fold sets the next action of the loop to a
re-run (`WorkKind::Rerun`) with the same instructions:

```rust
// crates/ostra-engine/src/state.rs
if status == ExecutionStatus::Interrupted {
    // Re-run with the same spawn block (HANDOVER 11.2).
    l.next = match in_flight { LoopNext::Work { instructions, .. } => LoopNext::Work {
        kind: WorkKind::Rerun, instructions }, ... };
    return;
}
```

Then the spawn factory adds one line to the task of the re-run: "An earlier run of this step ended before it
finished. Check the progress log and continue." The implementer writes a progress log in the session folder
during its work. Thus, a re-run reads the point where the last run stopped, and it does not start the phase
again. Ostra marks an interrupted spec, plan, EPA, or docs run as a run to do again, and the planner starts it.

`crates/ostra-engine/tests/recover.rs` tests this behavior. The test does these steps:

1. It starts a session whose executor reports $0.50 of usage and then hangs.
2. It opens the same database from a second `Engine`, as if the first process crashed.
3. It calls `recover`.
4. It checks that the execution finished as `interrupted` and kept its $0.50, and that its step runs again.

### Stopping a session while the server is down

If a session spent too much when the server stopped, stop the session before you start the server again.
Recovery runs each interrupted execution again, and this costs money. Use this command:

```bash
ostra stop s_01a0cdab436570b0b522e10ce263a904
```

`ostra stop` works directly on the database (`stop_session_offline`). It does these steps:

1. It folds the session.
2. It appends `ExecutionFinished` with status `cancelled` for each running execution. Each execution keeps its
   spend.
3. It appends `SessionFailed`.

When the server starts, the fold sees a session that ended, and recovery does not change it. If `server.json`
in the data folder shows a live server, the command refuses to run. Two processes that append to one session
break the sequence invariant above.

## Pause and continue

A pause is also a set of events. Thus, a paused session stays paused after a restart.

- **Pause (Rule P1).** `SessionPaused` sets the `paused` flag of the fold. It marks each running execution as
  interrupted for a pause. The runner cancels these executions and denies their waiting permission asks. A
  harness gets an Esc first, so that its CLI saves its full session. When the session is paused, the planner
  returns nothing: no spawn, judge, command, gate, or YOLO answer. You can still answer gates and add context.
  Both wait for continue.
- **Continue (Rule P2).** An execution that a pause interrupted finishes. Then the fold records it in
  `resume_from`, with a key for the work of the execution (for example `work:2:false` for the implement loop
  of phase 2). `SessionResumed` clears the flag. The planner asks for that step again and attaches the old
  execution as `resumes`. The runner then continues the old conversation and does not start a new one. A
  native execution continues from its stored transcript in the `messages` table. A harness execution continues
  through the resume command of its CLI, with the stored session id and the prompt "Continue the workflow."
- **Context added during a pause.** A resumed conversation does not see new context. Thus, the release of the
  context clears `resume_from`, and these steps run again from their spawn blocks with the updated request.
  The exception is a run that holds a correction (below). It resumes with the correction, because under Queue,
  running work finishes on the old request.
- **Queued context (Rule C2).** Ostra holds a `RequestAmended` with `delivery: queue` that comes when an
  execution runs. The fold keeps it out of the request and marks it `held`. When an addition is held, the
  planner starts nothing. When the `ExecutionFinished` of the last running execution folds, Ostra releases each
  held addition. The addition goes to the Route answer judge or directly into the request, the same as
  unqueued context. Before the release, `AmendmentWithdrawn` takes it back, and no agent or judge reads it.
  Ostra releases context that you send now immediately, so you cannot withdraw it.
- **Corrections (Rule U2).** `ExecutionSteered` on a running execution records the text in the `steers` of the
  fold. It marks the run as interrupted for a correction. The finish of the run puts it in `resume_from`, the
  same as a pause. Then the planner asks for the step again. The runner resumes the same execution with the
  correction as its first message, in place of "Continue the workflow." On a paused execution, the correction
  only waits, marked `queued`, and `SteerWithdrawn` removes it before the session continues. `ExecutionResumed`
  clears it. After a correction stops a running run, you cannot withdraw it, because the run already stopped
  for it.

The result shows the difference between a restart and a pause. A restart runs a step again with a note to
check the progress log, because the old process and its conversation do not exist now. A pause resumes the
same conversation, because Ostra stopped it intentionally and saved it.

## Messages between subagents

Messages between subagents (HANDOVER 10.8) are also events. `MessageSent` records a message with these data:

- The run that sent it.
- The target: an existing subagent, or a new helper of an agent with its project and its result contract.
- The text.
- Whether the sender waits.

A helper whose contract is `research` runs as a research task, and its document joins the research of the
session. A target from a log before contracts has no contract. It was an `explore` helper, and the fold reads it
in the same way. `AgentWaiting` records a run that paused and sent no message. `MessagesDelivered` records that
Ostra gave a list of messages to one run, at a turn boundary or to wake it. It also records the notice that the
run got when no message could come. A native run that pauses also records `ExecutionFinished` with status
`waiting`.

The fold derives the other data (`crates/ostra-engine/src/coord.rs`):

- The subagent that receives a message.
- The result message of a helper, from its submit or from its failure.
- The runs that wait, and the run that each one waits for.
- The messages that are still queued.
- The senders to which a run owes a reply.

All of these are functions of the events and the execution results. Thus, a replay builds them again, and the
planner reads them to decide which run to wake or continue.
[Subagents that talk to each other](agents.md#subagents-that-talk-to-each-other) shows the flow step by step.

Logs from before messages still fold, because `on_coord_event` reads the old events into the same messages:

- `AgentAsked` is a `MessageSent` with `wait` set. It keeps the ID of the question.
- `AgentReplied` becomes a message from the run that replied back to the subagent that asked, with the ID
  `<question>-reply`.
- `MessageDelivered` of a question marks the question as delivered. `MessageDelivered` of an answer marks the
  reply or the result of the helper (`<question>-result`) as delivered, and it ends the wait of the asker.
- An execution with the old `Consult` purpose marks its question as delivered when it starts.

A finished session from before the change folds to no open message and no waiting run. Thus, it stays complete.

## The tables beside the log

The workspace database holds more than events (HANDOVER 11.1). Ostra derives each table below from the log,
or the table holds data that is too large or too frequent for the log:

| Table | Holds | Relation to the log |
| --- | --- | --- |
| `events` | Every session event, in order. | The source of truth. |
| `sessions` | One row per session: request, category, status, lane, stage label, cost. | Refreshed on every append, for the session list. |
| `gates`, `decisions` | Open and answered gates. Judge calls with their reasons and override flag. | Materialized from events, for queries. |
| `executions` | Per execution: route, spawn parameters, status, native session id, token and cost totals. | Written during the execution, so the cost is known before it finishes. |
| `messages` | Native execution transcripts. | Used to resume a paused native execution and to show the Activity view. |
| `tool_calls` | Every tool call with its policy decision and the rule behind it. | An audit trail, not read by the fold. |

Project memory is not in the workspace database. It is in the `.ostra/memory/knowledge.sqlite3` of each
project, because it is part of the repository and lives longer than one workspace.

## Where to look in the code

| What | Where |
| --- | --- |
| Event and gate types | `crates/ostra-core/src/event.rs` |
| The fold and `SessionState` | `crates/ostra-engine/src/state.rs` |
| Messages in the fold, and old coordination events | `crates/ostra-engine/src/coord.rs` |
| Workflow and plugin stage fold | `crates/ostra-engine/src/workflow.rs`, `crates/ostra-engine/src/plugin_stage.rs` |
| `Inner::append`, `recover`, pause, stop | `crates/ostra-engine/src/runner.rs` |
| Storage of events | `crates/ostra-store/src/workspace/events.rs` |
| WebSocket routing of events | `crates/ostra-server/src/ws.rs` |
| Restart and pause tests | `crates/ostra-engine/tests/recover.rs`, `crates/ostra-engine/tests/pause.rs` |

Next: [The planner](planner.md) tells how Ostra decides what to do with the state from the fold.
