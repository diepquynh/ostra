# Spend and limits

One Ostra session can start many executions:

- Some explores in parallel.
- A spec writer and a fact-checker.
- A planner.
- An implementer and a reviewer for each phase.
- More executions for tests and docs.

Init can start six scouts at the same time. Each execution can run on a top-tier model. Without limits, a
fan-out stage can start dozens of expensive executions in one minute. This page describes the controls that
prevent this:

- A limit on the number of executions that run at the same time.
- A spend budget for each session.
- A cap on each fan-out.
- The ways to stop a session that spends more than you want.

## The two limits

The two limits are under `limits` in the workspace settings. You set them on the Settings screen.

| Setting | Default | Meaning |
| --- | --- | --- |
| `limits.max_parallel_executions` | `3` | The number of executions that can run at the same time in this workspace, across all its sessions. Other spawns wait in sequence. |
| `limits.session_budget_usd` | `25.0` | The dollars that one session can spend before it pauses for your decision. `0` means no limit. |

Save-time validation rejects a parallel limit of 0. It also rejects a budget that is negative or not a number.

Ostra keeps the two values in the registry, not in `workspace.toml` (Rule A2). Ostra ignores a `[limits]`
table in the file and removes it at the next save. Without this rule, a repository that you cloned can
increase its own budget. [Settings and routing](settings-and-routing.md#why-some-settings-stay-out-of-the-folder)
explains the rule.

The two limits are on the General tab of Settings:

![The General settings tab with the Executions at once and Session budget fields](../images/console/settings-general.png)

## Parallel executions: the slot limiter

Each agent execution needs a slot before it starts. The limiter is a counter and a notifier on the engine of
the workspace, in [`crates/ostra-engine/src/runner.rs`](../../crates/ostra-engine/src/runner.rs):

```rust
/// Wait for a slot under the workspace's parallelism limit, re-read each time so a settings
/// change applies to waiting spawns.
async fn acquire_slot(self: &Arc<Self>) -> Slot {
    loop {
        let limit = self.services.workspace().limits.max_parallel_executions.max(1) as usize;
        {
            let mut n = lock(&self.slots);
            if *n < limit {
                *n += 1;
                return Slot(self.clone());
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), self.slot_free.notified()).await;
    }
}
```

The slot is a value whose `Drop` gives it back and wakes the waiters. Thus, Ostra releases the slot for all
ends of the execution: success, failure, cancellation, or a panic in the executor. A waiting spawn checks again
when a slot becomes free, and at least each two seconds. Each time, it reads the limit from settings. If you
increase the limit, waiting spawns start in two seconds or less, without a restart. If you decrease it, Ostra
cancels nothing. Executions that already run finish, and new ones wait until the count is below the new limit.

These rules tell which executions hold a slot:

- **Holds a slot:** each agent execution that a session spawns, on all executors, native or harness. The limit
  is for each workspace. Thus, two sessions in the same workspace share it.
- **Gives its slot back when it waits:** a harness run or a programmatic agent that paused for a message and
  waits with a live process (Rule SM3). It takes a slot again before Ostra gives it the message. A native run that
  pauses ends, so it holds nothing when it waits. Without this rule, a limit of one causes a problem. The waiting
  run holds the only slot, and the run that it waits for cannot start.
- **Does not hold a slot:** judge calls and side-panel quick answers. A judge call is one structured request
  that the engine makes between steps. A quick answer runs outside all sessions. Both are short, and by default
  they run on the `fast` or `balanced` tier.

After a spawn gets its slot, it reads the state of the session again. If the session paused or ended during
the wait, the spawn gives the slot back and starts nothing.

The status bar shows the count of running executions. Click the count to see the list:

![The running executions menu open from the status bar](../images/console/running-menu.png)

## How cost is counted

Each execution records its token usage during the run. The usage has these parts:

- Input tokens and output tokens.
- Cache reads.
- Cache writes. Ostra keeps the part written at the one-hour cache lifetime separate, because it costs more.
- Tool calls and build time.

Ostra calculates the cost in dollars from this usage and a price table, in
[`crates/ostra-core/src/pricing.rs`](../../crates/ostra-core/src/pricing.rs):

```rust
pub fn cost(model: &str, usage: &Usage, web_searches: u64) -> f64 {
    let Some(p) = price(model) else { return 0.0 };
    let r = p.rates(usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens);
    // input, output, cache reads, 5-minute writes, 1-hour writes, plus $0.01 per web search
    ...
}
```

The Cost screen shows the totals. It also shows the same metrics for each session, stage, agent, and executor,
for this week or all time:

![The Cost screen for all time with tables by session, stage, agent, and executor](../images/console/cost-alltime.png)

### Where prices come from

Prices come from the public [models.dev](https://models.dev) catalog. The server keeps a copy in the data
folder as `models-dev.json`. It installs the copy at start-up and refreshes it one time each day in the
background. If a refresh fails, the server keeps the cached prices and tries again one hour later.
`OSTRA_MODELS_DEV_URL` sets a different copy of the catalog for the fetch. An empty value stops the fetch, for
machines without network access.

Resellers can list the same model at different prices. In this case, Ostra uses the listing of the
first-party provider. For some models, the price changes above a prompt size, a context-length tier. Ostra
prices each request of such a model at the rate for the size of that request.

Ostra records a cost of $0 for a model that is not in the catalog. It also records $0 for all models before
it installs a catalog. Thus, on a machine that never fetched the catalog and has no cached copy, each execution
is free, and the budget never pauses a session. After the first execution, look at the cost of the session on
its board to make sure that the prices are available.

### Native and harness executions

- **Native executions** get usage from each provider response. The provider client prices each response when
  it arrives. Ostra also writes the cost of each response to the Activity feed as a `turn` delta. Ostra emits
  the delta after the thinking of the response and before its tool calls run. The execution screen shows the
  cost on the thinking summary of that response. It also gives each tool call of the response an equal part
  of the cost, marked with `~`. The mark is there because the provider bills a response as one unit: its
  prompt, thinking, and all the calls that it wrote. A response with no thinking and no calls, for example a
  compaction, gets its own line.
- **Harness executions** get usage from the transcript of the CLI. Ostra reads the transcript during the run of
  the CLI:
  - Claude Code writes usage for each assistant message. Ostra keeps the last usage that it saw for each
    message id. Thus, Ostra counts a message that streamed in parts one time.
  - Codex writes running totals. Ostra prices the difference from the last total that it saw.
  - Grok Build reports its own cost with each finished turn, and Ostra uses that figure.
  - The transcript of Antigravity holds no usage, so Ostra records its executions at $0.

### Stored as it runs

Ostra writes each usage update to the record of the execution when the update arrives. The cost in the
session list updates live. Thus, a server crash does not lose spend. When recovery marks an interrupted
execution, it keeps the usage that Ostra already stored. An offline stop of a session does the same. See
[The event log](event-log.md#crash-recovery).

To get the spend of the workspace over time, use `GET /api/workspaces/{ws}/cost`. You can add
`?since=<RFC 3339 time>`.

## The session budget

### When the budget is checked

The planner enforces the budget, not the executors. Each time before the planner emits a spawn or the model call
of a workflow prompt node (`Step::RunNode` with `model: true`), it compares the spend of the session with its
limit. This occurs in `Planner::push` in
[`crates/ostra-engine/src/plan.rs`](../../crates/ostra-engine/src/plan.rs):

```rust
// Budget guard: once the session has spent its budget, no new execution starts until
// the user raises it. Running executions finish.
if matches!(step, Step::Spawn(_) | Step::RunNode { model: true, .. })
    && let Some(budget) = self.ctx.budget_usd
{
    let limit = budget + self.s.budget_raised;
    let spent = self.s.spent_usd();
    if spent >= limit {
        // open one "The session reached its budget" gate instead of the spawn
        ...
        return;
    }
}
```

Three details control this behavior:

- **Spent means finished.** `SessionState::spent_usd` adds the cost of the executions that finished. It also adds
  the spend of workflow prompt nodes (`node_cost`, from each `NodeRan` event, Rule WB3). Ostra counts a running
  execution when it ends. Thus, the budget is a maximum that Ostra checks between executions. Executions that
  run when the session gets to the limit run to the end. A spawn that already waited for a slot still starts. A
  session can finish a small amount above its budget. The maximum excess is the spend of these executions.
- **Judge calls are not counted against it.** The budget covers agent executions and prompt nodes. Judge calls
  are in the displayed cost of the session, but they do not open the gate. They are small, and the session cannot
  continue without them. A prompt node uses the same judge path, but it counts, because a workflow can hold any
  number of prompt nodes. A transform node calls no model, and the budget never holds it.
- **The setting is read on every planning pass.** Each time the runner plans, it puts `session_budget_usd`
  into the context of the planner (`plan_ctx`). Thus, a budget that you change on the Settings screen applies
  to running sessions at their next step. A value of 0 removes the limit.

The planner stays a pure function. The budget gets to it only through `PlanCtx`. The spend and all raises come
from the event log of the session.

### The budget gate

When the check fails, Ostra opens a gate in place of the spawn. The title of the gate is "The session reached
its budget". The gate shows the amount spent and the limit:

> This session has spent $26.40 of its $25.00 budget, so no new execution starts. Raise the budget to continue,
> or stop the session.

The gate has two answers:

- **Raise**, with an amount in dollars. The `budget_raised` of the session increases by that amount, plus the
  spend above the limit. Thus, the new maximum is your spend plus the amount that you entered. With no amount,
  the raise is the budget again (at least $1). Thus, the session can spend the same amount one more time before
  the next pause. In the example above, a raise of $10 lets the session continue until $36.40.
- **Stop**. The session ends as failed with "Stopped at the session budget after spending $26.40." Ostra does
  not undo the work that the session did.

Ostra records the raise in the event log of the session as the gate answer. Thus, the raise stays after
restarts and applies only to that session. Other sessions keep the budget of the workspace.

![The budget gate with the amount spent, the budget, and Raise the budget and Stop the session buttons](../images/console/gate-budget.png)

### YOLO never answers it

In YOLO mode, the engine answers gates and does not wait for you: approvals, open questions, and failed
executions. It does not answer the budget gate. Two places enforce this rule, so that one mistake cannot remove
it. The planner skips budget gates when it emits YOLO answers. Also, `yolo_plan` in
[`crates/ostra-engine/src/judge_input.rs`](../../crates/ostra-engine/src/judge_input.rs) returns no plan for a
budget gate:

```rust
// Spending more is the user's decision, so YOLO leaves a budget gate open.
GatePayload::BudgetReached { .. } => return None,
```

A YOLO session that gets to its budget waits for you, the same as all other sessions. If push notifications
are on, the open gate sends a notification.

The conformance fixtures `budget_pauses_spawns_until_raised`, `budget_stop_ends_the_session`, and
`no_budget_means_no_limit` in [`tests/conformance/main.rs`](../../tests/conformance/main.rs) test this
behavior. The first fixture has YOLO on.

## Fan-out caps

A cap on a fan-out stage limits the number of executions that one decision can start. The cap applies before
the budget and the slot limiter. The init flow has two caps:

| Cap | Value | What it limits |
| --- | --- | --- |
| `init::MAX_SCOUTS` | 6 | Scouts that examine slices of the repository in parallel. Ultracode allowed 12. Ostra takes the first six slices from the detect step. |
| `init::MAX_DEFAULT_GENERATE` | 8 | Skills that the proposal marks to generate by default. The default for other recommended skills is drop, with the note "Dropped by default to limit cost; choose generate to include it." You can still select them at the approval gate. |

Each generated skill runs on the `advanced` tier. Thus, the second cap limits the most expensive part of init.
Under YOLO, Ostra uses the defaults of the proposal with no change. Thus, the cap also applies under YOLO.

Messages between subagents can also start or wake runs, so they have their own caps (Rule SM5):

- One run can start at most three helpers (`coord::MAX_HELPERS_PER_RUN`).
- The agents of a session can send at most 48 messages (`coord::MAX_SESSION_MESSAGES`). The result of a helper,
  which Ostra sends, does not count.
- A helper cannot start helpers. Thus, the fan-out has only one level.

Helpers, and the runs that continue an ended subagent for its messages, go through the slot limiter and the
budget guard, the same as all other spawns. A pair loop that continues a conversation is not a new fan-out. But a
conversation that gets to six runs (`coord::MAX_CONVERSATION_RUNS`) starts again with no history, because each
turn of a long conversation sends its full history again. For the same reason, a subagent whose conversation
already has six runs takes no message.

Workflows add their own limits (Rule WF7):

- A workflow holds at most 24 custom stages (`workflow::MAX_CUSTOM_STAGES`).
- Each stage runs at most `max_rounds` times. A workflow file can set this value from 1 to 10
  (`workflow::MAX_STAGE_ROUNDS`). The default is 3.

After the last round, a stage that fails asks you and does not run again. YOLO does not answer that gate, because
only you can decide to spend more on the stage. A plugin stage counts a decision to run past `max_rounds` as a
failure. A `project` stage runs one time for each project in scope, and a `phase` stage one time for each passed
phase. Each stage run goes through the slot limiter and the budget guard, the same as all other spawns. Thus,
stages with the same dependencies that run at the same time never exceed `max_parallel_executions`.

A programmatic agent is a plugin agent that runs in code. It has no turn count. Thus, its tool calls and model calls
together have a cap of 2,000 for each run (`MAX_PROGRAM_CALLS` in `crates/ostra-exec-native/src/program.rs`). Its
model calls run on the route of the agent and count toward the usage of the run and the spend of the session.

The engine has other loop limits that limit spend:

- A cap on review passes in each implement loop.
- One automatic retry after an error.
- Three sufficiency rounds for research.
- A guard that refuses build commands after five failed builds in a row.

[The pipeline](pipeline.md#fan-out-caps-and-limits) lists each limit with its value. Each agent also has a
`timeout_seconds` in its `agent.toml`. This value ends an execution that runs too long.

Each new fan-out in Ostra must have its own cap and must go through the slot limiter. This is a contributor
rule, not a setting.

## Stopping a session that spends too much

Select the action by two factors: how fast the spend must stop, and if you want to continue later.

| Action | How | What happens |
| --- | --- | --- |
| Pause | Pause on the board of the session, or `POST /api/sessions/{id}/pause` | Ostra interrupts running executions and starts nothing new, also no YOLO answer. Continue starts each paused execution again from the point where it stopped, as the same execution. Thus, its cost continues to add up on one row. |
| Stop | Stop on the board of the session, or `POST /api/sessions/{id}/stop` | Ostra cancels each running execution and denies waiting permission asks, and the session ends. A spawn or command that Ostra prepared when the stop arrived does not start. The reason is that the runner refuses to record a start after the session ended. The work that the session did stays. |
| Cancel one execution | Cancel on the execution, or `POST /api/executions/{id}/cancel` | That execution ends, and its failure gate opens as "You stopped ...". At the gate, you select retry or abandon. Nothing tries it again without you. YOLO keeps the gate open, and the init step of a created project skips the advisor (Rule P4). |
| Skip one task | Skip on a running research, test analysis, docs, or architecture execution, or `POST /api/executions/{id}/skip` | That execution ends `interrupted`, and its task ends without a result. No gate opens, and nothing runs it again (Rule U1). |
| Stop while the server is down | `ostra stop <session-id>` | Marks the running executions of the session as cancelled and ends the session. Thus, the next server start does not recover them and run them again. |
| Slow a workspace down | Decrease `max_parallel_executions` or `session_budget_usd` | Applies at the next spawn. Ostra cancels no running work. |

The row before the last row is important after a crash. When the server starts, it recovers each session that
was running. It runs the interrupted executions again. If a session spent more than you wanted when the server
stopped, run `ostra stop` on it before you start the server again. If a server runs, `ostra stop` refuses,
because the running server holds the session. In that case, use the board or the API.

## Where to look in the code

| What | Where |
| --- | --- |
| `Limits` and their defaults, save-time checks | [`crates/ostra-core/src/config.rs`](../../crates/ostra-core/src/config.rs) |
| Limits kept in the registry (Rule A2) | [`crates/ostra-workspace/src/trust.rs`](../../crates/ostra-workspace/src/trust.rs) |
| Slot limiter, budget in `plan_ctx`, live cost updates, stop and pause | [`crates/ostra-engine/src/runner.rs`](../../crates/ostra-engine/src/runner.rs) |
| Budget guard and the budget gate | `Planner::push` in [`crates/ostra-engine/src/plan.rs`](../../crates/ostra-engine/src/plan.rs) |
| `spent_usd`, `budget_raised`, and how a raise folds | [`crates/ostra-engine/src/state.rs`](../../crates/ostra-engine/src/state.rs) |
| YOLO handling of each gate | `yolo_plan` in [`crates/ostra-engine/src/judge_input.rs`](../../crates/ostra-engine/src/judge_input.rs) |
| Init caps | [`crates/ostra-engine/src/init.rs`](../../crates/ostra-engine/src/init.rs) |
| Prices and the cost formula | [`crates/ostra-core/src/pricing.rs`](../../crates/ostra-core/src/pricing.rs), [`crates/ostra-server/src/prices.rs`](../../crates/ostra-server/src/prices.rs) |
| Harness transcript usage | [`crates/ostra-exec-harness/src/transcript.rs`](../../crates/ostra-exec-harness/src/transcript.rs) |
