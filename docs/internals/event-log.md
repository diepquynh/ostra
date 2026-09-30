# The event log

Ostra keeps every session as an append-only list of events. Nothing else records what a session is doing. The
board you see in the browser, the stage label in the session list, the cost figure, what the engine starts
next, and what happens after a crash all come from reading that list back. This page explains how the log
works, why it is built this way, and what that means for you when a server dies in the middle of a build.

## Why a log and not a status column

A pipeline run lasts hours and passes through dozens of model executions, judge calls, gates, and shell
commands. A lot can go wrong partway: the laptop sleeps, the provider rate-limits, you kill the server to change
a setting. If Ostra kept its progress as a set of mutable fields ("current stage: review, iteration: 2"), every
crash would leave some fields updated and some not, and nothing would say which.

Ostra records facts instead. "Execution X started with this spawn block." "Execution X finished with status
`ok` and this submit payload." "The user answered gate G with approve." Each fact is written once and never
changed. The current state of a session is whatever you get when you replay those facts from the first one.
This is event sourcing, and it gives Ostra three properties that the rest of the design relies on:

- **Restart is replay.** After a crash, the server reads the events and has the exact state it had before,
  because the state was never stored anywhere else.
- **History is complete.** Every decision a judge made, every gate answer, and whether a person or YOLO gave
  it, stays in the log. The UI's "Ostra chose X because Y" cards are events, not reconstructions.
- **Tests need no model.** A test can write a list of events by hand, fold it, and ask what happens next. That
  is how every pipeline rule is tested (see [The planner](planner.md)).

The pipeline state does not live in any model's context either. HANDOVER section 11.2 puts it plainly: no
compaction checkpoint is needed, because the engine holds the state and each agent only gets the inputs its
step needs.

## What an event looks like

Events are the `SessionEvent` enum in `crates/ostra-core/src/event.rs`. They are stored as JSON rows in the
`events` table of the workspace database (`.ostra/workspace.db`), one row per event, keyed by session and a
sequence number:

```sql
INSERT INTO events (session_id, seq, type, payload, at) VALUES (?1, ?2, ?3, ?4, ?5)
```

The sequence number is computed inside an immediate SQLite transaction as one more than the session's current
maximum, so two events in one session can never share a number
(`crates/ostra-store/src/workspace.rs`, `append_event`).

There are about twenty event kinds. Grouped by what they record:

| Group | Events | What they record |
| --- | --- | --- |
| Session lifecycle | `SessionCreated`, `SessionCompleted`, `SessionFailed` | The request, options, projects, and folders at the start; the completion report or the reason it stopped. |
| What you did | `RequestAmended`, `SessionPaused`, `SessionResumed`, `YoloSet` | Context added mid-session (queued or sent now, with `routed` when the Route answer judge decides it, Rule C2), pause and continue (Rules P1 and P2), turning YOLO on or off. |
| Judgment | `DecisionMade`, `DecisionOverridden` | A judge's output, its reason, and the input summary it saw; a user's override of it. |
| Executions | `ExecutionStarted`, `ExecutionResumed`, `ExecutionFinished` | Agent, purpose, stage, executor, model, the full spawn parameters and rendered spawn block, and later the result with its submit payload, token usage, and cost. `ExecutionResumed` reopens an execution the pause interrupted (Rule P2), so one execution can have several results in the log; the last one counts, and it includes what the earlier parts spent. |
| Gates | `GateOpened`, `GateAnswered` | A question the pipeline asks, and its answer with its source: `user`, `yolo`, or `engine`. |
| Engine work | `CommandStarted`, `CommandRan`, `AutofixApplied`, `DocsPlanned`, `BookWritten` | Format and `git add` commands with their exit code and output tail; review findings the engine applied itself; how a project's part of the book is split among writers, measured from the disk because the fold cannot read it (rule B9); the documentation book the engine wrote, with the projects whose parts it holds and any write error (rule B5). |
| Projects | `ProjectCreated`, `ProjectInitFinished`, `InitStepFailed` | A project an implementer created with `ProjectCreate`, with every fact of the call (rule O3); the successful end of its init inside the session (rule O4); an init step whose result the engine found unusable, such as no slices or a missing inventory, which the advisor looks at next (rule O5). |
| Outcomes | `SecurityBlock`, `PhaseBlocked`, `Note` | A BLOCKER finding (Hard rule 21), a phase that cannot continue, and free-text notes such as "A step failed". |

Two fields do more work than they appear to. `ExecutionStarted.purpose` (an `ExecPurpose` such as
`Implement { phase: 2, work: Fix }` or `Review { phase: 2, tests: false, iteration: 3 }`) is what the fold keys
on to know which loop an execution belongs to. `ExecutionFinished.result.submit` is the structured payload the
agent passed to its `submit_<agent>` tool. The engine never reads an agent's final chat message, only this
payload, so the log holds everything the engine acted on.

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

