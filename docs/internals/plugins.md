# Plugins

A plugin adds agents, workflow stage logic, full workflows, and transform functions to Ostra in Rust code.
Markdown agents ([Agents](agents.md)) and workflow files ([Workflows](workflows.md)) give a prompt and an order
of stages. A plugin gives the parts that need code:

- An agent whose work is a program.
- A stage that decides in code what runs next.
- A result contract of its own, which the plugin changes into a verdict.
- A workflow that the plugin builds in code.
- A transform function that the plugin runs in code.

This page covers these topics:

- The `ostra-sdk` crate, and its typed result contracts.
- The standard plugin, which holds the agents of Ostra and runs the built-in stages.
- The two ways to run a plugin, and the stdio protocol.
- Programmatic agents, plugin stages, and plugin contracts.
- Plugin workflows and transform functions.
- How Ostra stops a plugin program from starting before you approve it.

The rules are PL1 to PL8 in [HANDOVER section 10.10](../../HANDOVER.md#1010-plugins).

## What a plugin offers

A plugin implements the `Plugin` trait ([`crates/ostra-core/src/plugin.rs`](../../crates/ostra-core/src/plugin.rs)).
The [`ostra-sdk`](../../crates/ostra-sdk/src/lib.rs) crate exports the trait again:

```rust
#[async_trait::async_trait]
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> PluginManifest;
    fn is_alive(&self) -> bool;
    async fn decide_stage(&self, stage: &str, view: StageView, checkpoints: Arc<dyn Checkpoints>)
        -> Result<StageDecision, String>;
    async fn handle_result(&self, contract: &str, result: ResultView, checkpoints: Arc<dyn Checkpoints>)
        -> Result<CustomSubmit, String>;
    async fn transform(&self, name: &str, inputs: Map<String, Value>, args: Map<String, Value>)
        -> Result<Value, String>;
    async fn run_agent(&self, agent: &str, task: AgentTask, calls: Arc<dyn AgentCalls>,
        checkpoints: Arc<dyn Checkpoints>) -> Result<AgentOutcome, String>;
}
```

Only `manifest` is required. `is_alive` returns `true` by default. Each other method returns an error by default,
and the error names the item that the plugin does not serve. The manifest names the plugin and lists what it
offers:

- **Agents** (`PluginAgent`). An agent with a `prompt` runs on a model, the same as a markdown agent. It has the
  same fields: description, default tier, capabilities, write scope, effort, timeout, `data_schema`, `helper`,
  `returns`, and `brief`. `returns` is the result contract of the agent, and its default is `stage`. `brief` lists
  the sections of the repo brief that the agent gets. An agent without a prompt is **programmatic**: `run_agent`
  does its work in code (Rule PL2).
- **Stages** (`PluginStage`). `decide_stage` controls a workflow node `plugin = "<plugin>:<stage>"` (Rule PL3).
- **Contracts** (`PluginContractDef`: a name, a description, and a JSON schema). An agent that returns
  `<plugin>:<name>` submits against that schema. Then the `handle_result` method of the plugin changes each
  result into a verdict (Rule PL5, below).
- **Workflows** (`PluginWorkflow`: a name and a workflow in the shape of a workflow file). A session names one as
  `<plugin>:<name>` (Rule PL6, below).
- **Transform functions** (`TransformInfo`: a name, a description, typed inputs and arguments, and an output
  kind). A workflow node calls one as `transform = "<plugin>:<name>"`, and `transform` runs it (Rule PL7, below).

To make a `PluginAgent`, call `PluginAgent::new(name, description)` and then the builder methods: `prompt`,
`tier`, `capabilities`, `write_scope`, `effort`, `timeout_seconds`, `data_schema`, `helper`, `returns`, and
`brief`. A plugin that keeps its agents in files can read them with `ostra_sdk::definition`:

- `parse_markdown` reads a markdown file with TOML frontmatter between `+++` lines.
- `parse_toml` reads an `agent.toml` file next to its prompt.

Ostra reads its own agents and the `.ostra/agents/` folder of a workspace with the same two functions. Thus, each
agent is the same `PluginAgent`, from all sources.

The agents of a plugin join the agent catalog of the workspace, with the plugin as their origin. They go through
the same conversion as all other agents (`catalog::from_plugin_agent`). Thus, these parts use a plugin agent the
same as a custom agent: routes, the Settings list, `SendMessage` helpers, workflow stages, and built-in stage
bindings. An agent can request each capability, which includes the document, ledger, and project-management
grants, because no capability is reserved ([Agents](agents.md#result-contracts-and-grants)).

## Typed result contracts

A result contract is a struct in `ostra-core`, for example `CodeReviewerSubmit` for `review`. The core keeps these
structs, because more than one part reads each: the engine, a built-in stage, a plugin, and the console. The
module `ostra_sdk::contracts` ([`crates/ostra-sdk/src/contracts.rs`](../../crates/ostra-sdk/src/contracts.rs))
ties each contract to its struct, so a plugin reads and writes results as Rust values, not as JSON:

```rust
pub trait ContractType: Send + Sync + 'static {
    type Submit: Serialize + DeserializeOwned + Send + Sync;
    fn contract() -> Contract;
    fn schema() -> Value { /* the schema of the core struct */ }
    fn parse(submit: &Value) -> Result<Self::Submit, String> { /* ... */ }
    fn to_submit(submit: &Self::Submit, schema: &Value) -> Result<Value, String> { /* ... */ }
}
```

The module has one type for each built-in contract: `Research`, `Spec`, `FactCheck`, `Plan`, `Implementation`,
`Review`, `PathAnalysis`, `Tests`, `Prompt`, `Advice`, `Answer`, `Setup`, and `Stage`. `documentation` has no
type, because its struct belongs to the docs stage of the standard plugin. A plugin implements `ContractType` for
each contract of its own, and `contract_def::<C>(description)` makes the `PluginContractDef` of its manifest.

`to_submit` runs the same check as the engine (`validate_submit_with`). Thus, a typed agent that returns a bad
result fails in the plugin with the message that a model gets.

The module gives these parts:

- `ContractAgent`: a programmatic agent whose `run` returns the struct of its contract. `TypedAgents` lists such
  agents: `definitions()` gives each agent with `returns` set from its type, for the manifest, and `run(name, ...)`
  runs one from `Plugin::run_agent`. `PluginAgentExt::returns_type::<C>()` sets `returns` on a `PluginAgent`.
- `SubmitExt::submit_as::<C>()` reads the submit of a `StageRunView` or a `ResultView` as the struct of `C`.
  `StageViewExt::submits_of::<C>()` and `last_submit_of::<C>()` read the runs of a plugin stage. `handled(run)`
  reads the outcome that the handler of the plugin gave a run.
- `ResultHandler`: a handler for the results of a contract of the plugin, which gets each submit as its struct.
  `handle_result(handler, result, checkpoints)` calls it from `Plugin::handle_result`.

A typed agent that returns a built-in contract fills the same contract as the agent of Ostra. Thus, a workflow can
bind it to a built-in stage (Rule WF8). The example
[`crates/ostra-sdk/examples/typed_review.rs`](../../crates/ostra-sdk/examples/typed_review.rs) has a programmatic
reviewer that returns `review`, a contract of its own with a typed handler, and a stage that reads its runs as
structs.

## The standard plugin

The agents and default workflows of Ostra are also a plugin: the standard plugin `ostra` (Rule PL4). Two crates
hold it:

- `ostra-standard` holds its definitions: `Standard` in
  [`crates/ostra-standard/src/lib.rs`](../../crates/ostra-standard/src/lib.rs), the agent files, and the
  default workflows.
- `ostra-default-plugin` exports the definitions again and adds the pipeline: the code of the built-in stages
  (see [The pipeline](#the-pipeline), below).

`Standard` uses `ostra-sdk`, and it is the reference for a new plugin. Its manifest lists the 13 built-in agents
and the 9 default workflows. It lists no stages or contracts, because the pipeline runs the built-in stages. It
makes each agent from its embedded `assets/agents/<name>/agent.toml` and `prompt.md` files with `parse_toml`,
and it sets nothing more:

```rust
pub fn agent(agent: AgentName) -> Result<PluginAgent, String> {
    let toml_path = format!("{agent}/agent.toml");
    let prompt = agent_file(&format!("{agent}/prompt.md"))?;
    ostra_sdk::definition::parse_toml(agent.as_str(), &agent_file(&toml_path)?, &prompt)
        .map_err(|e| format!("assets/agents/{toml_path}: {e}"))
}
```

Thus, the contract, write scope, brief, and grants of a built-in agent are fields that each agent can declare.
The engine reads results by contract. `Standard::default_for(contract)` names the standard agent that returns a
contract. A built-in stage runs that agent, but a workflow can bind a different agent to the stage (Rule WF8,
[Workflows](workflows.md#which-agent-fills-a-built-in-stage)).

Each default workflow is its embedded `assets/workflows/<base>.toml` file. The manifest offers it as a workflow
with the name of the base. `WorkflowSet::add_plugin` adds the workflows of the standard plugin as the defaults of
the set, not as workflows of a different plugin. Thus, each one resolves as `ostra:<base>`. It also resolves as
a bare `<base>` when the workspace has no copy of it ([Workflows](workflows.md#default-workflows)). A set without
the standard plugin has no defaults. Thus, the server and the workspace add the standard plugin to each set that
they make (`add_workflows`). A workspace `[[plugins]]` entry cannot have the name `ostra`, so a plugin program
cannot take the name of the standard plugin.

### The pipeline

The built-in stages of Ostra (research, track, spec, stakes, plan, build, feedback, closing, and book) are not
plugin stages. They are the pipeline of the standard plugin: `OstraPipeline` in
[`crates/ostra-default-plugin/src/pipeline/`](../../crates/ostra-default-plugin/src/pipeline/), which implements
the `Pipeline` trait of the engine ([`crates/ostra-engine/src/pipeline.rs`](../../crates/ostra-engine/src/pipeline.rs)).
The engine gets the pipeline from `Services::pipeline`. The server gives `ostra_default_plugin::pipeline()`, and
each session state carries it.

A plugin stage decides one step at a time over stdio, and Ostra records each decision as an event. The pipeline
runs in the process instead. It folds its own state and plans its own steps, the same as the engine:

- The engine keeps the state of the pipeline in each session as an opaque box, `SessionState::ext`. The
  standard pipeline keeps `OstraState` in it
  ([`crates/ostra-default-plugin/src/data.rs`](../../crates/ostra-default-plugin/src/data.rs)): the research
  tasks, the spec and plan tracks, the phases and their loops, and each project's closing and docs track.
- The fold calls the pipeline for each event that the built-in stages read
  ([`fold/`](../../crates/ostra-default-plugin/src/fold/)). The fold stays a pure function of the log, because
  the pipeline is deterministic Rust code.
- The planner calls `Pipeline::builtin_stage` for each built-in node of the workflow
  ([`planner/`](../../crates/ostra-default-plugin/src/planner/), split by stage, with track and stakes in `shared.rs`). A step that only the
  pipeline plans is `Step::Pipeline`, with a key and a summary from the pipeline. The runner hands it back to
  `Pipeline::perform` ([`pipeline/effects.rs`](../../crates/ostra-default-plugin/src/pipeline/effects.rs)), for
  example the format command, an autofix, or the docs scan.
- A spawn's `SpawnInputs` holds the generic fields. The pipeline's own inputs travel in `SpawnInputs::extra`, as
  `OstraInputs` ([`inputs.rs`](../../crates/ostra-default-plugin/src/inputs.rs)), and the spawn factory
  (`AgentsFactory` in [`factory.rs`](../../crates/ostra-default-plugin/src/factory.rs)) reads them.
- The judges, their inputs, the YOLO answers, and the checks of gate answers are in the pipeline
  ([`judge.rs`](../../crates/ostra-default-plugin/src/judge.rs),
  [`judge_input/`](../../crates/ostra-default-plugin/src/judge_input/)).
- `Pipeline::check_submit` adds checks to a submit at submit time, after the shape check. The executors call it
  through `ExecutionHost::check_submit`. The standard pipeline checks a `documentation` submit there.
- The board, the run labels, and the artifacts of a session come from the pipeline
  ([`view/`](../../crates/ostra-default-plugin/src/view/)).

The engine performs the book write itself (`Step::WriteBook`, Rule B5), from the update that
`Pipeline::book_update` gives. The engine depends on no part of the standard plugin. The tests of a real engine
with the standard pipeline are in [`crates/ostra-default-plugin/tests/`](../../crates/ostra-default-plugin/tests/).

## Two ways to run

### Built into the binary

A binary that uses `ostra-server` gives its plugins to the command-line entry point:

```rust
fn main() -> anyhow::Result<()> {
    ostra_server::cli::main_with(ostra_sdk::Registry::new().with(Arc::new(MyPlugin)))
}
```

The `ostra` binary is this `main` with an empty registry. A built-in plugin serves all workspaces. It needs no
approval, because you compiled it into the binary. If a workspace plugin program has the same name, the built-in
plugin wins.

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

The program is an executable that calls `ostra_sdk::stdio::serve(Arc::new(MyPlugin)).await` from its `main`. It
writes logs to stderr, because stdout carries the protocol.

`ostra plugin add` writes the same entry from the command line. It works when the server runs and when it does
not:

```bash
ostra plugin add release-gate --env RELEASE_CHANNEL=stable --timeout 120 -- /opt/plugins/release_gate
```

The command does these steps:

1. It finds the registered workspace that holds the current folder, or the folder that `--workspace` names.
2. It refuses a name that the file has already.
3. It checks the entries with the same `[[plugins]]` validation that a save in Settings runs. Thus, a bad name,
   an empty command, or a timeout outside 1 to 3600 fails before the command writes anything.
4. It saves through the same path as Settings (`trust::save_workspace`).

`--disabled` writes `enabled = false`. The command keeps the program path as written. A relative path resolves
against the workspace folder, because the program starts in that folder. If the folder file was approved, it
stays approved, because the registration is your own edit. If the file waits for approval, it continues to wait,
and the command tells you. A running server starts the program the next time that it reads the agents of the
workspace.

`PluginHost` in [`crates/ostra-server/src/plugins.rs`](../../crates/ostra-server/src/plugins.rs) runs the programs:

- It starts a wanted program in the workspace folder, reads its manifest, and keeps it running. It refuses a
  program whose manifest has a name different from its entry.
- It starts a program again when its entry changes. The next read of the agents of the workspace stops a program
  in these cases, before it lists anything: you removed or disabled its entry, or its file waits for approval
  again.
- It starts programs at these times: before a session resolves its workflow, before a stage decision, before a
  programmatic run, and in the background at each read of the agents of the workspace. Thus, their agents and
  stages appear in Settings soon after you add them.
- If a program did not start or stopped, Settings shows an issue under its `plugins[<i>]` entry. The issue shows
  the error or the end of the stderr of the program.

Rule PL1: a program is wanted only when it is enabled and the folder file of the workspace is approved (Rule
A1). `[[plugins]]` is part of the hash that you approve, together with the custom agent files and workflow
files. Ostra starts programs from the effective settings, and these settings disable each entry of a file that
waits for approval. Thus, a workspace folder that arrives with a plugin entry starts nothing until you approve
it. An edit outside Ostra also waits for approval again.

### What each plugin is doing

`GET /api/workspaces/:ws/plugins` lists the built-in plugins and each `[[plugins]]` entry of the workspace file.
Each item is a `PluginInfo` (`PluginHost::infos`, Rule AG3) with its state:

| State | When |
| --- | --- |
| `running` | A built-in plugin, or a program that started and is alive |
| `starting` | An entry that is wanted but did not start yet |
| `disabled` | `enabled = false` |
| `waiting_approval` | The workspace file waits for approval (Rule PL1) |
| `failed` | It did not start, or it stopped. `error` holds the reason or the end of its stderr |

A running plugin carries its manifest: its agents, stages, and contracts. Before the list leaves the server, the
server replaces the `env` values of the entry with the saved-value marker. Thus, the browser sees the names and
never the values. The palette of the Workflow builder lists the stages of running plugins from the same list.

## The stdio protocol

The program and Ostra use newline-delimited JSON-RPC 2.0, with requests in the two directions
([`crates/ostra-sdk/src/stdio.rs`](../../crates/ostra-sdk/src/stdio.rs)):

| Direction | Method | Answer |
| --- | --- | --- |
| Ostra to plugin | `initialize {protocol}` | The `PluginManifest` |
| Ostra to plugin | `stage/decide {stage, view, call, checkpoints}` | A `StageDecision`, in `timeout_secs` |
| Ostra to plugin | `agent/run {agent, task, call, checkpoints}` | An `AgentOutcome`, in the timeout of the execution |
| Ostra to plugin | `agent/cancel {execution}` (notification) | Ostra sends it when a run stops before it returns |
| Ostra to plugin | `result/handle {contract, result, call, checkpoints}` | A `CustomSubmit`: the outcome of one result of a contract of the plugin, in `timeout_secs` |
| Ostra to plugin | `transform/run {name, inputs, args}` | The output of one transform function of the plugin, in `timeout_secs` |
| Plugin to Ostra | `host/tool {execution, name, input}` | A `ToolReply` |
| Plugin to Ostra | `host/complete {execution, request}` | `{text}` |
| Plugin to Ostra | `host/status {execution, text}` (notification) | A line in the Activity view of the run |
| Plugin to Ostra | `host/checkpoint {call, key, value}` | `{}` after the save is in the log of the session. A `null` value removes the key |

Each request runs on its own task, so a long `agent/run` never blocks a stage decision. The transport reads a
line longer than 16 MiB and then skips it. It also skips a line that is not JSON. When the connection closes,
each request that waits on it fails.

`call` is a token for one request. `checkpoints` holds the data that the plugin saved in the session when Ostra
sent the request ([Checkpoints and recovery](#checkpoints-and-recovery)). A `host/checkpoint` uses that token to
name its request. The token stops working when its request ends.

## Programmatic agents

A programmatic agent always runs natively, because the plugin is its executor. Its executor route has no effect.
Its model route serves the model calls that it makes. The runner gives its run to
`NativeExecutor::run_program` ([`crates/ostra-exec-native/src/program.rs`](../../crates/ostra-exec-native/src/program.rs)).
This function makes the same tool environment, policy, and sandbox that a model run gets. Then it calls
`run_agent` with two values:

- An `AgentTask`, which holds these items:
  - The execution ID and the agent.
  - The first message: the spawn block, the repo brief, the custom instructions, and a resume note when the run
    resumes.
  - The repo root, the session dir, and the workspace root.
  - The model, the names of the tools that the agent can call, and its submit schema.
- `AgentCalls`, which carries each call of the agent:
  - `tool(name, input)` runs one tool through the same path as a tool call from a model: the policy, the
    permission ask, the sandbox, and the Activity entry. Only the tools from its capabilities and its MCP tools
    are available. It cannot call a submit tool. It returns its result from `run_agent`.
  - `complete(request)` makes one model call on the route of the agent, with the effort of the agent. It adds the
    usage to the run.
  - `status(text)` writes a line to the Activity view.

`AgentContext` in the SDK wraps these calls with helpers: `read`, `write`, `bash`, `grep`, `list_agents`,
`send_message`, `start_helper`, `wait_for_message`, `complete`, and `status`.

A programmatic agent gets its messages after each tool call. The messages in its queue follow the output of that
call (Rule SM2). A `SendMessage` with `wait`, or a `WaitForMessage`, returns when a message arrives, with the
message in its output. The run stays alive, and it gives back its execution slot when it waits.

A run can make at most 2,000 tool calls and model calls (`MAX_PROGRAM_CALLS`). When `run_agent` returns, Ostra
checks the result the same as the submit of a model:

- The run must reply to each sender that waits for it (Rule SM6).
- The result must match the submit schema of the agent.
- If Ostra gave the run a report file, the run must write it. The exception is the `review` contract, because its
  ledger exists only when the review found a problem.

An error from the plugin, or a result that fails a check, ends the run as an error. The `pass` and `fail`
helpers in the SDK make the result of a custom agent.

## Plugin stages

The plugin decides a plugin stage one step at a time, the same as a judge answers
([`crates/ostra-engine/src/plugin_stage.rs`](../../crates/ostra-engine/src/plugin_stage.rs)):

1. The planner emits `Step::DecideStage` for the stage instance.
2. The runner makes a `StageView` from the state and calls `decide_stage`. The view holds these items:
   - The request, the projects in scope, and the instructions of the node.
   - The `inputs` of the node, resolved from earlier nodes ([Workflows](workflows.md#data-between-nodes)).
   - Each earlier decision, and each run that the stage started, with its status and submit.
   - Your answers at the gates of the stage, and the earlier custom stages.
   - The spec and the master plan.
3. The runner records the answer as a `StageDecided` event. If a plugin does not run or cannot decide, the runner
   records a `fail` decision. If a program stops during the call, Ostra first starts it again and asks again
   ([Checkpoints and recovery](#checkpoints-and-recovery)).
4. The fold applies the decision (`on_stage_decided` in [`workflow.rs`](../../crates/ostra-engine/src/workflow.rs)):

| Decision | What follows |
| --- | --- |
| `run {agent, instructions}` | The agent runs as this stage, with the instructions of the node and of the plugin. A run past `max_rounds` counts as a failure. |
| `pass {summary}` | The stage instance is done. |
| `fail {summary}` | The `on_fail` of the stage applies: `continue` goes to the next stage, `fail` stops the session, and `gate` or `retry` opens a `stage_review` gate. |
| `ask {question, options}` | A `stage_review` gate with the question. |

If the `when` conditions of a plugin stage are false, Ostra skips the stage before it asks the plugin anything
([Workflows](workflows.md#conditions-and-skipping)).

The planner asks again each time a run of the stage ends, and after each answer at its gate. The verdict of a run
that the plugin started gives information to the plugin, but it decides nothing. Each decision is an event, so
the fold stays a function of the log. Thus, a restart replays the decisions and does not ask the plugin again.

## Plugin contracts

A plugin contract lets a plugin define the data that its agents return and decide what a result means (Rule PL5).
The runner asks each plugin of the workspace for its `contracts` (`PluginHost::contracts`). Then the catalog
attaches the schema of each contract to the agents that return it. If the `returns` of an agent names a contract
that no plugin of the workspace defines, Settings shows an issue, and the catalog leaves out the agent.

An agent that returns `<plugin>:<name>` submits against that schema. When its run ends with a submit, nothing
reads the submit until its plugin handles it:

1. The fold lists the run in `results_due`, and the planner emits `Step::HandleResult` for it.
2. The runner makes a `ResultView` and calls `handle_result`. For a program, the call goes over `result/handle`.
   The view holds the session, the execution, the agent, the name of the contract in the plugin, the workflow
   node and scope of the run, and the submit.
3. The runner appends `ResultHandled` with the returned `CustomSubmit`: verdict, summary, findings, question, and
   options. If a plugin does not run or returns an error, the result is a `fail` verdict that names the error.
4. The fold records the outcome on the run (`ExecRecord.handled`) and applies it:
   - A custom stage uses it as the verdict of the run ([Workflows](workflows.md#how-a-stage-ends)).
   - A plugin stage sees it with the run in its `StageView` (`StageRunView.handled`).
   - The agent that started a helper gets it as the result message of the helper.

The outcome is an event, so a restart replays it and does not ask the plugin again.

## Workflows and transform functions built in code

A plugin can supply full workflows and its own transform functions (Rules PL6 and PL7). The SDK module
[`ostra_sdk::workflow`](../../crates/ostra-sdk/src/workflow.rs) has builders for the two:

- `Workflow` and `Node` make a `PluginWorkflow`.
- `TransformFn` makes the `TransformInfo` that declares a function.

This example comes from the doc comment of the module:

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

`Workflow::new(name, base)` starts a workflow that lists all its nodes. `Workflow::extending(name, parent)`
starts from `ostra:<base>` or from a different plugin workflow. `remove`, `track`, and `bind` set the same keys
as a workflow file. `Node` has one constructor for each node kind: `stage`, `agent`, `plugin_stage`, `transform`,
and `prompt`. It has methods for `after`, `before`, `input`, `arg`, `when`, `instructions`, `scope`, `on_fail`,
and `bind`.

### Plugin workflows

Each workflow in the manifest of a plugin has the name `<plugin>:<name>`. When Ostra reads the workflows of the
workspace, it adds the workflows of each plugin that runs for the workspace. `WorkflowSet::add_plugin` adds them
to `WorkflowSet.plugin_files`. This occurs in the `Services::workflows` of the engine and in the `workflow_set`
of the workspace runtime. `WorkflowSet::resolve` reads a name that holds a `:` as a plugin workflow. The exception
is `ostra:<base>`, which is a default workflow of Ostra ([Workflows](workflows.md#default-workflows)).

Thus, a session names a plugin workflow the same as all other workflows. A workspace workflow can extend a plugin
workflow. A plugin workflow can extend `ostra:<base>` or a different plugin workflow. A plugin workflow runs only
when a session names it. The default of a category is never a plugin workflow.

Ostra checks a plugin workflow the same as a workflow file. If a plugin workflow does not resolve or cannot run,
Settings shows an issue under `plugins` (`Plugin workflow `<name>` cannot run: ...`). The console lists the
workflow with `WorkflowInfo.plugin` set. The builder opens it read-only, because its source is code:

- `WorkflowDoc.plugin` names the plugin.
- The page shows a banner.
- The page hides the palette and the Save button.

A new workflow can start from a plugin workflow. `check_workflow` refuses a save under the name of the plugin
workflow, and it tells you to save under a name of your own. A plugin program starts only when the workspace
file is approved (Rule PL1), so its workflows exist only then.

### Plugin transform functions

Each transform function in the manifest of a plugin has the name `<plugin>:<name>` in the workspace
(`WorkflowSet.plugin_transforms`, with `TransformInfo.plugin` set). A node calls one with
`transform = "<plugin>:<name>"`. Ostra checks it at save time the same as its own functions. `function_info_with`
and `check_transform` take the plugin functions in addition to the composites. The check makes sure of these
items:

- Each input has a source.
- Each argument has its declared type.
- The output kind passes the data-flow type check ([Workflows](workflows.md)).

A resolved workflow records the inputs and the output of each plugin function that it calls in
`WorkflowDef.plugin_transforms`. Thus, the log of the session holds them.

When the planner emits `Step::RunNode` for such a node, the runner splits the name and calls
`Services::plugin_transform`. The server sends the call to the `Plugin::transform` of the plugin, over
`transform/run` for a program. The runner records the output or the error in `NodeRan`. Thus, the fold never
calls the plugin, and a restart replays the recorded output. An error follows the `on_fail` of the node, the same
as all other failed transforms.

A composite transform function of the workspace (`.ostra/transforms/`) cannot call a plugin function.
`check_function` refuses the step, because a composite runs in Ostra in one step. The palette of the builder lists
plugin functions under "Plugin transforms". The Transforms panel of the Workflows page lists them with their
plugin and with no editor.

## Checkpoints and recovery

A plugin program can stop at any time. For example, it crashes, a user or the OS kills it, or its machine has no
more memory. Ostra continues to run when this occurs, because the program is a child process. The end of the
program only closes the connection. Each request that waits on it fails with "the other side closed the
connection". The plugin loses the data that it held in memory. Checkpoints keep that data in the session
(Rule PL8).

### What a checkpoint is

A checkpoint is one key and one JSON value, which a plugin saves in one session. Each plugin has its own keys,
and it selects them. Each call that Ostra makes for a session gives the `Checkpoints` of the plugin
([`crates/ostra-core/src/plugin.rs`](../../crates/ostra-core/src/plugin.rs)):

```rust
async fn decide_stage(&self, stage: &str, view: StageView, checkpoints: Arc<dyn Checkpoints>) -> ...
async fn handle_result(&self, contract: &str, result: ResultView, checkpoints: Arc<dyn Checkpoints>) -> ...
async fn run_agent(&self, agent: &str, task: AgentTask, calls: Arc<dyn AgentCalls>, checkpoints: Arc<dyn Checkpoints>) -> ...
```

`get(key)` and `all()` read. `save(key, value)` and `remove(key)` write. A save returns after the
`PluginCheckpoint` event is in the log of the session. Thus, a program that stops directly after a save does not
lose the save. The fold keeps the last value for each plugin and key (`SessionState.plugin_checkpoints`).
Transform functions get no checkpoints, because Ostra records their output. A node that runs again calculates its
output again.

Over stdio, a request carries the checkpoints from the time that Ostra sent it. The `all()` of the program reads
that copy and its own saves. If two calls of one plugin run at the same time, they do not see the later saves of
each other.

The engine part is `SessionCheckpoints` in [`runner/`](../../crates/ostra-engine/src/runner/) (the struct in
`mod.rs`, its `Checkpoints` impl in `driver.rs`). It reads the
state of the session, checks the write, and appends the event. A checkpoint decides nothing, because no planner
rule reads one. Thus, a save never starts, stops, or retries anything. The `pl8_*` fixtures in
[`tests/conformance/main.rs`](../../tests/conformance/main.rs) test this.

### Limits

| Limit | Value | Why |
| --- | --- | --- |
| Keys for each plugin and session | 256 (`MAX_CHECKPOINT_KEYS`) | A change or a removal of a key is always allowed |
| Bytes for each value, as JSON | 64 KiB (`MAX_CHECKPOINT_BYTES`) | Write large data to a file in the session dir and save its path |
| Saves for each plugin and session | 5,000 (`MAX_CHECKPOINT_SAVES`) | Each save is an event in the log |
| Key | 1 to 200 characters, no control characters | |

A save of the value that a key holds already is not a save, and it does not count. A removal of a key that does
not exist also does not count. Ostra refuses a save after the session ended.

### What happens when a program stops

| The program stops during | What Ostra does |
| --- | --- |
| A stage decision, a result handler, or a transform | `ServerServices::call_plugin` sees that the plugin is not alive. It starts the program again and asks the same question again, at most 2 times (`PLUGIN_RESTARTS`). Only after that does it record the error, as a `fail` decision, a `fail` outcome, or a failed node. |
| A programmatic run | `program::execute` starts the program again through `PluginRestart`. It calls `run_agent` again under the same execution, with `AgentTask.resumed` set, at most 2 times for each run. The run keeps its Activity, its usage, its call count, and its timeout. The timeout covers all tries. |
| Nothing (between calls) | The next call starts it again, because `PluginHost::prepare` removes a program that is not alive before each call. |

Ostra does not start a run again when the user or the session stopped it. Ostra checks the cancel token of the
run first.

An Ostra restart is different. Recovery ends each running execution as interrupted, and the stage that ran it
starts a new execution. The checkpoints are in the log, so the new run also reads them. If a checkpoint must stay
after an Ostra restart, key it by the work that it describes, such as the stage node, not by the execution ID. A
paused session that continues an interrupted run resumes it under its own ID, with `resumed` set.

### Writing a plugin that resumes

Save after each step that has an effect outside the program, and before the next step. When a call starts, read
the checkpoint first:

```rust
let asked = format!("asked:{}", task.execution);
if checkpoints.get(&asked).is_none() {
    checkpoints.save(&asked, json!(implementer)).await?;
    ctx.send_message(&implementer, "Fix the changelog, then reply to me.", true).await;
}
```

Make each step safe to run two times, because a step between its effect and its save runs again after a
restart. A tool call that ran when the program stopped finishes in Ostra, and its reply goes nowhere.

`MemoryCheckpoints` in the SDK keeps checkpoints in memory with the same limits, for the tests of a plugin.
A call outside a session gets `NoCheckpoints`.

## A worked example

[`crates/ostra-sdk/examples/release_gate.rs`](../../crates/ostra-sdk/examples/release_gate.rs) is a plugin program
with one programmatic agent, `changelog-check`, and one stage, `release`. Its stage logic decides as follows:

- If no run exists, it runs `changelog-check`.
- If the last run passed, the stage passes.
- If one run failed, it runs `changelog-check` again, and tells it to send a message to the implementer first.
- If two runs failed, it asks whether to release.

Its agent reads `CHANGELOG.md` and `git diff --stat HEAD` through the tools of Ostra, so the policy and the
sandbox apply. In the second round, it finds the implementer with `ListAgents` and sends it a message with
`wait`. The implementer continues its own conversation, with its tools, and fixes the changelog. The check
resumes when the implementer replies. Then the check asks the model on its route whether the changelog describes
the change, and returns `pass` or `fail`. Before it sends the message to the implementer, it saves a checkpoint.
Thus, a run that resumes after its program stopped does not send the message two times.

To use the example, do these steps:

1. Compile it with `cargo build -p ostra-sdk --example release_gate`.
2. Add it under `[[plugins]]`, or run `ostra plugin add release-gate -- <path to the example binary>`.
3. Add a stage `plugin = "release-gate:release"` to a workflow.
4. Approve the workspace file.

The example also declares a transform function, `bump`. It returns the next semantic version from its `current`
and `part` arguments. The example also has a workflow built in code, `release-gate:implement-and-release`. This
workflow extends `ostra:implement` with the release stage and a `release-gate:bump` node before the closing
stages. A session can use that workflow without changes.

## A web development plugin

[`crates/ostra-sdk/examples/web_dev/`](../../crates/ostra-sdk/examples/web_dev/main.rs) is a larger plugin program,
`web-dev`. It adds end-to-end browser tests to the implement pipeline. It uses each part of the protocol: a model
agent, a programmatic agent, a stage, a contract of its own, a workflow built in code, and checkpoints.

| Part | What it does |
| --- | --- |
| `e2e-tester` | A model agent (`balanced` tier, project write scope, `test_files`). Its prompt is [`e2e-tester.md`](../../crates/ostra-sdk/examples/web_dev/e2e-tester.md). It decides whether the project has a web interface. It sets up Playwright with a `webServer` entry that starts the app on `127.0.0.1`. It writes one scenario for each flow that the request adds or changes, runs them, and gives each failure a cause: `app`, `test`, or `environment`. It changes only test files and test configuration. |
| `e2e-runner` | A programmatic agent. It sends the app failures to the implementer of the project, waits for the reply, and runs the same Playwright command again with `--reporter=json`. It makes no model call, so a run after a fix costs only the test run. |
| Contract `web-dev:e2e` | The two agents return it: `web_app`, `dir`, `command`, `scenarios` (each with `name`, `file`, `status`, `error`, `cause`), `blocker`, and `summary`. |
| Stage `web-dev:e2e` | The loop between the two agents, described below. |
| Workflow `web-dev:web-app` | `ostra:implement` with an `e2e` node after `build` and before `feedback`, with `scope = "project"`, `on_fail = "continue"`, and `max_rounds = 10`. |

The `handle_result` of the plugin sets the verdict from the scenarios. It does not take a verdict from the agent
(Rule PL5):

- A project with `web_app = false` passes, because a library or an API server has no pages to load.
- A `blocker` asks the user.
- At least one passing scenario and no failing scenario is a pass.
- All other results fail, with one finding for each failing scenario.

The stage logic decides from that outcome (Rule PL3):

- If no run exists, it runs `e2e-tester`.
- A pass ends the stage.
- A blocker asks the user: `retry`, `continue`, or `stop`. Two runs in sequence that end without a result also
  ask the user.
- If all failures are `app` failures, they go to `e2e-runner`. The instructions carry the project, folder,
  command, and failures as a JSON block, and the runner reads that block.
- A `test`, `unknown`, or unclassified failure goes back to `e2e-tester`, with the failures in its instructions.
  The runner marks each failure that it sees as `unknown`, because only the tester can find the cause.
- After 4 runs that follow the last answer of the user, it asks the user. `retry` starts the count again. `continue`
  fails the stage, and `on_fail = "continue"` records the failure and goes to the next stage. `stop` ends the
  session. The `max_rounds` of the node stops the stage after 10 runs in total.

The runner saves the state of its conversation with the implementer as a checkpoint, `implementer:<execution>`,
with the states `asked` and `replied` (Rule PL8). If its program stops when it waits, the resumed run waits for
the reply and does not send the message again. A Playwright JSON report can be larger than the 30,000 characters
that a Bash reply keeps. Thus, the runner writes a short Node script,
[`summarize.cjs`](../../crates/ostra-sdk/examples/web_dev/summarize.cjs), into its session dir. The script changes
the report into at most 30 failures and 100 passes before Ostra reads it.

To use the plugin, run these commands:

```bash
cargo build -p ostra-sdk --example web_dev
ostra plugin add web-dev -- "$PWD/target/debug/examples/web_dev"
```

Then start a session with the workflow `web-dev:web-app`. The tests start the app in the sandbox, so the sandbox
of the workspace must allow loopback connections. The install of Playwright and its browser also needs network
access to the package registry and the browser download hosts. Without this access, the tester reports a
blocker, and you decide whether the session continues without these tests. To run the tests of the example, use
`cargo test -p ostra-sdk --example web_dev`.

## Where to look in the code

| What | Where |
| --- | --- |
| `Plugin`, `PluginAgent` and its builder, `PluginContractDef`, `ResultView`, `AgentCalls`, `Checkpoints` and its limits, the protocol types, `[[plugins]]` validation | [`crates/ostra-core/src/plugin.rs`](../../crates/ostra-core/src/plugin.rs) |
| `AgentContext`, `Registry`, `MemoryCheckpoints`, `pass`, `fail` | [`crates/ostra-sdk/src/lib.rs`](../../crates/ostra-sdk/src/lib.rs) |
| Builders for plugin workflows and transform functions: `Workflow`, `Node`, `TransformFn` | [`crates/ostra-sdk/src/workflow.rs`](../../crates/ostra-sdk/src/workflow.rs) |
| `WorkflowSet::add_plugin`, plugin workflow resolution | [`crates/ostra-core/src/workflow.rs`](../../crates/ostra-core/src/workflow.rs) |
| `PluginTransforms`, `function_info_with` | [`crates/ostra-core/src/transform.rs`](../../crates/ostra-core/src/transform.rs) |
| Agent files: `parse_markdown`, `parse_toml` | [`crates/ostra-sdk/src/definition.rs`](../../crates/ostra-sdk/src/definition.rs) |
| Typed result contracts: `ContractType`, `ContractAgent`, `TypedAgents`, `SubmitExt`, `StageViewExt`, `ResultHandler` | [`crates/ostra-sdk/src/contracts.rs`](../../crates/ostra-sdk/src/contracts.rs) |
| The standard plugin's definitions: `Standard`, `Standard::default_for`, the default workflows | [`crates/ostra-standard/src/lib.rs`](../../crates/ostra-standard/src/lib.rs) |
| The `Pipeline` trait, `PipelineBox`, `Step::Pipeline` | [`crates/ostra-engine/src/pipeline.rs`](../../crates/ostra-engine/src/pipeline.rs), [`crates/ostra-engine/src/plan.rs`](../../crates/ostra-engine/src/plan.rs) |
| The standard pipeline: `OstraPipeline`, its state, fold, planner, judges, views, and step effects | [`crates/ostra-default-plugin/src/`](../../crates/ostra-default-plugin/src/) |
| The stdio transport, `serve`, `StdioPlugin` | [`crates/ostra-sdk/src/stdio.rs`](../../crates/ostra-sdk/src/stdio.rs) |
| `PluginHost`, `ProgramExecutor`, the `Restart` of a run, `contracts`, `infos` | [`crates/ostra-server/src/plugins.rs`](../../crates/ostra-server/src/plugins.rs) |
| The restart of a plugin during a stage decision, result handler, or transform: `call_plugin` | [`crates/ostra-server/src/services.rs`](../../crates/ostra-server/src/services.rs) |
| The save of checkpoints into the log: `SessionCheckpoints` | [`crates/ostra-engine/src/runner/`](../../crates/ostra-engine/src/runner/) |
| `main_with` | [`crates/ostra-server/src/cli.rs`](../../crates/ostra-server/src/cli.rs) |
| Example plugins: `release_gate`, and `web_dev` with its prompt and report summarizer | [`crates/ostra-sdk/examples/`](../../crates/ostra-sdk/examples/) |
| Programmatic runs | [`crates/ostra-exec-native/src/program.rs`](../../crates/ostra-exec-native/src/program.rs) |
| Result handling: `results_due`, `result_view`, `on_result_handled` | [`crates/ostra-engine/src/workflow.rs`](../../crates/ostra-engine/src/workflow.rs) |
| Plugin stage planning and the stage view | [`crates/ostra-engine/src/plugin_stage.rs`](../../crates/ostra-engine/src/plugin_stage.rs) |
| Approval of `[[plugins]]` and the definition files | [`crates/ostra-workspace/src/trust.rs`](../../crates/ostra-workspace/src/trust.rs) |
| Fixtures (`pl3_*`, `pl8_*`), the whole-stack tests, a program that stops two times and resumes | [`tests/conformance/main.rs`](../../tests/conformance/main.rs), [`crates/ostra-server/tests/plugins.rs`](../../crates/ostra-server/tests/plugins.rs), [`crates/ostra-server/tests/plugin_recovery.rs`](../../crates/ostra-server/tests/plugin_recovery.rs) |
