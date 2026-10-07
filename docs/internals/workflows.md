# Workflows

A workflow is the pipeline as a graph of stages. The stages of Ostra (research, spec, plan, build, and the other
built-in stages) are nodes in the graph. A workspace can add its own nodes between them. For example, it can add
an audit after the build, a release check before the closing stages, or a design review after the plan.

A custom node runs one of these items:

- A custom agent ([Agents](agents.md)).
- The stage logic of a plugin ([Plugins](plugins.md)).
- A transform function: a function of Ostra, a function of the workspace, or a function of a plugin.
- One model call (a prompt node).

A plugin can also supply complete workflows that it builds in code. Each node can read the results of earlier
nodes. Each node can also have conditions that skip it. Thus, a workflow can branch on results and change the
shape of data between steps. All gates and rules of the built-in stages stay in effect.

This page covers these topics:

- The default workflows and the file format.
- How a session fixes its workflow, and how the planner walks the graph.
- How custom stages run and end.
- Data between nodes, conditions, and transform and prompt nodes.
- The API of the Workflow builder, and the checks of validation and approval.

The rules are WF1 to WF9 in [HANDOVER section 10.9](../../HANDOVER.md#109-workflows), WB1 to WB7 in
[HANDOVER section 10.11](../../HANDOVER.md#1011-workflow-builder), and PL6 and PL7 in
[HANDOVER section 10.10](../../HANDOVER.md#1010-plugins). PL6 and PL7 apply to plugin workflows and plugin
functions.

## Default workflows

No workflow is defined in code (Rule WF9). Ostra supplies one default workflow for each base pipeline, as a TOML
file in [`assets/workflows/`](../../assets/workflows/). The definitions of the standard plugin `ostra`
([`crates/ostra-standard/src/lib.rs`](../../crates/ostra-standard/src/lib.rs),
[Plugins](plugins.md#the-standard-plugin)) embed these files in the binary. It gives them as the workflows of its
manifest, and `WorkflowSet::add_plugin` keeps them as the `defaults` of the set. Each file lists its built-in
stages in a chain, and each stage waits for the stage before it:

| Workflow | Stages |
| --- | --- |
| `research` | research |
| `spec` | research, spec |
| `plan` | research, spec, plan |
| `implement` | research, track, spec, stakes, plan, build, feedback, closing, book |
| `verify`, `prompt`, `quick-change` | build |
| `test`, `docs` | closing, book |

In each default file, the node of the book stage has the ID `docs`:

```toml
# assets/workflows/docs.toml (end)
[[stage]]
id = "closing"
uses = "ostra:closing"

[[stage]]
id = "docs"
uses = "ostra:book"
```

```toml
# assets/workflows/spec.toml
description = "Research, then a spec with its fact-check and your approval."
base = "spec"

[[stage]]
id = "research"
uses = "ostra:research"

[[stage]]
id = "spec"
uses = "ostra:spec"
```

Ostra also reads from the default file the order of built-in stages that each base needs (Rule WF2).
`WorkflowSet::builtin_chain(base)` returns the `uses` stages of the default file in order. `resolve` checks each
workflow of that base against this order. `ostra_default_plugin::workflow(base)` resolves the default file. A set
that Ostra builds without the standard plugin has no defaults. Thus, it resolves no default name and checks no
chain. The server and the workspace add the plugin to each set that they build.

### Workflow names

A workflow name tells `WorkflowSet::resolve` where to look:

- `ostra:<base>` is always the default of Ostra.
- A name with a different `:` is a workflow that a plugin builds in code, `<plugin>:<name>` (Rule PL6,
  [Plugins](plugins.md#plugin-workflows)). It resolves only when that plugin runs for the workspace. It runs only
  when a session names it, and never as the default of a category.
- Each other name is the workspace file with that name. If the workspace has no such file, the name is the
  default of Ostra.

A workspace file, a plugin workflow, and a default can each extend the others by these names.

A new workspace starts with a copy of each default in `.ostra/workflows/` (`create_workspace` in
[`crates/ostra-workspace/src/trust.rs`](../../crates/ostra-workspace/src/trust.rs)). Ostra writes the copies
before it records the approval of the workspace, so the copies count as approved. A folder that arrives with its
own `workspace.toml` gets no copies. The copies are usual workflow files. You can edit them in the Workflow
builder or in a text editor.

When Ostra looks up a workflow by name (`WorkflowSet::resolve`), the workspace file has priority over the default
with the same name. `ostra:<base>` always names the default of Ostra. Thus, a workspace without a copy still
runs, and a copy can start from the default with `extends = "ostra:implement"`. Ostra refuses
`extends = "implement"` in `implement.toml`, because the file then extends itself. The error message gives that
fix. A workflow with the name of a category must keep that `base`, because sessions of that category run it.

`WorkspaceDetail.missing_workflows` lists the defaults that have no copy in the workspace
(`WorkflowSet::missing_defaults`). Settings shows this list. `POST /api/workspaces/:ws/workflows/defaults` writes
the missing copies (`restore_default_workflows`). It writes them as a save in Ostra, so an approved workspace
stays approved (Rule A1). It never overwrites a file that exists.

A file names the built-in stages `ostra:<stage>`:

| Stage | What it runs |
| --- | --- |
| `ostra:research` | Research tasks and the Sufficiency judge (Rules D1, D2) |
| `ostra:track` | The Track judge: light or full |
| `ostra:spec` | The spec, its open questions, its fact-check, and its approval. Nothing on the light track |
| `ostra:stakes` | The Stakes judge. Nothing on the light track |
| `ostra:plan` | The plan, its fact-check, and its approval. Nothing on the light track or with low stakes |
| `ostra:build` | The implement and review loop of each phase, and the phase stages |
| `ostra:feedback` | The implementation review gate and its feedback rounds (Rule F1) |
| `ostra:closing` | For each project: format, the closing gate, and tests. Without a book stage after it, also docs and the book |
| `ostra:book` | For each project that the closing gate chose docs for: the docs stage of Rule B10, then the book write |

### A workflow without the book stage

The closing stage of Ostra wrote the docs and the book before the book stage existed. A workspace copy of a
default workflow from that time has no `ostra:book` node. Such a workflow still writes the docs: without a book
stage, the closing stage runs the docs stage and the book write, as it did before. A session from a log that
recorded no workflow does the same.

This behavior is deprecated. `WorkflowDef::notices` returns one notice for a workflow that has `ostra:closing`
and no `ostra:book`: "Add a stage with `uses = "ostra:book"` after the closing stage. Without it, the closing
stage writes the documentation book, and a later release removes that behavior." The API gives the notices of
each workflow in `WorkflowInfo.notices`. The console does not show them yet. To remove the notice, add the node:

```toml
[[stage]]
id = "docs"
uses = "ostra:book"
```

## The file format

A workspace keeps its workflows in `.ostra/workflows/<name>.toml`. The name is the file name, in lowercase
kebab-case.

```toml
# <workspace>/.ostra/workflows/secure.toml
description = "The implement pipeline with a security audit before the closing stages."
extends = "implement"          # a different workflow: a workspace file, else the default of Ostra. "ostra:<base>" is always the default
default_for = ["implement"]    # the categories that run it when the user names no workflow
remove = ["feedback"]          # stages of the extended workflow to remove
track = "full"                 # implement only: a fixed track in place of the Track judge

[[stage]]
id = "audit"
agent = "security-auditor"     # or uses = "ostra:<stage>", or plugin = "<plugin>:<stage>"
after = ["build"]              # the stages that it waits for
before = ["closing"]           # the stages that wait for it
scope = "project"              # session (the default), project, or phase
on_fail = "retry"              # gate (the default), retry, continue, or fail
max_rounds = 3                 # the default is 3, the limit is 10
instructions = "Check every changed file for secrets and unsafe input handling."

[agents]                       # the agent that fills a contract in each built-in stage that reads the contract
review = "strict-reviewer"
```

The top-level keys are:

- `description`: the text that Ostra shows with the workflow. The default is the description of the extended
  workflow.
- `base`: the built-in pipeline that the workflow runs on. With `extends`, you can omit it. If you set it, it must
  be equal to the base of the extended workflow. The base sets the category of the session. A workflow with the
  name of a category keeps that base.
- `extends`: the workflow whose stages this workflow copies. The value is a file of this workspace, else the
  default of Ostra with that name, or `ostra:<base>` for the default. A chain can have a depth of at most 8.
- `remove`: the IDs of stages of the extended workflow to remove. A stage that waited for a removed stage then
  waits for the stages that the removed stage waited for. Thus, the order stays the same.
- `default_for`: the categories (`implement`, `quick-change`, or the log form `IMPLEMENT`) that run this workflow
  when the user names no workflow. At most one workflow can list a category.
- `track`: a fixed track for the implement pipeline (Rule WF3).
- `[agents]`: binds a result contract to an agent in each built-in stage of the workflow that reads that contract
  (Rule WF8, below). A contract that no built-in stage of the workflow reads is an error.
- `[layout]`: maps node IDs to `[x, y]` positions for the Workflow builder (Rule WB1). The engine never reads it.

Each `[[stage]]` has an `id` (lowercase kebab-case, at most 48 characters). It also has exactly one of these keys:
`uses`, `agent`, `plugin`, `transform` (a [transform node](#transform-nodes)), or `prompt` (a
[prompt node](#prompt-nodes), with the prompt as its value). The other keys are:

- `after`: the stages that this stage waits for. Without this key, a stage waits for the stage before it in the
  file. In a workflow that extends a different workflow, the first stage waits for the last stage of the extended
  workflow. A phase stage waits for the stages that the build stage waits for.
- `before`: the stages that wait for this stage. Two kinds of stage do not become the default stage that the next
  stage waits for: a stage that sets `before`, with or without `after`, and a phase stage.
- `scope`, `on_fail`, `max_rounds`, and `instructions`: these apply to custom stages. A built-in stage cannot set
  a scope.
- `lane`: Ostra keeps this value with the stage. But the board now shows the runs of all custom stages in the
  Review lane.
- `agents`: on a built-in stage only. It binds contracts for that one stage, for example
  `agents = { spec = "my-spec-writer" }`. It has priority over the top-level `[agents]`.
- `inputs`: maps names to [references](#data-between-nodes) to earlier nodes, for example
  `{ items = "audit.findings" }`.
- `args`: on a transform node only. It holds the fixed arguments of the function.
- `when`: a list of [conditions](#conditions-and-skipping). `when_mode` (`all`, the default, or `any`) combines
  them.
- `output_schema`: on a prompt node only. It is the JSON Schema that the answer must match.
- `tier` and `effort`: these set the model of a prompt node. No other node takes them. A prompt node takes no
  `instructions`.

A built-in stage takes no `inputs`, `args`, `when`, or `output_schema`, because it always runs on its own rules.

### Which agent fills a built-in stage

A built-in stage reads results by contract, not by agent (Rule CA5). Thus, a workflow can replace the agent
behind each built-in stage (Rule WF8). `BuiltinStage::contracts` lists the contracts that each stage reads:

| Stage | Contracts that it reads |
| --- | --- |
| `ostra:research` | `research` |
| `ostra:spec` | `spec`, `fact-check` |
| `ostra:plan` | `plan`, `fact-check` |
| `ostra:build` | `implementation`, `review`, `prompt`, `advice` |
| `ostra:closing` | `path-analysis`, `tests`, `review`, `documentation`, `implementation`, `advice` |
| `ostra:book` | `documentation`, `fact-check` |
| `ostra:track`, `ostra:stakes`, `ostra:feedback` | None. These stages ask judges or open gates |

When the planner spawns an agent for a stage, it calls `SessionState::agent_for(stage, contract)`. This function
returns the agent that the workflow binds to that contract on that stage. If there is no binding, it returns the
agent of the standard plugin for the contract (`Pipeline::default_agent`, which reads
`Standard::default_for`, Rule PL4). In a workflow without a book stage, the docs stage reads the `documentation`
binding of the closing stage.

For example, `[agents] review = "strict-reviewer"` replaces `code-reviewer` in the review loops of the build stage
and in the closing review. An `agents` key on one stage changes only that stage. A contract that no built-in
stage binds runs the agent of the standard plugin (`default_agent`). Examples are a quick answer and the setup
step of a project in an init session. A work loop records the contracts that it spawns for (`work` and `fix`),
not agent names. Thus, a later round calls `agent_for` again and runs the same bound agent.

## A session fixes its workflow

A session records the workflow that it runs (Rule WF1). A workflow file can change during a session, but the
session must not change with it. The reason is that the fold must stay a function of the event log (CLAUDE.md
pattern 1). The steps are:

1. The `SessionCreated` event of a new session contains a `WorkflowChoice`. It is `Named` when the New task
   request named a workflow, and `ByCategory` in all other cases. The Workflow menu of the New task form lists
   `WorkspaceDetail.workflows`. This list contains the valid workflow files of the workspace, the defaults of
   Ostra that have no copy, and the workflows of the plugins that run for the workspace. `WorkflowInfo.plugin`
   names the plugin. The form sends the selected name as `CreateSession.workflow`. Its default, "by category",
   sends no name.
2. The planner emits `Step::ResolveWorkflow` before a stage runs. For a named workflow, the step comes before
   Classify, because the base of the workflow becomes the category. For `ByCategory`, the step comes after
   Classify selects the category.
3. The runner starts the plugin programs of the workspace and reads the workflow files. Then it adds the
   workflows and transform functions of each plugin that runs (`Services::workflows`, through
   `WorkflowSet::add_plugin`).
4. The runner resolves the workflow with `WorkflowSet::resolve` or with `default_for`. `default_for` takes the
   file whose `default_for` lists the category. Else it takes the workflow with the name of the category: the
   copy of the workspace or the default of Ostra. Both functions apply `extends` and `remove` and validate the
   result.
5. The runner checks the agents and plugin stages that the workflow names against the workspace (`check_runnable`
   in [`crates/ostra-engine/src/workflow.rs`](../../crates/ostra-engine/src/workflow.rs)). Then it appends
   `WorkflowResolved` with the complete resolved workflow, or `SessionFailed` with the reason.
6. The fold keeps the workflow and gives it to the pipeline (`Pipeline::workflow_resolved`). If the workflow has a
   fixed `track`, the standard pipeline makes that track the track of the session. The exception is a track that
   the New task form set.

The check in step 5 requires these conditions:

- The agent of an `agent` stage exists and returns `stage` or a plugin contract.
- A plugin serves each `plugin` stage.
- Each agent that the workflow binds to a built-in stage exists and returns the contract that it fills.

Ostra refuses a request that names a workflow that does not resolve or cannot run. It refuses the request before
the session exists. A session from a log that Ostra wrote before workflows existed has no `WorkflowChoice`. It
runs the built-in workflow of its category without a step. Thus, old sessions recover in the same way that they
ran.

`WorkflowResolved` contains each node with its inputs, arguments, and conditions. It also contains the composites
of the workspace that the workflow calls (`functions`). For each plugin transform function that the workflow
calls, it contains the inputs and the output of the function (`plugin_transforms`). Thus, a session runs the
graph that it started with, also after a file or a plugin changes.

## How the planner walks the graph

`workflow_flow` in [`crates/ostra-engine/src/plan.rs`](../../crates/ostra-engine/src/plan.rs) visits the stages in
dependency order. A stage runs when each stage in its `after` is done (Rule WF4). Thus, stages with the same
dependencies run at the same time. Each spawn goes through the slot limiter and the budget guard. Each stage
reports whether it is done, and the session completes when all stages are done.

Each kind of stage goes to its own function:

- An agent stage goes to `agent_stage`.
- A plugin stage goes to `plugin_stage`.
- A transform node or a prompt node goes to `data_stage`.

Each of these functions first checks the conditions of the node (`skip_step`, below).

A built-in stage goes to the pipeline (`Pipeline::builtin_stage`). The standard pipeline calls the function that
the fixed pipeline called (`builtin_stage` in
[`crates/ostra-default-plugin/src/planner/shared.rs`](../../crates/ostra-default-plugin/src/planner/shared.rs)):

```rust
match b {
    BuiltinStage::Research => self.explore_complete(),
    BuiltinStage::Track => { /* ask the Track judge until the track is set */ }
    BuiltinStage::Spec => light || self.spec_flow(wf.base != Category::Spec),
    BuiltinStage::Stakes => { /* skip on light, else ask the Stakes judge */ }
    BuiltinStage::Plan if implement => { /* skip on light; plan unless stakes are low */ }
    BuiltinStage::Plan => self.plan_flow(),
    BuiltinStage::Build => { self.phases(); self.phase_stages(wf); self.build_done(wf) }
    BuiltinStage::Feedback => self.implementation_review(),
    BuiltinStage::Closing => { self.closing_stages(); self.all_implement_done() }
    BuiltinStage::Book => self.book_flow(),
}
```

The book stage is done (`book_flow`) when the docs stage of each project that the closing gate chose docs for is
settled, and the engine wrote the book. A workflow without a book stage keeps that work in
`all_implement_done` of the closing stage.

The build stage is done (`build_done`) when two conditions are true:

- Each phase ended its implement loop. A phase that a blocked dependency removed does not count.
- Each phase stage of each passed phase is done.

The built-in workflows chain these calls in the same order as the fixed pipeline. One order changed: with a book
stage, the docs of a project start after the closing stage of every project is done. Without a book stage, the
docs of one project can run at the same time as the tests of a different project.

## Custom stages

### Scopes

A custom stage runs one time for each instance of its scope (`stage_scopes`):

- `session`: one time.
- `project`: one time for each project in the scope of the session, with the scope value `project:<key>`.
- `phase`: one time for each plan phase whose review passed, with the value `phase:<n>`. Phase stages run in the
  build stage (`phase_stages`). A phase that depends on a different phase also waits for the stages of that phase
  (`deps_passed`). Thus, a check for each phase stops the next phase until the check is done.

### What the agent gets

Each run of a custom agent is an execution with the `Stage { node, scope, round }` purpose, in the Custom stage
kind. Its spawn block (`CustomParams` in [`crates/ostra-agents/src/spawn.rs`](../../crates/ostra-agents/src/spawn.rs))
names these items:

- The stage, the round, the current request, and the instructions of the node.
- The spec and the master plan, if they exist, and the research documents.
- For a phase stage: the phase, its phase file, and its implementer report.
- One line for each earlier custom stage, with its verdict, summary, and report (`Earlier stages`).
- On a retry: the findings of the previous round, and your answers at the gates of the stage.

A session stage works in the session folder. A project stage or a phase stage works in the session folder of its
project.

The scope also sets the projects that the run works in (Rule WD1). A session stage works in every project in the
scope of the session. A project stage works in its project. A phase stage works in each project of its phase,
because a phase can name several projects (Rule WD2). A harness run works in its main project only (Rule WD3).

### How a stage ends

The agent of a custom stage returns the `stage` contract or a contract of a plugin. A `stage` result contains a
verdict (Rule CA3), and the fold (`stage_finished`) applies it.

The result of a plugin contract waits for its plugin (Rule PL5). The steps are:

1. The run ends with a submit, and the planner emits `Step::HandleResult`.
2. The runner calls `handle_result` of the plugin with the `ResultView` of the run. The `ResultView` contains the
   session, execution, agent, contract, stage, scope, and the submit.
3. The runner appends `ResultHandled` with the verdict that the plugin returned. If the plugin cannot handle the
   result, the verdict is `fail` with the error. Then the `on_fail` of the stage decides the next action.
4. The fold applies that verdict in the same way as the verdict of a `stage` agent ([Plugins](plugins.md)).

In both cases, the verdict decides the next action:

| Verdict | What follows |
| --- | --- |
| `pass` | The stage instance is done. |
| `needs_user` | A `stage_review` gate with the question and options of the agent. |
| `fail` with `on_fail = "retry"` | The next round, with the findings, when the round is below `max_rounds`. After that, a `stage_review` gate. |
| `fail` with `on_fail = "gate"` | A `stage_review` gate. |
| `fail` with `on_fail = "continue"` | Ostra records the failure, and the stages after it run. |
| `fail` with `on_fail = "fail"` | The session fails, and the reason names the stage. |

Other ends of a run are:

- A run that ended without a valid custom submit opens the gate as a failure.
- A run with an error opens the `execution_failed` gate. There, `retry` runs the stage again, and `abandon` lets
  the workflow continue without it.
- A run that the pause interrupted continues in the same round (Rule P2).
- A run that waits for a message holds the stage until a message wakes it.

At the `stage_review` gate ([Gates and judges](gates-and-judges.md#stage-review)), your choices depend on the gate.
After a failure, you select `retry` with optional guidance, `continue`, or `stop`. For a question, you select an
option, `other` with your answer, or `stop`. Your words become user notes in the spawn of the next round. Each new
round continues the conversation of the agent and does not start a new one (Rule H5). Thus, the agent keeps what
it read. Under YOLO, a question takes its first option, and a failure runs again when rounds are left. After the
last round, the gate waits for you.

### Talking to the stages around it

The agent of a stage can send messages to the subagents that it works with (Rule WF6). `ListAgents` marks these
agents:

- The author of the spec.
- The author of the plan.
- The agent of each earlier custom stage.
- The implementer of the phase or project that the stage checks.

For example, an audit that finds a problem can send a message to the implementer. The implementer continues its
own conversation and uses its tools to fix the problem. The audit can wait for the reply before it decides its
verdict ([Messages between subagents](agents.md)).

## Data between nodes

A node reads earlier results through references (Rule WB4). A reference is `<node>.<path>`, and `parse_ref` in
[`crates/ostra-core/src/transform.rs`](../../crates/ostra-core/src/transform.rs) parses it. The first part names a
node, `session`, or `scope`. Each later part is an object key or an array index. A step that is not there reads
as null.

The value of each node is (`SessionState::node_value` in
[`crates/ostra-engine/src/workflow.rs`](../../crates/ostra-engine/src/workflow.rs)). The pipeline gives the
value of a built-in stage (`Pipeline::stage_value`) and the pipeline facts in `session` (`Pipeline::session_facts`):

| Node | Value |
| --- | --- |
| Agent or plugin stage | Its last submit (`verdict`, `summary`, `findings`, `question`, `options`, `report_path`, `data`), with `output` set to its `data` and `scope` set to its instance |
| Transform or prompt node | `{verdict: "pass", summary, data, output}`, where `output` is the result of the node |
| Skipped node | null |
| `ostra:research` | `{research_docs}` |
| `ostra:track` | `{track}` |
| `ostra:spec` | `{spec_file}` |
| `ostra:stakes` | `{stakes}` |
| `ostra:plan` | `{master_plan, phases}` |
| `ostra:build` | `{phases}` |
| `ostra:feedback` | `{}` |
| `ostra:closing` | `{docs}`. From a project instance, `docs` is `true` when the closing gate chose docs for that project and no BLOCKER is open (Hard rule 21). From a different scope, `docs` is the list of the projects with docs. |
| `ostra:book` | `{book}`: the ID of the book that the session wrote, or null |
| `session` | `{request, category, track, stakes, projects, title}` |
| `scope` | `{kind: "session"}`, `{kind: "project", project}`, or `{kind: "phase", phase, project}` |

The scope decides which instance a reference reads:

- A reference reads a session node as its one instance.
- A node of the same scope kind reads a project node or a phase node as its own instance. For example, a project
  node that reads a different project node gets the instance of the same project.
- A node of a different scope reads a project node or a phase node as the list of all its instances.

A node can read only the nodes that it waits for, directly or through other nodes. Thus, their results exist when
the node runs. The error message names the wait to add. `session` and `scope` need no wait.

`inputs` resolves each named reference (`node_inputs`). The node then uses the inputs in these ways:

- A transform gets them as its inputs.
- The spawn of a custom agent lists them under `Inputs`, one `name = <json>` line for each
  (`CustomParams.inputs`).
- The `StageView` of a plugin stage contains them as `inputs`. Thus, the stage logic sees them when it decides.

## Conditions and skipping

`when` holds conditions on references (Rule WB5). Each condition has a `ref` and an `op`. It also has a `value`
for the operators that compare against one:

| Operator | True when |
| --- | --- |
| `eq`, `ne` | The value is equal, or not equal, to `value` (`1` is equal to `1.0`) |
| `gt`, `ge`, `lt`, `le` | Both are numbers, or both are strings, and the comparison is true |
| `contains` | A string contains the text, an array contains the value, or an object has the key |
| `in` | The value is one of the items of `value`, or a substring of a `value` string |
| `exists` | The value is not null |
| `empty`, `not_empty` | The value is, or is not, null, `""`, `[]`, or `{}` |
| `truthy`, `falsy` | The value is, or is not, a value other than null, false, 0, `""`, `[]`, and `{}` |

A condition is a validation error in two cases:

- Its operator takes a value, and the condition gives no value.
- The condition gives a value, and its operator takes no value.

All conditions must be true. With `when_mode = "any"`, one condition must be true.

The planner checks the conditions of a node after the nodes that it waits for are done, and before the node starts
(`skip_step`). The conditions read only nodes that the node waits for, and these nodes are done. Thus, the result
cannot change later. If the conditions are not true, these actions occur:

1. The planner emits `Step::SkipStage`.
2. The runner appends `StageSkipped`.
3. The fold marks the instance `Skipped`.

A skipped node counts as done. Thus, the nodes after it run, and branches join again. Ostra never asks a plugin
stage to decide if its conditions are not true.

Two nodes with opposite conditions on the same result make a branch:

```toml
[[stage]]
id = "fix"
agent = "security-fixer"
after = ["auth"]
inputs = { files = "auth.output" }
when = [{ ref = "auth.output", op = "not_empty" }]

[[stage]]
id = "notes"
agent = "release-notes"
after = ["auth"]
when = [{ ref = "auth.output", op = "empty" }]
```

## Transform nodes

A transform node (`transform = "<function>"`) runs a function in the engine, with no agent (Rule WB2). The
function is one of the typed functions of Ostra or one of the [composites](#your-own-transform-functions) of the
workspace.

A node can also call a function that a plugin runs in code, `transform = "<plugin>:<name>"` (Rule PL7). Ostra
checks and types this function in the same way as its own functions. It uses the `TransformInfo` that the plugin
declares. The runner calls the plugin through `Services::plugin_transform` and records the output or the error in
`NodeRan` ([Plugins](plugins.md#plugin-transform-functions)).

Each function declares three things (`transforms()` in
[`crates/ostra-core/src/transform.rs`](../../crates/ostra-core/src/transform.rs)):

- Its inputs, which come from earlier nodes.
- Its arguments, which are fixed in `args`.
- Its output type.

| Function | Inputs | Arguments | Output |
| --- | --- | --- | --- |
| `pick` | `value` | `path` (string) | Any: the field at the dotted path |
| `count` | `items` | none | Number: the items of a list, the keys of an object, or the characters of a text. Null is 0 |
| `filter` | `items` (list) | `field`, `op`, `to` | List: the items whose field passes the comparison |
| `map` | `items` (list) | `field` | List: that field of each item |
| `concat` | any, in name order | none | List: the list inputs joined, and the other inputs added as items |
| `unique` | `items` (list) | none | List: with no repeats, and the first item kept |
| `sort` | `items` (list) | `field`, `descending` (bool) | List |
| `group_count` | `items` (list) | `field` | Object: the count for each field value |
| `sum` | `items` (list) | `field` | Number |
| `compare` | `value` | `op`, `to` | Bool |
| `all`, `any` | any | none | Bool: all inputs truthy, or one input truthy |
| `not` | `value` | none | Bool |
| `merge` | objects, in name order | none | Object: a later key has priority |
| `template` | any | `text` | String: Ostra replaces `{{name}}` and `{{name.path}}`. Text stays as it is, and other values become JSON |
| `constant` | none | `value` | Any |
| `findings` | node results or lists of them | none | List: their `findings`, joined |

`op` takes the condition operators above. A function with fixed inputs refuses a missing required input and an
unknown input. A function that takes any inputs needs at least one input. At save time, `check_transform` refuses
these errors:

- A missing required argument or an unknown argument.
- An argument with the wrong JSON type.
- An unknown `op`.

A transform node takes no `instructions`.

The planner emits `Step::RunNode` with the node, its scope, and the round (`data_stage_action`). The runner
resolves the inputs and calls `run_transform`. Then it appends `NodeRan` with the output or the error. Ostra
records the output and does not calculate it again. Thus, the fold stays a function of the log, also if a later
version of Ostra calculates a function in a different way.

## Prompt nodes

A prompt node (`prompt = "..."`) makes one model call and gives JSON that matches its `output_schema` (Rule WB3).
The `output_schema` must be an object schema. The node runs natively, through the judge path:

- The system prompt is [`assets/judges/prompt-node.md`](../../assets/judges/prompt-node.md).
- The user message holds the prompt and the inputs as JSON. After a failed round, it also holds your words at the
  gate of the node.
- The answer is a forced `decide` call. The input schema of this call is the `output_schema` of the node.

The route is the judge route on the `tier` of the node (default `balanced`), at its `effort` (default `medium`).
If an answer does not match the schema, Ostra asks for the answer one more time. A second mismatch or a failed
call fails the node with the reason.

Ostra records the cost of the node in `NodeRan` (`cost_usd`) and adds it to the `node_cost` of the session.
`spent_usd` counts `node_cost`, so prompt nodes count toward the session budget. `Step::RunNode` for a prompt node
has `model: true`, and the budget guard holds it in the same way as a spawn
([Spend and limits](spend-and-limits.md)).

### How a transform or prompt node ends

A node that gave its output is done. A node that failed follows its `on_fail`, in the same way as a `fail` verdict
of an agent:

- `continue`: the workflow continues without the output.
- `fail`: the session stops.
- `retry`: the node runs again when rounds are left.
- `gate`, or `retry` after the last round: a `stage_review` gate opens, with the title "Node <id> failed" and the
  options `retry`, `continue`, and `stop`.

A retry from the gate gives your guidance to a prompt node.

## Checking data before it flows

When Ostra checks a workflow, it follows each reference through the shape of the node that the reference names
(Rule WB6, `check_types`). Without this check, a reference that reads nothing shows only when the session runs, as
a failed node. `check_runnable` calls `check_types` at save time, in settings validation, and when a session
resolves the workflow.

The value of each node has a shape, written as a JSON Schema (`node_shape` in
[`crates/ostra-engine/src/workflow.rs`](../../crates/ostra-engine/src/workflow.rs)):

| Node | Shape |
| --- | --- |
| An agent stage | Its submit: `verdict`, `summary`, `question`, `report_path`, `scope` (text), `options` (a list of text), `findings` (a list of objects with `file`, `description`, `fix`), and `data` and `output`, which both have the `data_schema` of the agent. |
| A plugin stage | The same submit, with `data` and `output` that accept all values. |
| A transform | `verdict`, `summary`, `scope`, and `output` of the declared output kind of the function. `filter`, `sort`, and `unique` keep the shape of the list in `items`. |
| A prompt node | `output` is its `output_schema`. |
| A built-in stage | The facts that it sets, as listed in [Data between nodes](#data-between-nodes). |
| `session`, `scope` | Their fields, as text, a list of text (`projects`), or a number (`phase`). |

A node that runs for each project or phase is a list of those shapes, when a node of a different scope reads it.
`ref_shape` walks the path of a reference through the shape and refuses these references:

- A field that an object shape does not declare. For example: ``Node `r` reads `audit.data.riks`, but
  `audit.data` has no field `riks`; it has `risk`.`` To fix it, add the field to the `data_schema` of the agent,
  or correct the name.
- A field on a list. The path must give an index (`audit.findings.0.file`), or the list must go into a transform.
- A field on a text, a number, or a boolean.
- A transform input that gets a value of a kind that the function does not take. For example: ``Wire a list into
  input `items` of node `f`: `n.output` is a number.``

A shape that declares nothing accepts all paths. Examples are the data of a plugin stage, an agent without a
`data_schema`, and a prompt schema without `properties`. Ostra checks the path of each condition in the same way.

`POST /api/workspaces/:ws/workflows/:name/check` runs all checks of a save (`check_workflow` in
[`crates/ostra-workspace/src/builder.rs`](../../crates/ostra-workspace/src/builder.rs)). It returns the problems and
writes nothing. The builder calls it 0.5 seconds after each edit.

## Your own transform functions

A workspace can build a transform function from other functions (Rule WB7). Thus, you write one time a chain that
several workflows use. Each function is a file in `.ostra/transforms/<name>.toml`:

```toml
# .ostra/transforms/high-files.toml
description = "The files of findings at a risk."
output = "files"                 # the step whose output the function returns

[[input]]                        # comes from a node, read as input.<name>
name = "findings"
kind = "array"

[[arg]]                          # set on the node, given to steps as "$<name>"
name = "risk"
kind = "string"

[[step]]
id = "high"
transform = "filter"
inputs = { items = "input.findings" }
args = { field = "risk", op = "eq", to = "$risk" }

[[step]]
id = "files"
transform = "map"
inputs = { items = "high.output" }   # an earlier step, read as <step>.output
args = { field = "file" }
```

An input or an argument has these keys:

- `name`.
- `kind`: `any`, `bool`, `number`, `string`, `array`, or `object`.
- `required`: true if you do not set it.
- `description`.

The steps run in order. Each step runs one function of Ostra or a different composite. A string argument that is
exactly `"$<name>"` takes the argument of the function with that name. The output kind of the function is the
output kind of its `output` step. Ostra follows this kind through nested composites.

A workflow node uses a composite in the same way as other transforms: `transform = "high-files"`, with
`inputs = { findings = "..." }` and `args = { risk = "high" }`. `function_info` in
[`crates/ostra-core/src/transform.rs`](../../crates/ostra-core/src/transform.rs) describes it with the same
`TransformInfo` as the functions of Ostra, with `custom` set. Thus, the node checks, the type check above, and the
palette of the builder use the two kinds of function in the same way.

`check_function` refuses a composite with one of these errors:

- No steps, or more than 32 steps.
- A name that a function of Ostra has.
- An input name or an argument name that is not an identifier.
- A step ID that occurs two times.
- A step whose function does not exist, or calls the composite again.
- A step input that reads an undeclared input or a step that is not earlier.
- A `"$<name>"` that names no declared argument. When Ostra checks the arguments of a step, a declared argument
  gets a placeholder value of its kind.
- An `output` that names no step.

Calls can nest to a depth of at most 8.

A composite cannot call a function of a plugin (`<plugin>:<name>`). `check_function` refuses the step, because a
composite runs in Ostra in one step, and a plugin call is a request to a different program.

`run_function` runs a composite. The steps are:

1. It puts the inputs of the function under `input`.
2. It runs each step. It looks up the inputs of the step and fills the `"$<name>"` arguments.
3. It keeps the result of each step under the step ID.
4. It returns the result of the `output` step.

An error names the function and the step.

`WorkflowSet::load` reads `.ostra/transforms/` next to the workflow files (`load_functions`). When a workflow
resolves, `functions_used` collects each composite that its transform nodes reach into `WorkflowDef.functions`.
It includes composites that other composites call. That map goes in `WorkflowResolved`, and the runner runs a
node from it. Thus, a session runs the version of a function that it started with. An edit of the file changes
only later sessions.

## The Workflow builder's API

The Workflow builder in the console edits a workflow as a diagram: nodes, with an edge for each `after` entry
(Rule WB1). Its server side is in [`crates/ostra-workspace/src/builder.rs`](../../crates/ostra-workspace/src/builder.rs):

| Request | What it does |
| --- | --- |
| `GET /api/workspaces/:ws/builder` | Returns the palette. See the list below this table. |
| `GET /api/workspaces/:ws/workflows/:name` | Returns a `WorkflowDoc`: the workflow with each node written out, the resolved workflow, its problems, and `plugin` when a plugin builds it. |
| `PUT /api/workspaces/:ws/workflows/:name` | Saves a `WorkflowFile`, after the checks below. |
| `DELETE /api/workspaces/:ws/workflows/:name` | Deletes the file, unless a different workflow extends it. |
| `POST /api/workspaces/:ws/workflows/:name/check` | Returns the problems that make Ostra refuse a save of the given `WorkflowFile`. It saves nothing (Rule WB6). |
| `POST /api/workspaces/:ws/workflows/defaults` | Adds the [default workflows](#default-workflows) that have no copy in the workspace. |
| `GET /api/workspaces/:ws/transforms/:name` | Returns a `FunctionDoc`: the file of the composite, its `TransformInfo`, its problems, and the workflow nodes and composites that call it. |
| `PUT /api/workspaces/:ws/transforms/:name` | Saves a `FunctionFile`. Ostra refuses the save if the file fails its checks, or if it breaks a workflow or composite that worked before. |
| `DELETE /api/workspaces/:ws/transforms/:name` | Deletes the file, unless a workflow node or a different composite calls it. |

The palette contains these items:

- The built-in stages, with their descriptions, the contracts that each reads, and whether a workflow can remove
  it.
- The stages that the running plugins serve.
- Each transform function with its typed inputs and arguments. The functions of Ostra come first, then the
  composites of the workspace (`custom`), then the functions of the running plugins under their full names
  (`plugin`).
- The condition operators.
- The tiers.

Agents come from `WorkspaceDetail.agents`.

The `file` of a `WorkflowDoc` comes from `WorkflowFile::from_def`. This function applies `extends`, `remove`,
`before`, and the top-level `[agents]`, and gives each node its full `after`. Thus, the diagram shows the complete
graph, and a save writes it out. The other fields are:

- `flattened`: the file on disk used `extends` or `remove`. A save from the builder replaces them with the
  written-out nodes.
- `builtin`: the workspace has no copy, and the document is the default of Ostra. A save writes the copy.
- `plugin`: the plugin that builds a `<plugin>:<name>` workflow. The console shows this workflow as read-only.

The document keeps `[layout]` and `default_for` from the file.

Ostra checks a save before it writes a file. It refuses the save with all problems in these cases:

- The workflow does not resolve, or cannot run with the agents and plugin stages of the workspace.
- Its `default_for` gives a category two default workflows.
- It breaks a different workflow that resolved before, for example a workflow that extends it.

A name must be lowercase kebab-case. Ostra refuses a save under the name of a plugin workflow, because the source
of that workflow is the code of the plugin. The error tells you to save under a name of your own. Ostra writes the
file as TOML through `trust::save_definitions`. This function keeps an approved workspace approved and a waiting
workspace waiting (Rule A1).

### The builder in the console

The Workflows page (`/w/<ws>/workflows`) shows these items:

- The workflows of the workspace.
- The defaults of Ostra that have no copy in the workspace, with an action that adds them.
- Each plugin workflow, marked with its plugin and as read-only, with no delete action.
- A control that starts a new workflow from a base or from a listed workflow, which can be a plugin workflow.

`/w/<ws>/workflows#<name>` opens the builder (`web/src/features/builder/WorkflowBuilder.tsx`). The builder draws
the graph with `@xyflow/react`:

- The palette on the left adds a node of each kind. The kinds are a stage of Ostra that the graph does not have
  yet, an agent that returns `stage` or a plugin contract, a plugin stage, a transform, and a prompt. A new node
  waits for the selected node, if one is selected.
- In the diagram, each edge is one `after` entry. To add an edge, drag from the right handle of a node to the left
  handle of a different node. Delete removes the selected node or edge. A node with conditions shows them, and its
  incoming edges are animated.
- The inspector on the right edits the selected node. It edits the node ID: a rename also changes the `after`
  entries and the references. It also edits the agent bound to each contract of an Ostra stage, and the prompt,
  tier, and output schema of a prompt node. Other fields are the inputs, the typed arguments of a transform, the
  conditions and their mode, the scope, `on_fail`, and `max_rounds`. For inputs, it suggests references to the
  other nodes and to `session` and `scope`.
- The composites of the workspace are a separate palette group, "Your transforms". The functions of the running
  plugins are a different group, "Plugin transforms".
- A plugin workflow opens as read-only. A banner says that its plugin builds it in code. The palette, the live
  check, and Save are off. To change a copy, start a new workflow from it.
- 0.5 seconds after each edit, the builder sends the graph to `POST .../workflows/<name>/check`. It lists the
  problems in a banner. It draws a red outline on each node whose ID a problem names.
- Save sends the complete graph and the position of each node to `PUT .../workflows/<name>`. It shows the problems
  of a refused save. A node without a position gets one from its depth in the graph.

The Workflows page also has a Transforms panel. The panel lists the composites of the workspace and the functions
of the running plugins, with their output kind. It also has a name field that starts a new composite. The panel
lists a function of a plugin with its plugin and gives it no editor, because the plugin runs it in code.

`/w/<ws>/workflows#transform:<name>` opens the editor of the composite
(`web/src/features/builder/TransformEditor.tsx`). The editor shows these items:

- The description.
- The inputs and arguments, with their kind and whether each one is required.
- The steps in order. Each step selects a function of Ostra or of the workspace, but not of a plugin. It connects
  the inputs of that function, with suggestions of `input.<name>` and earlier `<step>.output`. It also sets the
  arguments.
- The output step.

Save and Delete show the problems that make the server refuse them. The editor lists the users of the function.

## Validation and approval

Ostra checks a workflow when it validates the settings of the workspace. It checks it again when a session
resolves it (`WorkflowDef::validate` and `check_runnable`). A workflow must keep these conditions:

- The stage IDs are unique and well formed. Each `after` names stages of the workflow, and there are no loops.
- Each built-in stage of its base pipeline is present (Rule WF2). The exceptions are `ostra:feedback`,
  `ostra:closing`, `ostra:book`, and `ostra:track` with a fixed `track`. No built-in stage is present that its base does not
  have.
- Each built-in stage waits for the built-in stage before it, directly or through other stages.
- There are at most 24 custom stages, and `max_rounds` is from 1 to 10.
- `agents` bindings are only on built-in stages, and only for contracts that the stage reads (Rule WF8).
- A phase stage has no order against the build stage, in a workflow with a build stage. No stage waits for a phase
  stage directly (Rule WF7).
- Each agent and plugin stage that the workflow names is present in the workspace. The agent of an `agent` stage
  returns `stage` or a plugin contract. Each bound agent returns the contract that it fills (`check_runnable`).
- Each node passes `StageDef::node_issues`. See the list below.
- Each input reference and condition reference reads a field that the shape of the named node gives. Each
  transform input gets a value of the kind that it takes (`check_types`, Rule WB6,
  [above](#checking-data-before-it-flows)).

`StageDef::node_issues` requires these conditions on each node:

- `inputs` have valid names and references.
- References name only nodes that the node waits for.
- A condition has a `value` exactly when its operator takes one.
- `args` occur only on a transform node, and `output_schema` only on a prompt node.
- A prompt node has a prompt and an object `output_schema`.
- The inputs and arguments of a transform match its function: a function of Ostra or a composite of the workspace.

The contract of an agent decides where it can run. The source of its definition, Ostra or the workspace, does not
(Rule CA5). A custom agent that returns `review` can fill the review contract of a built-in stage. An `agent`
stage takes each agent that returns `stage` or a plugin contract.

Ostra reports each failure as a settings issue:

- A workflow file that fails one of these checks: under `workflows.<name>`.
- A composite that fails `check_function`: under `transforms.<name>`.
- A plugin workflow that cannot run: under `plugins`.

Workflow files and composite transform files are part of the workspace content that you approve (Rule A1). Custom
agent files and plugin programs are also part of it. If a folder arrives with these files, Ostra runs none of them
until you approve them in Settings. Settings lists a composite as a Transform. Agents cannot write
`.ostra/workflows/` or `.ostra/transforms/`, because both are protected paths.

## A worked example

A team wants an audit of each changed project before tests and docs. It also wants a short release note at the
same time.

```toml
# .ostra/workflows/audited.toml
extends = "implement"
default_for = ["implement"]

[[stage]]
id = "audit"
agent = "security-auditor"
after = ["feedback"]
before = ["closing"]
scope = "project"
on_fail = "retry"
max_rounds = 2

[[stage]]
id = "notes"
agent = "release-notes"
after = ["feedback"]
```

A session that the judge classifies as IMPLEMENT records `audited`. After you accept the implementation, the
planner starts an audit run for each project and the release notes at the same time. The closing stages wait for
the two audits, but not for the notes. If an audit fails, it runs one more time with its findings, and it continues
its conversation. If it fails again, the board shows a `stage_review` gate with its findings. There, you select one
of three actions: run it again, continue without it, or stop. The session completes when the closing stages, the
audits, and the notes are all done.

## Where to look in the code

| What | Where |
| --- | --- |
| The file format, resolution, default workflows, validation | [`crates/ostra-core/src/workflow.rs`](../../crates/ostra-core/src/workflow.rs) |
| The default workflow files | [`assets/workflows/`](../../assets/workflows/) |
| References, conditions, transform functions, composites (`FunctionFile`, `check_function`, `run_function`, `functions_used`), plugin functions (`PluginTransforms`, `function_info_with`) | [`crates/ostra-core/src/transform.rs`](../../crates/ostra-core/src/transform.rs) |
| The builder reads, checks, and saves of workflows and composites, and the restore of defaults | [`crates/ostra-workspace/src/builder.rs`](../../crates/ostra-workspace/src/builder.rs) |
| The builder and the composite editor in the console | [`web/src/features/builder/`](../../web/src/features/builder/) |
| `WorkflowResolved`, `StageSkipped`, `NodeRan`, `StageReview`, the `Stage` purpose | [`crates/ostra-core/src/event.rs`](../../crates/ostra-core/src/event.rs) |
| The walk: `workflow_flow`, `phase_stages` | [`crates/ostra-engine/src/plan.rs`](../../crates/ostra-engine/src/plan.rs) |
| The built-in stages: `builtin_stage` and the closing and book values | [`crates/ostra-default-plugin/src/planner/shared.rs`](../../crates/ostra-default-plugin/src/planner/shared.rs) |
| `build_done`, `book_flow` | [`stages/build/planner.rs`](../../crates/ostra-default-plugin/src/stages/build/planner.rs), [`stages/book/planner.rs`](../../crates/ostra-default-plugin/src/stages/book/planner.rs) |
| The deprecation notices of a workflow | `WorkflowDef::notices` in [`crates/ostra-core/src/workflow.rs`](../../crates/ostra-core/src/workflow.rs) |
| The stage fold, stage actions, node values, `skip_step`, `data_stage_action`, `check_runnable`, shapes and `check_types`, `agent_for`, `results_due`, `result_view`, partners | [`crates/ostra-engine/src/workflow.rs`](../../crates/ostra-engine/src/workflow.rs) |
| `Step::ResolveWorkflow`, `Step::HandleResult`, `Step::SkipStage`, `Step::RunNode` | `perform` in [`crates/ostra-engine/src/runner/driver.rs`](../../crates/ostra-engine/src/runner/driver.rs) |
| `prompt_node` | [`crates/ostra-engine/src/runner/judges.rs`](../../crates/ostra-engine/src/runner/judges.rs) |
| YOLO at a stage gate | `yolo_plan` in [`crates/ostra-default-plugin/src/judge_input/yolo.rs`](../../crates/ostra-default-plugin/src/judge_input/yolo.rs) |
| Fixtures (`wf*`, `wb*`, with `wb7_*` for composites, and `pl6_pl7_*` for plugin workflows and functions) | [`tests/conformance/main.rs`](../../tests/conformance/main.rs) |