`apply` is one large `match` on the event kind. It fills in the pieces the rest of the engine asks about: the
explore tasks and their research documents, the spec and plan tracks (runs, fact-check passes, pending answers,
approval), each phase's implement loop and test loop, open gates, decisions, executions, the paused flag, and
the running cost. The `SessionState` struct is large because it is the whole picture of a session, derived and
never stored.

Nothing else in the engine changes a `SessionState`. The runner never sets "stage = review" directly. When a
review finishes, the runner appends `ExecutionFinished`, and the fold works out that the review loop for phase 2
now has three HIGH findings, one auto-fixable, and that the next step is the autofix and then a fix pass.

### The fold must be pure

The fold is a pure function of the log. It reads no files, no settings, no clock, and no environment. Given the
same events it produces the same state every time, on any machine, a year later.

This rule has a cost. Some decisions depend on facts from outside the log. When that happens, Ostra copies the
fact into an event at the moment it is used, so the fold can read it back later.

The clearest case is auto-fixable review findings. Each project's `project.toml` lists review rule IDs that the
engine may fix by itself, from the finding's exact `Change \`x\` to \`y\` on line N` text. When a review
finishes, the fold must split its findings into auto-fixable, sent-to-fix-agent, and BLOCKER. If the fold read
`project.toml` at that moment, a user who edited the file between a crash and a restart would change how an old
review folds, and replay would give a different answer than the original run did.

So the runner records the list when it starts the review, inside the execution's `params`:

```rust
// crates/ostra-engine/src/runner.rs, perform_spawn
if matches!(req.purpose, ExecPurpose::Review { .. }) && let Value::Object(map) = &mut params {
    let ids = profile.as_ref().map(|p| p.auto_fixable_ids()).unwrap_or_default();
    map.insert(AUTO_FIXABLE_PARAM.into(), serde_json::to_value(ids).unwrap_or_default());
}
```

and the fold reads it from the event, not the file:

```rust
// crates/ostra-engine/src/state.rs, loop_finished
// Recorded at spawn from the project's Review Rule Set, so the fold stays a pure function
// of the event log.
let autofix_ids: BTreeSet<String> = rec.params.get(AUTO_FIXABLE_PARAM) ...
```

The same reasoning applies elsewhere. A session's projects and their paths are copied into `SessionCreated`.
A project created mid-session arrives the same way: `ProjectCreated` carries its key, path, stack, purpose,
base requirements, and the execution that created it, because the fold cannot read `workspace.toml` to learn
that the project exists, and the init that runs later builds its `User focus:` from those recorded facts. An
attached file's absolute location is computed from those recorded paths. Uploads are moved into the session
folder and their names, paths, and sizes are recorded in the event, so a later change to the workspace does not
change what an old session saw.

## One place appends

Every event goes through a single function, `Inner::append` in `crates/ostra-engine/src/runner.rs`. It does
five things in order, while holding the session's state lock:

1. **Store.** Write the event to SQLite and get its sequence number.
2. **Materialize.** Update the query tables that mirror the log: `gates`, `decisions`, and the session row
   (category, status, lane, stage label, projects, cost, title). These tables make list views and the API fast;
   they are never read back into the fold.
3. **Fold.** Apply the stored event to the in-memory `SessionState`.
4. **Broadcast.** Send the event on the engine's broadcast channel. The WebSocket hub routes it to every
   browser subscribed to `session:<id>`, with its sequence number, so the UI can apply it in order. A matching
   Web Push notice goes out for events a person should see while away: a gate opened (unless YOLO will answer
   it), a phase blocked, the session completed or failed.
5. **Wake.** Notify the session's driver that the state changed, so the planner runs again.

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

Keeping all five in one function removes a whole class of bug. There is no path where an event is stored but
the in-memory state misses it, or where the UI hears about a change the database does not have. The comment on
the lock is the invariant: because appends in one session are serialized by the state lock, the order in the
database is the order the fold saw.

## Crash recovery

When `ostra serve` starts, it calls `Engine::recover` before it accepts requests. For each session in the
workspace it:

1. Loads the session's events and folds them.
2. Finds every execution the fold still thinks is running. The process that ran it is gone, so Ostra appends
   `ExecutionFinished` with status `interrupted` and the error "The server restarted while this execution ran."
   The usage the execution reported while it ran was stored as it streamed, so the interrupted result keeps
   what it spent. Your cost figure does not drop after a restart. A harness run that was waiting for another
   subagent's answer with its process alive is recorded as `waiting` instead, so the answer later resumes its
   harness session rather than re-running it (Rule H2).
3. Finds every open permission gate. The execution that asked is gone, so the engine answers it with deny,
   source `engine`, and says why.
4. Starts the session's driver again if the session has not ended.

From there, nothing special happens. The fold treats an `interrupted` execution like any other result, and the
planner decides what to do next. For a phase's work loop, the fold sets the loop's next action to a re-run
(`WorkKind::Rerun`) with the same instructions:

```rust
// crates/ostra-engine/src/state.rs
if status == ExecutionStatus::Interrupted {
    // Re-run with the same spawn block (HANDOVER 11.2).
    l.next = match in_flight { LoopNext::Work { instructions, .. } => LoopNext::Work {
        kind: WorkKind::Rerun, instructions }, ... };
    return;
}
```

