# Plugins

A plugin adds agents, workflow stage logic, whole workflows, and transform functions to Ostra in Rust code.
Markdown agents ([Agents](agents.md)) and workflow files ([Workflows](workflows.md)) cover a prompt and an order
of stages; a plugin covers what needs code: an agent whose work is a program, a stage that decides in code what
runs next, a result contract of its own that it turns into a verdict, a workflow it builds in code, or a
transform function it runs in code. This page covers the `ostra-sdk` crate, the standard plugin that holds
Ostra's own agents, the two ways a plugin runs, the stdio protocol, programmatic agents, plugin stages, plugin
contracts, plugin workflows and transform functions, and how Ostra keeps plugin programs from starting before
you approve them. The rules are PL1 to PL7 in [HANDOVER section 10.10](../../HANDOVER.md#1010-plugins).

## What a plugin offers

A plugin implements the `Plugin` trait ([`crates/ostra-core/src/plugin.rs`](../../crates/ostra-core/src/plugin.rs)),
which the [`ostra-sdk`](../../crates/ostra-sdk/src/lib.rs) crate re-exports:

```rust
#[async_trait::async_trait]
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> PluginManifest;
    async fn decide_stage(&self, stage: &str, view: StageView) -> Result<StageDecision, String>;
    async fn handle_result(&self, contract: &str, result: ResultView) -> Result<CustomSubmit, String>;
    async fn transform(&self, name: &str, inputs: Map<String, Value>, args: Map<String, Value>)
        -> Result<Value, String>;
    async fn run_agent(&self, agent: &str, task: AgentTask, calls: Arc<dyn AgentCalls>)
        -> Result<AgentOutcome, String>;
}
```

Only `manifest` is required; each other method defaults to an error that names what the plugin does not
serve. The manifest names the plugin and lists what it offers:

- **Agents** (`PluginAgent`). One with a `prompt` runs on a model like a markdown agent, with the same fields:
  description, default tier, capabilities, write scope, effort, timeout, `data_schema`, `helper`, `returns`
  (its result contract, `stage` by default), and `brief` (the repo brief sections it gets). One without a
  prompt is **programmatic**: `run_agent` does its work in code (Rule PL2).
- **Stages** (`PluginStage`). A workflow node `plugin = "<plugin>:<stage>"` is driven by `decide_stage`
  (Rule PL3).
- **Contracts** (`PluginContractDef`: a name, a description, and a JSON schema). An agent that returns
  `<plugin>:<name>` submits against that schema, and the plugin's `handle_result` turns each result into a
  verdict (Rule PL5, below).
- **Workflows** (`PluginWorkflow`: a name and a workflow in the shape of a workflow file). A session names one
  as `<plugin>:<name>` (Rule PL6, below).
- **Transform functions** (`TransformInfo`: a name, a description, typed inputs and arguments, and an output
  kind). A workflow node calls one as `transform = "<plugin>:<name>"`, and `transform` runs it (Rule PL7,
  below).

A `PluginAgent` is built with `PluginAgent::new(name, description)` and builder methods (`prompt`, `tier`,
`capabilities`, `write_scope`, `effort`, `timeout_seconds`, `data_schema`, `helper`, `returns`, `brief`). A
plugin that keeps its agents as files can read them with `ostra_sdk::definition`: `parse_markdown` reads a
markdown file with TOML frontmatter between `+++` lines, and `parse_toml` reads an `agent.toml` beside its
prompt. Ostra reads its own agents and a workspace's `.ostra/agents/` with the same two functions, so every
agent is the same `PluginAgent` whatever wrote it.

A plugin's agents join the workspace's agent catalog, with the plugin as their origin, through the same
conversion as every other agent (`catalog::from_plugin_agent`), so routes, the Settings list, `SendMessage`
helpers, workflow stages, and built-in stage bindings use them like any custom agent. An agent may request any
capability, including the document, ledger, and project-management grants, because none is reserved
([Agents](agents.md#result-contracts-and-grants)).

## The standard plugin

Ostra's own agents are a plugin too: the standard plugin `ostra` (`Standard` in
[`crates/ostra-agents/src/standard.rs`](../../crates/ostra-agents/src/standard.rs), Rule PL4). It is written with
`ostra-sdk` and serves as the reference for writing one. Its manifest lists the 14 built-in agents and no stages
or contracts, because the built-in stages are the planner's own rules. It builds each agent from its embedded
`assets/agents/<name>/agent.toml` and `prompt.md` with `parse_toml` and sets nothing else:

```rust
pub fn agent(agent: AgentName) -> Result<PluginAgent, AgentsError> {
    let dir = format!("agents/{}", agent.as_str());
    let toml_path = format!("{dir}/agent.toml");
    let prompt = asset_text(&format!("{dir}/prompt.md"))?;
    ostra_sdk::definition::parse_toml(agent.as_str(), &asset_text(&toml_path)?, &prompt)
        .map_err(|message| AgentsError::Parse { path: toml_path, message })
}
```

So a built-in agent's contract, write scope, brief, and grants are fields any agent can declare. The engine
reads results by contract, and `Standard::default_for(contract)` names the standard agent that returns a
contract, which a built-in stage runs unless its workflow binds another agent (Rule WF8,
[Workflows](workflows.md#which-agent-fills-a-built-in-stage)). A workspace `[[plugins]]` entry may not be
named `ostra`, so a plugin program cannot take the standard plugin's name.

## Two ways to run

### Built into the binary

A binary built on `ostra-server` passes its plugins to the command line entry point:

```rust
fn main() -> anyhow::Result<()> {
    ostra_server::cli::main_with(ostra_sdk::Registry::new().with(Arc::new(MyPlugin)))
}
```

The `ostra` binary itself is this `main` with an empty registry. Built-in plugins serve every workspace, need no
approval, because you compiled them in, and take precedence over a workspace plugin program of the same name.

### As a program

A workspace lists a plugin program in `workspace.toml`:

```toml
[[plugins]]
name = "release-gate"                   # must equal the name in the program's manifest
command = ["/opt/plugins/release_gate"] # program, then arguments
env = { RELEASE_CHANNEL = "stable" }    # passed as written
enabled = true
timeout_secs = 120                      # for `initialize`, each stage decision, result/handle, and transform/run, 1 to 3600
```

The program is any executable that calls `ostra_sdk::stdio::serve(Arc::new(MyPlugin)).await` from its `main`. It
writes logs to stderr, because stdout carries the protocol.

`ostra plugin add` writes the same entry from the command line, and works whether or not the server runs:

```bash
ostra plugin add release-gate --env RELEASE_CHANNEL=stable --timeout 120 -- /opt/plugins/release_gate
```

It finds the registered workspace that holds the current folder (or the folder `--workspace` names), refuses a
name the file already has, and checks the entries with the same `[[plugins]]` validation a save in Settings runs,
so a bad name, an empty command, or a timeout outside 1 to 3600 fails before anything is written. `--disabled`
writes `enabled = false`. The program path is kept as written, and a relative one resolves against the workspace
folder, because that is where the program starts. The command saves through the same path as Settings
(`trust::save_workspace`): a folder file that was approved stays approved, because the registration is your own
edit, and one that waits for approval keeps waiting, and the command says so. A running server starts the
program the next time it reads the workspace's agents.

`PluginHost` in [`crates/ostra-server/src/plugins.rs`](../../crates/ostra-server/src/plugins.rs) runs the programs:

- It starts a wanted program in the workspace folder, reads its manifest, and keeps it running. A program whose
  manifest names itself differently from its entry is refused.
- It restarts a program when its entry changes. The next read of the workspace's agents stops a program whose
  entry was removed or disabled, or whose file waits for approval again, before it lists anything.
- It starts programs before a session resolves its workflow, before a stage decision, before a programmatic
  run, and in the background whenever the workspace's agents are read, so their agents and stages appear in
  Settings soon after you add them.
- A program that did not start, or stopped, is a settings issue under its `plugins[<i>]` entry, with its error
  or the end of its stderr.

Rule PL1: a program is wanted only while it is enabled and the workspace's folder file is approved (Rule A1).
`[[plugins]]` is part of the hash you approve, together with the custom agent and workflow files, and the
effective settings Ostra starts programs from disable every entry of a file that waits for approval. So a
workspace folder that arrives with a plugin entry starts nothing until you approve it, and an edit outside
Ostra waits for approval again.

### What each plugin is doing

`GET /api/workspaces/:ws/plugins` lists the built-in plugins and every `[[plugins]]` entry of the workspace
file, each as a `PluginInfo` (`PluginHost::infos`, Rule AG3) with its state:

| State | When |
| --- | --- |
| `running` | A built-in plugin, or a program that started and is alive |
| `starting` | An entry that is wanted but has not started yet |
| `disabled` | `enabled = false` |
| `waiting_approval` | The workspace file waits for approval (Rule PL1) |
| `failed` | It did not start, or it stopped; `error` holds the reason or the end of its stderr |

A running plugin carries its manifest: its agents, stages, and contracts. The entry's `env` values are replaced
with the saved-value marker before the list leaves the server, so the browser sees the names and never the
values. The Workflow builder's palette lists the stages of running plugins from the same list.

## The stdio protocol

The program and Ostra speak newline-delimited JSON-RPC 2.0, with requests in both directions
([`crates/ostra-sdk/src/stdio.rs`](../../crates/ostra-sdk/src/stdio.rs)):

| Direction | Method | Answer |
| --- | --- | --- |
| Ostra to plugin | `initialize {protocol}` | The `PluginManifest` |
| Ostra to plugin | `stage/decide {stage, view, call, checkpoints}` | A `StageDecision`, within `timeout_secs` |
| Ostra to plugin | `agent/run {agent, task, call, checkpoints}` | An `AgentOutcome`, bounded by the execution's timeout |
| Ostra to plugin | `agent/cancel {execution}` (notification) | Sent when a run is stopped before it returns |
| Ostra to plugin | `result/handle {contract, result, call, checkpoints}` | A `CustomSubmit`, the outcome of one result of the plugin's contract, within `timeout_secs` |
| Ostra to plugin | `transform/run {name, inputs, args}` | The output of one of the plugin's transform functions, within `timeout_secs` |
| Plugin to Ostra | `host/tool {execution, name, input}` | A `ToolReply` |
| Plugin to Ostra | `host/complete {execution, request}` | `{text}` |
| Plugin to Ostra | `host/status {execution, text}` (notification) | A line in the run's Activity view |
| Plugin to Ostra | `host/checkpoint {call, key, value}` | `{}` once the save is in the session's log; a `null` value removes the key |

Each request runs on its own task, so a long `agent/run` never blocks a stage decision. A line longer than
16 MiB is read and then skipped, and a line that is not JSON is skipped too. When the connection closes, every request
waiting on it fails.

`call` is a token for one request, and `checkpoints` is what the plugin had saved in the session when the request
was sent ([Checkpoints and recovery](#checkpoints-and-recovery)). A `host/checkpoint` names the request it belongs
to by that token, and a token stops working when its request ends.

## Programmatic agents

A programmatic agent runs natively whatever its executor route says, because the plugin is its executor; its
model route serves the model calls it makes. The runner hands its run to `NativeExecutor::run_program`
([`crates/ostra-exec-native/src/program.rs`](../../crates/ostra-exec-native/src/program.rs)), which builds the same
tool environment, policy, and sandbox a model run gets, then calls `run_agent` with:

- an `AgentTask`: the execution id, the agent, its first message (the spawn block, the repo brief, the custom
  instructions, and a resume note when it resumes), the repo root, session dir, and workspace root, the model,
  the tool names it may call, and its submit schema;
- `AgentCalls`, through which every call goes:
  - `tool(name, input)` runs one tool through the same path a model's tool call takes: the policy, the
    permission ask, the sandbox, and the Activity entry. Only the tools its capabilities give it (and its MCP
    tools) are available, and it cannot call a submit tool; it returns its result from `run_agent` instead.
  - `complete(request)` makes one model call on the agent's route, with the agent's effort, and adds its usage
    to the run.
  - `status(text)` writes a line to the Activity view.

`AgentContext` in the SDK wraps these with helpers: `read`, `write`, `bash`, `grep`, `list_agents`,
`send_message`, `start_helper`, `wait_for_message`, `complete`, and `status`.

Messages reach a programmatic agent after each tool call: the messages queued for it follow that call's output
(Rule SM2). A `SendMessage` with `wait`, or `WaitForMessage`, returns once a message arrives, with the message in
its output; the run stays alive and gives its execution slot back while it waits.

A run may make at most 2,000 tool and model calls. When `run_agent` returns, Ostra checks the result as it checks
a model's submit: the run must have replied to any sender that waits for it (Rule SM6), the result must match the
agent's submit schema, and a run that was given a report file must have written it, unless its contract is `review`, whose ledger exists
only when the review found something. A plugin's error, or a
result that fails a check, ends the run as an error. The SDK's `pass` and `fail` helpers build a custom agent's
result.

## Plugin stages

A plugin stage is decided one step at a time, the way a judge answers ([`crates/ostra-engine/src/plugin_stage.rs`](../../crates/ostra-engine/src/plugin_stage.rs)):

1. The planner emits `Step::DecideStage` for the stage instance.
2. The runner builds a `StageView` from the state (the request, the projects in scope, the node's instructions,
   the node's `inputs` resolved from earlier nodes ([Workflows](workflows.md#data-between-nodes)), every earlier
   decision, every run the stage started with its status and submit, your answers at the stage's
   gates, the earlier custom stages, the spec, and the master plan) and calls `decide_stage`.
3. The runner records the answer as a `StageDecided` event. A plugin that is not running or cannot decide is
   recorded as a `fail` decision. A program that stops during the call is first started again and asked again
   ([Checkpoints and recovery](#checkpoints-and-recovery)).
4. The fold applies it (`on_stage_decided` in [`workflow.rs`](../../crates/ostra-engine/src/workflow.rs)):

| Decision | What follows |
| --- | --- |
| `run {agent, instructions}` | The agent runs as this stage, with the node's instructions and the plugin's. A run beyond `max_rounds` counts as a failure instead. |
| `pass {summary}` | The stage instance is done. |
| `fail {summary}` | The stage's `on_fail`: `continue` goes on, `fail` stops the session, and `gate` or `retry` opens a `stage_review` gate. |
| `ask {question, options}` | A `stage_review` gate with the question. |

A plugin stage with `when` conditions that do not hold is skipped before the plugin is asked anything
([Workflows](workflows.md#conditions-and-skipping)).

The planner asks again each time a run the stage started ends, and after each answer at its gate. The verdict of
a run the plugin started informs the plugin and decides nothing on its own. Recording each decision as an event
keeps the fold a function of the log: a restart replays the decisions instead of asking the plugin again.

## Plugin contracts

A plugin contract lets a plugin define what its agents return and decide what a result means (Rule PL5). The
runner asks every plugin of the workspace for its `contracts` (`PluginHost::contracts`), and the catalog
attaches each contract's schema to the agents that return it. An agent whose `returns` names a contract no
plugin of the workspace defines is a settings issue and is left out of the catalog.

An agent that returns `<plugin>:<name>` submits against that schema. When its run ends with a submit, nothing
reads it until its plugin has handled it:

1. The fold lists the run in `results_due`, and the planner emits `Step::HandleResult` for it.
2. The runner builds a `ResultView` (the session, the execution, the agent, the contract's name inside the
   plugin, the workflow node and scope the run served, and the submit) and calls `handle_result`, over
   `result/handle` for a program.
3. The runner appends `ResultHandled` with the returned `CustomSubmit` (verdict, summary, findings, question,
   options). A plugin that is not running or returns an error yields a `fail` verdict that names the error.
4. The fold records the outcome on the run (`ExecRecord.handled`) and applies it: a custom stage takes it as the
   run's verdict ([Workflows](workflows.md#how-a-stage-ends)), a plugin stage sees it with the run in its
   `StageView` (`StageRunView.handled`), and the agent that started a helper gets it as the helper's result
   message.

Because the outcome is an event, a restart replays it instead of asking the plugin again.

## Workflows and transform functions built in code

A plugin can ship whole workflows and its own transform functions (Rules PL6 and PL7). The SDK module
[`ostra_sdk::workflow`](../../crates/ostra-sdk/src/workflow.rs) has builders for both: `Workflow` and `Node`
produce a `PluginWorkflow`, and `TransformFn` produces the `TransformInfo` that declares a function. From its doc
comment:

```rust
use ostra_sdk::workflow::{Node, TransformFn, Workflow};
use ostra_sdk::{CondOp, ValueKind};

let flow = Workflow::extending("release-check", "ostra:implement")
    .description("The implement pipeline with a release check after the build.")
    .node(Node::agent("audit", "security-auditor").after(["build"]).before(["closing"]))
    .node(
        Node::transform("high", "release:high-risk")
            .after(["audit"])
            .before(["closing"])
            .input("findings", "audit.findings"),
    )
    .node(
        Node::plugin_stage("gate", "release:gate")
            .after(["high"])
            .before(["closing"])
            .when("high.output", CondOp::NotEmpty, None),
    )
    .build();
let high_risk = TransformFn::new("high-risk", "Findings whose file is under src/auth.")
    .input("findings", ValueKind::Array, "Findings with a file.")
    .output(ValueKind::Array)
    .build();
```

`Workflow::new(name, base)` starts a workflow that lists every node; `Workflow::extending(name, parent)` starts
from `ostra:<base>` or another plugin workflow, and `remove`, `track`, and `bind` set the same keys a workflow file
has. `Node` has one constructor per node kind (`stage`, `agent`, `plugin_stage`, `transform`, `prompt`) and methods
for `after`, `before`, `input`, `arg`, `when`, `instructions`, `scope`, `on_fail`, and `bind`.

### Plugin workflows

Each workflow in a plugin's manifest is named `<plugin>:<name>`. When Ostra reads the workspace's workflows, it
adds the workflows of every plugin that runs for the workspace (`WorkflowSet::add_plugin`, into
`WorkflowSet.plugin_files`), both in the engine's `Services::workflows` and in the workspace runtime's
`workflow_set`. `WorkflowSet::resolve` reads a name that holds a `:` as a plugin workflow, except `ostra:<base>`,
which is Ostra's default (see [Workflows](workflows.md#default-workflows)). So a session names a plugin workflow
like any other, a workspace workflow can extend it, and it can extend `ostra:<base>` or another plugin workflow. It
runs only when a session names it; a category's default is never a plugin workflow.

A plugin workflow is checked like a workflow file. One that does not resolve or cannot run is a settings issue
under `plugins` (`Plugin workflow `<name>` cannot run: ...`). The console lists it with `WorkflowInfo.plugin` set,
and the builder opens it read only, because its source is code: `WorkflowDoc.plugin` names the plugin, the page
shows a banner, and the palette and Save button are hidden. A new workflow can start from it, and a save under
its name is refused by `check_workflow` with the instruction to save under a name of your own. A plugin program
starts only while the workspace file is approved (Rule PL1), so its workflows are there only then.

### Plugin transform functions

Each transform function in a plugin's manifest is named `<plugin>:<name>` in the workspace
(`WorkflowSet.plugin_transforms`, with `TransformInfo.plugin` set). A node calls one with
`transform = "<plugin>:<name>"`. It is checked at save time like Ostra's functions (`function_info_with` and
`check_transform`, which take the plugin functions besides the composites): its inputs are wired, its arguments
have their declared types, and its output kind takes part in the data-flow type check ([Workflows](workflows.md)).
A resolved workflow records what each plugin function it calls takes and gives in
`WorkflowDef.plugin_transforms`, so the session's log holds it.

When the planner emits `Step::RunNode` for such a node, the runner splits the name and calls
`Services::plugin_transform`, which the server routes to the plugin's `Plugin::transform` (over `transform/run`
for a program). The runner records the output or the error in `NodeRan`, so the fold never calls the plugin and a
restart replays the recorded output. An error follows the node's `on_fail` like any failed transform.

A composite transform function of the workspace (`.ostra/transforms/`) cannot call a plugin function:
`check_function` refuses the step, because a composite runs inside Ostra in one step. The builder's palette lists
plugin functions under "Plugin transforms", and the Workflows page's Transforms panel lists them with their plugin
and no editor.

## Checkpoints and recovery

A plugin program can stop at any time: it crashes, it is killed, or its machine runs out of memory. Ostra keeps
running when that happens, because the program is a child process and its end only closes the connection: every
request waiting on it fails with "the other side closed the connection". What a plugin loses is what it held in
memory. Checkpoints keep that part in the session instead (Rule PL8).

### What a checkpoint is

A checkpoint is one key and one JSON value, saved by a plugin in one session. Each plugin has its own keys, and
it chooses them. Every call Ostra makes for a session passes the plugin's `Checkpoints`
([`crates/ostra-core/src/plugin.rs`](../../crates/ostra-core/src/plugin.rs)):

```rust
async fn decide_stage(&self, stage: &str, view: StageView, checkpoints: Arc<dyn Checkpoints>) -> ...
async fn handle_result(&self, contract: &str, result: ResultView, checkpoints: Arc<dyn Checkpoints>) -> ...
async fn run_agent(&self, agent: &str, task: AgentTask, calls: Arc<dyn AgentCalls>, checkpoints: Arc<dyn Checkpoints>) -> ...
```

`get(key)` and `all()` read, and `save(key, value)` and `remove(key)` write. A save returns once the
`PluginCheckpoint` event is in the session's log, so a program that stops right after a save has not lost it.
The fold keeps the latest value per plugin and key (`SessionState.plugin_checkpoints`). Transform functions get
no checkpoints, because Ostra records their output and a node that runs again computes it again.

Over stdio, a request carries the checkpoints as they were when it was sent, and the program's `all()` reads
that copy plus its own saves. Two calls of one plugin that run at once do not see each other's later saves.

The engine side is `SessionCheckpoints` in [`runner.rs`](../../crates/ostra-engine/src/runner.rs): it reads the
session's state, checks the write, and appends the event. A checkpoint decides nothing: no planner rule reads
one, so a save never starts, stops, or retries anything. The fixture `pl8_*` in
[`tests/conformance/main.rs`](../../tests/conformance/main.rs) pins that.

### Limits

| Limit | Value | Why |
| --- | --- | --- |
| Keys per plugin and session | 256 (`MAX_CHECKPOINT_KEYS`) | A changed or removed key is always allowed |
| Bytes per value, as JSON | 64 KiB (`MAX_CHECKPOINT_BYTES`) | Write large data to a file in the session dir and save its path |
| Saves per plugin and session | 5,000 (`MAX_CHECKPOINT_SAVES`) | Every save is an event in the log |
| Key | 1 to 200 characters, no control characters | |

A save of the value a key already holds, or a removal of a key that does not exist, is no save and counts
for nothing. A save after the session ended is refused.

### What happens when a program stops

| The program stops during | What Ostra does |
| --- | --- |
| A stage decision, a result handler, or a transform | `ServerServices::call_plugin` sees the plugin is no longer alive, starts the program again, and asks the same question again, at most twice (`PLUGIN_RESTARTS`). Only then is the error recorded, as a `fail` decision, a `fail` outcome, or a failed node. |
| A programmatic run | `program::execute` starts the program again through `PluginRestart` and calls `run_agent` again under the same execution, with `AgentTask.resumed` set, at most twice per run. The run keeps its Activity, its usage, its call count, and its timeout, which covers every attempt. |
| Nothing (between calls) | The next call starts it again, because `PluginHost::prepare` drops a program that is no longer alive before each call. |

A run that is stopped by the user or the session is not started again: Ostra checks the run's cancel token first.

An Ostra restart is different: recovery ends every running execution as interrupted, and the stage that ran it
starts a new execution. The checkpoints are in the log, so the new run reads them too; key them by the work they
describe, such as the stage node, rather than by the execution id, when they must outlive an Ostra restart.
A paused session that continues an interrupted run resumes it under its own id, with `resumed` set.

### Writing a plugin that resumes

Save after each step that has an effect outside the program, and before the next one, then read the checkpoint
first when a call starts:

```rust
let asked = format!("asked:{}", task.execution);
if checkpoints.get(&asked).is_none() {
    checkpoints.save(&asked, json!(implementer)).await?;
    ctx.send_message(&implementer, "Fix the changelog, then reply to me.", true).await;
}
```

A step between its effect and its save runs again after a restart, so make such a step safe to repeat. A tool call
that was running when the program stopped finishes on Ostra's side, and its reply goes nowhere.

`MemoryCheckpoints` in the SDK keeps checkpoints in memory with the same limits, for a plugin's own tests, and
`NoCheckpoints` is what a call outside a session receives.

## A worked example

[`crates/ostra-sdk/examples/release_gate.rs`](../../crates/ostra-sdk/examples/release_gate.rs) is a plugin program
with one programmatic agent, `changelog-check`, and one stage, `release`. Its stage logic decides:

- no run yet: run `changelog-check`;
- the last run passed: pass;
- one run failed: run `changelog-check` again, telling it to message the implementer first;
- two runs failed: ask whether to release anyway.

Its agent reads `CHANGELOG.md` and `git diff --stat HEAD` through Ostra's tools, so the policy and sandbox
apply. On the second round it finds the implementer with `ListAgents` and messages it with `wait`, so the
implementer continues its own conversation, with its tools, to fix the changelog, and the check resumes once the
implementer replies. Then it asks the model on its route whether the changelog describes the change, and returns
`pass` or `fail`. Before it messages the implementer it saves a checkpoint, so a run that resumes after its
program stopped does not message twice. To use it, build it with
`cargo build -p ostra-sdk --example release_gate`, add it under `[[plugins]]` (or run
`ostra plugin add release-gate -- <path to the example binary>`), add a stage `plugin = "release-gate:release"` to
a workflow, and approve the workspace file.

The example also declares a transform function, `bump`, which returns the next semantic version from its
`current` and `part` arguments, and a workflow built in code, `release-gate:implement-and-release`, which extends
`ostra:implement` with the release stage and a `release-gate:bump` node before the closing stages. A session can
name that workflow as it is.

## Where to look in the code

| What | Where |
| --- | --- |
| `Plugin`, `PluginAgent` and its builder, `PluginContractDef`, `ResultView`, `AgentCalls`, `Checkpoints` and its limits, the protocol types, `[[plugins]]` validation | [`crates/ostra-core/src/plugin.rs`](../../crates/ostra-core/src/plugin.rs) |
| `AgentContext`, `Registry`, `MemoryCheckpoints`, `pass`, `fail` | [`crates/ostra-sdk/src/lib.rs`](../../crates/ostra-sdk/src/lib.rs) |
| Builders for plugin workflows and transform functions: `Workflow`, `Node`, `TransformFn` | [`crates/ostra-sdk/src/workflow.rs`](../../crates/ostra-sdk/src/workflow.rs) |
| `WorkflowSet::add_plugin`, plugin workflow resolution | [`crates/ostra-core/src/workflow.rs`](../../crates/ostra-core/src/workflow.rs) |
| `PluginTransforms`, `function_info_with` | [`crates/ostra-core/src/transform.rs`](../../crates/ostra-core/src/transform.rs) |
| Agent files: `parse_markdown`, `parse_toml` | [`crates/ostra-sdk/src/definition.rs`](../../crates/ostra-sdk/src/definition.rs) |
| The standard plugin, `Standard::default_for` | [`crates/ostra-agents/src/standard.rs`](../../crates/ostra-agents/src/standard.rs) |
| The stdio transport, `serve`, `StdioPlugin` | [`crates/ostra-sdk/src/stdio.rs`](../../crates/ostra-sdk/src/stdio.rs) |
| `PluginHost`, `ProgramExecutor`, the run's `Restart`, `contracts`, `infos` | [`crates/ostra-server/src/plugins.rs`](../../crates/ostra-server/src/plugins.rs) |
| Restarting a plugin during a stage decision, result handler, or transform: `call_plugin` | [`crates/ostra-server/src/services.rs`](../../crates/ostra-server/src/services.rs) |
| Saving checkpoints into the log: `SessionCheckpoints` | [`crates/ostra-engine/src/runner.rs`](../../crates/ostra-engine/src/runner.rs) |
| `main_with` | [`crates/ostra-server/src/cli.rs`](../../crates/ostra-server/src/cli.rs) |
| Programmatic runs | [`crates/ostra-exec-native/src/program.rs`](../../crates/ostra-exec-native/src/program.rs) |
| Result handling: `results_due`, `result_view`, `on_result_handled` | [`crates/ostra-engine/src/workflow.rs`](../../crates/ostra-engine/src/workflow.rs) |
| Plugin stage planning and the stage view | [`crates/ostra-engine/src/plugin_stage.rs`](../../crates/ostra-engine/src/plugin_stage.rs) |
| Approval of `[[plugins]]` and the definition files | [`crates/ostra-workspace/src/trust.rs`](../../crates/ostra-workspace/src/trust.rs) |
| Fixtures (`pl3_*`, `pl8_*`), the whole-stack tests, a program that stops twice and resumes | [`tests/conformance/main.rs`](../../tests/conformance/main.rs), [`crates/ostra-server/tests/plugins.rs`](../../crates/ostra-server/tests/plugins.rs), [`crates/ostra-server/tests/plugin_recovery.rs`](../../crates/ostra-server/tests/plugin_recovery.rs) |