The spawn factory then adds one line to the re-run's task: "An earlier run of this step ended before it
finished. Check the progress log and continue." The implementer keeps a progress log in the session folder as
it works, so a re-run reads where the last run got to rather than starting the phase over. An interrupted spec,
plan, EPA, or docs run is marked as needing a run, and the planner starts it again.

`crates/ostra-engine/tests/recover.rs` tests exactly this. It starts a session whose executor reports $0.50 of
usage and then hangs, opens the same database from a second `Engine` as if the first process had crashed, calls
`recover`, and checks that the execution finished as `interrupted`, kept its $0.50, and that its step runs
again.

### Stopping a session while the server is down

Recovery re-runs every interrupted execution, which costs money. If a session was running away when the server
died, stop it before you start the server again:

```bash
ostra stop s_01a0cdab436570b0b522e10ce263a904
```

`ostra stop` works directly on the database (`stop_session_offline`). It folds the session, appends
`ExecutionFinished` with status `cancelled` for each running execution (keeping what each spent), and appends
`SessionFailed`. When the server starts, the fold sees a session that has ended, and recovery leaves it alone.
The command refuses to run while `server.json` in the data folder says a server is alive, because two
processes appending to one session would break the ordering invariant above.

## Pause and continue

Pausing is also events, which is why a paused session survives a restart paused.

- **Pause (Rule P1).** `SessionPaused` sets the fold's `paused` flag and marks every running execution as being
  interrupted for a pause. The runner cancels those executions and denies their waiting permission asks. A
  harness gets an Esc first, so its CLI saves its session whole. While paused the planner returns nothing: no
  spawn, judge, command, gate, or YOLO answer. You can still answer gates and add context; both wait for
  continue.
- **Continue (Rule P2).** When an execution that was interrupted for a pause finishes, the fold records it in
  `resume_from`, keyed by what the execution was doing (for example `work:2:false` for phase 2's implement
  loop). `SessionResumed` clears the flag, the planner asks for that step again, and it attaches the old
  execution as `resumes`. The runner then continues the old conversation instead of starting fresh: a native
  execution from its stored transcript in the `messages` table, a harness execution through its CLI's resume
  command with the stored session id and the prompt "Continue the workflow."
- **Context added while paused.** A resumed conversation would not see new context, so `RequestAmended` clears
  `resume_from`. Those steps re-run from their spawn blocks with the updated request instead.

The difference between a restart and a pause shows in the result. A restart re-runs a step with a note to check
the progress log, because the old process and its conversation are gone. A pause resumes the same conversation,
because Ostra stopped it on purpose and saved it.

## Questions between subagents

Subagent coordination (HANDOVER 10.8) is also events. `AgentAsked` records a question: who asked, the target (a
new helper of an agent, or a subagent ID), and the text. `AgentReplied` records an answer. `MessageDelivered`
records that Ostra handed a question or an answer to a run that waits. A run that asks and then ends records
`ExecutionFinished` with status `waiting`.

The fold derives the rest. A helper's answer is its explore submit. A subagent that fails before it answers, or a
run that ends without replying to the question it was given, answers with that failure. Which run waits for
which message is a function of these events and the execution results, so a replay rebuilds it, and the planner
reads it to decide what to wake. [Subagents that talk to each other](agents.md#subagents-that-talk-to-each-other)
walks through the flow.

## The tables beside the log

The workspace database holds more than events (HANDOVER 11.1). Each table below is either derived from the log
or holds data too large or too frequent to put in it:

| Table | Holds | Relation to the log |
| --- | --- | --- |
| `events` | Every session event, in order. | The source of truth. |
| `sessions` | One row per session: request, category, status, lane, stage label, cost. | Refreshed on every append, for the session list. |
| `gates`, `decisions` | Open and answered gates; judge calls with their reasons and override flag. | Materialized from events, for queries. |
| `executions` | Per execution: route, spawn parameters, status, native session id, token and cost totals. | Written as the execution runs, so cost is known before it finishes. |
| `messages` | Native execution transcripts. | Used to resume a paused native execution and to show the Activity view. |
| `tool_calls` | Every tool call with its policy decision and the rule behind it. | An audit trail, not read by the fold. |

Project memory stays out of the workspace database, in each project's `.ostra/memory/knowledge.sqlite3`, because
it belongs to the repository and outlives any one workspace.

## Where to look in the code

| What | Where |
| --- | --- |
| Event and gate types | `crates/ostra-core/src/event.rs` |
| The fold and `SessionState` | `crates/ostra-engine/src/state.rs` |
| `Inner::append`, `recover`, pause, stop | `crates/ostra-engine/src/runner.rs` |
| Storage of events | `crates/ostra-store/src/workspace.rs` |
| WebSocket routing of events | `crates/ostra-server/src/ws.rs` |
| Restart and pause tests | `crates/ostra-engine/tests/recover.rs`, `crates/ostra-engine/tests/pause.rs` |

Next: [The planner](planner.md) explains how Ostra decides what to do with the state the fold produces.
