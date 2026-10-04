# Workflows

A workflow is the pipeline as a graph of stages. Ostra's own stages (research, spec, plan, build, and the rest)
are nodes in it, and a workspace can add its own nodes between them: an audit after the build, a release check
before the closing stages, a design review after the plan. A custom node runs a custom agent
([Agents](agents.md)), a plugin's stage logic ([Plugins](plugins.md)), a transform function (Ostra's, the
workspace's own, or a plugin's), or one model call (a prompt node). A plugin can also ship whole workflows built
in code. Any node may read what earlier nodes produced and may carry conditions that
skip it, so a workflow can branch on results and reshape data between steps. Every gate and rule of the
built-in stages still holds.

This page covers the default workflows, the file format, how a session fixes its workflow, how the planner walks
the graph, how custom stages run and end, data between nodes, conditions, transform and prompt nodes, the
Workflow builder's API, and what validation and approval check. The rules are WF1 to WF9 in
[HANDOVER section 10.9](../../HANDOVER.md#109-workflows), WB1 to WB7 in
[HANDOVER section 10.11](../../HANDOVER.md#1011-workflow-builder), and PL6 and PL7 for plugin workflows and
functions in [HANDOVER section 10.10](../../HANDOVER.md#1010-plugins).

## Default workflows

No workflow is defined in code (Rule WF9). Ostra ships one default workflow per base pipeline as a TOML file
in [`assets/workflows/`](../../assets/workflows/), embedded in the binary through `DEFAULT_WORKFLOWS` in
[`crates/ostra-core/src/workflow.rs`](../../crates/ostra-core/src/workflow.rs). Each lists its built-in stages
in a chain, each waiting for the one before it:

| Workflow | Stages |
| --- | --- |
| `research` | research |
| `spec` | research, spec |
| `plan` | research, spec, plan |
| `implement` | research, track, spec, stakes, plan, build, feedback, closing |
| `verify`, `prompt`, `quick-change` | build |
| `test`, `docs` | closing |

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

The order of built-in stages each base needs (Rule WF2) is read from its default too: `builtin_chain(base)`
returns the `uses` stages of the default file in order, and validation checks every workflow of that base
against it. `WorkflowDef::builtin(base)` resolves the default file.

### Workflow names

A workflow name tells `WorkflowSet::resolve` where to look. `ostra:<base>` is always Ostra's default. A name
that holds any other `:` is a workflow a plugin builds in code, `<plugin>:<name>` (Rule PL6,
[Plugins](plugins.md#plugin-workflows)); it resolves only while that plugin runs for the workspace, and runs only
when a session names it, never as a category's default. Any other name is the workspace's file of that name, or
Ostra's default when the workspace has no such file. A workspace file, a plugin workflow, and a default can each
extend the others by these names.

A new workspace starts with a copy of each default in `.ostra/workflows/` (`create_workspace` in
[`crates/ostra-workspace/src/trust.rs`](../../crates/ostra-workspace/src/trust.rs)), written before its approval
is recorded, so the copies count as approved. A folder that arrives with its own `workspace.toml` gets no copies.
The copies are ordinary workflow files: you edit them in the Workflow builder or by hand.

When Ostra looks up a workflow by name (`WorkflowSet::resolve`), the workspace's file wins over the default of
the same name, and `ostra:<base>` always names Ostra's default. So a workspace without a copy still runs, and a
copy can start from the default with `extends = "ostra:implement"`; `extends = "implement"` in `implement.toml`
is refused as extending itself, with that fix in the message. A workflow named after a category must keep that
`base`, because sessions of the category run it.

`WorkspaceDetail.missing_workflows` lists the defaults the workspace has no copy of
(`WorkflowSet::missing_defaults`), which Settings shows. `POST /api/workspaces/:ws/workflows/defaults` writes
the missing ones (`restore_default_workflows`) as a save made in Ostra, so an approved workspace stays approved
(Rule A1), and never overwrites a file that exists.

The built-in stages are named `ostra:<stage>` in a file:

| Stage | What it runs |
| --- | --- |
| `ostra:research` | Research tasks and the Sufficiency judge (Rules D1, D2) |
| `ostra:track` | The Track judge: light or full |
| `ostra:spec` | The spec, its open questions, its fact-check, and its approval; nothing on the light track |
| `ostra:stakes` | The Stakes judge; nothing on the light track |
| `ostra:plan` | The plan, its fact-check, and its approval; nothing on the light track or with low stakes |
| `ostra:build` | Every phase's implement and review loop, and the phase stages |
| `ostra:feedback` | The implementation review gate and its feedback rounds (Rule F1) |
| `ostra:closing` | Per project: format, the closing gate, tests, docs, and the book |

## The file format

A workspace keeps its workflows in `.ostra/workflows/<name>.toml`. The name is the file name, in lowercase
kebab-case.

```toml
# <workspace>/.ostra/workflows/secure.toml
description = "The implement pipeline with a security audit before the closing stages."
extends = "implement"          # another workflow: a workspace file, else Ostra's default; "ostra:<base>" is always the default
default_for = ["implement"]    # categories that run it when the user names none
remove = ["feedback"]          # stages of the extended workflow to leave out
track = "full"                 # implement only: a fixed track in place of the Track judge

[[stage]]
id = "audit"
agent = "security-auditor"     # or uses = "ostra:<stage>", or plugin = "<plugin>:<stage>"
after = ["build"]              # the stages it waits for
before = ["closing"]           # the stages that wait for it
scope = "project"              # session (the default), project, or phase
on_fail = "retry"              # gate (the default), retry, continue, or fail
max_rounds = 3                 # the default is 3, the limit 10
instructions = "Check every changed file for secrets and unsafe input handling."

[agents]                       # which agent fills a contract in every built-in stage that reads it
review = "strict-reviewer"
```

The top-level keys:

- `description` is shown with the workflow. It defaults to the extended workflow's.
- `base` names the built-in pipeline the workflow runs on. With `extends` it may be left out, and when given it
  must equal the extended workflow's. The base sets the session's category. A workflow named after a category
  keeps that base.
- `extends` copies another workflow's stages: a file of this workspace, else Ostra's default of that name, or
  `ostra:<base>` for the default. A chain may be up to 8 deep.
- `remove` drops stages of the extended workflow by id. A stage that waited for a removed one waits for what the
  removed one waited for instead, so the order holds.
- `default_for` lists categories (`implement`, `quick-change`, or the log's form `IMPLEMENT`) this workflow
  runs for when the user names no workflow. At most one workflow may list a category.
- `track` fixes the implement pipeline's track (Rule WF3).
- `[agents]` binds a result contract to an agent in every built-in stage of the workflow that reads that
  contract (Rule WF8, below). A contract no built-in stage of the workflow reads is an error.
- `[layout]` maps node ids to `[x, y]` positions for the Workflow builder (Rule WB1). The engine never reads it.

Each `[[stage]]` has an `id` (lowercase kebab-case, at most 48 characters) and exactly one of `uses`, `agent`,
`plugin`, `transform` (a [transform node](#transform-nodes)), or `prompt` (a [prompt node](#prompt-nodes), whose
value is the prompt). The rest:

- `after` lists the stages it waits for. Without it, a stage waits for the stage before it in the file, or, in a
  workflow that extends another, for the extended workflow's last stage. A phase stage waits for what the build
  stage waits for instead.
- `before` lists stages that wait for this one. A stage that sets `before`, whether or not it also sets `after`,
  and a phase stage do not become the stage the next one waits for by default.
- `scope`, `on_fail`, `max_rounds`, and `instructions` apply to custom stages; a built-in stage may not set a
  scope.
- `lane` is kept with the stage, but the board shows every custom stage's runs in the Review lane today.
- `agents`, on a built-in stage only, binds contracts for that stage alone:
  `agents = { spec = "my-spec-writer" }`. It wins over the top-level `[agents]`.
- `inputs` maps names to [references](#data-between-nodes) to earlier nodes: `{ items = "audit.findings" }`.
- `args`, on a transform node only, holds the function's fixed arguments.
- `when` lists [conditions](#conditions-and-skipping), and `when_mode` (`all`, the default, or `any`) combines them.
- `output_schema`, on a prompt node only, is the JSON Schema its answer matches. `tier` and `effort` set a prompt
  node's model; no other node takes them, and a prompt node takes no `instructions`.
- A built-in stage takes none of `inputs`, `args`, `when`, or `output_schema`, because it always runs on its own
  rules.

### Which agent fills a built-in stage

A built-in stage reads results by contract, not by agent (Rule CA5), so a workflow can swap the agent behind
any of them (Rule WF8). `BuiltinStage::contracts` lists what each stage reads:

| Stage | Contracts it reads |
| --- | --- |
| `ostra:research` | `research` |
| `ostra:spec` | `spec`, `fact-check` |
| `ostra:plan` | `plan`, `fact-check` |
| `ostra:build` | `implementation`, `review`, `prompt`, `advice` |
| `ostra:closing` | `path-analysis`, `tests`, `review`, `documentation`, `architecture`, `implementation`, `advice` |
| `ostra:track`, `ostra:stakes`, `ostra:feedback` | none; they ask judges or gates |

When the planner spawns for a stage, `SessionState::agent_for(stage, contract)` takes the agent the workflow
binds to that contract on that stage, and otherwise the standard plugin's agent for it
(`Standard::default_for`, Rule PL4). So `[agents] review = "strict-reviewer"` replaces `code-reviewer` in both
the build stage's review loops and the closing review, while a stage-level `agents` changes one stage. A
contract no built-in stage binds, such as a quick answer or a project's setup step in an init session, runs
the standard plugin's agent (`default_agent`). A work loop records the contracts it spawns for (`work` and
`fix`), not agent names, so a later round asks `agent_for` again and runs the same bound agent.

## A session fixes its workflow

A workflow file can change while a session runs, and a session must not change course with it, because the
fold has to stay a function of the event log (CLAUDE.md pattern 1). So a session records the workflow it runs
(Rule WF1):

1. A new session's `SessionCreated` event carries a `WorkflowChoice`: `Named` when the New task request named a
   workflow, `ByCategory` otherwise. The New task form's Workflow menu lists `WorkspaceDetail.workflows`, the
   workspace's valid workflow files, Ostra's defaults it has no copy of, and the workflows of the plugins that
   run for it (`WorkflowInfo.plugin` names the plugin), and sends the picked name as
   `CreateSession.workflow`; its default, "by category", sends none.
2. The planner emits `Step::ResolveWorkflow` before any stage runs: for a named workflow before Classify, because
   its base becomes the category whatever Classify picks; for `ByCategory` once Classify picked the category.
3. The runner starts the workspace's plugin programs, reads the workflow files and adds each running plugin's
   workflows and transform functions (`Services::workflows`, through `WorkflowSet::add_plugin`), resolves the workflow
   (`WorkflowSet::resolve`, or `default_for`: the file whose `default_for` lists the category, else the
   workflow named after it, the workspace's copy or Ostra's default; both apply `extends` and `remove` and
   validate the result), and
   checks the agents and plugin stages it names against the workspace (`check_runnable` in
   [`crates/ostra-engine/src/workflow.rs`](../../crates/ostra-engine/src/workflow.rs)). It appends
   `WorkflowResolved` with the whole resolved workflow, or `SessionFailed` with the reason. The check
   requires that an `agent` stage's agent exists and returns `stage` or a plugin contract, that a `plugin`
   stage is one a plugin serves, and that every agent bound to a built-in stage exists and returns the
   contract it is bound to.
4. The fold keeps the workflow. A fixed `track` in it becomes the session's track unless the New task form set
   one.

A request that names a workflow that does not resolve or cannot run is refused before the session exists. A
session from a log written before workflows has no `WorkflowChoice` and runs the built-in workflow of its
category without a step, so old sessions recover as they ran.

`WorkflowResolved` carries every node with its inputs, arguments, and conditions, the workspace's composites it
calls (`functions`), and what each plugin transform function it calls takes and gives (`plugin_transforms`), so a
session runs the graph it started with even after a file or a plugin changes.

## How the planner walks the graph

`workflow_flow` in [`crates/ostra-engine/src/plan.rs`](../../crates/ostra-engine/src/plan.rs) visits the stages in
dependency order. A stage runs once every stage in its `after` is done (Rule WF4), so stages with the same
dependencies run side by side, each spawn going through the slot limiter and the budget guard. Each stage
reports whether it is done; the session completes when every stage is. An agent stage goes to `agent_stage`, a
plugin stage to `plugin_stage`, and a transform or prompt node to `data_stage`; each first checks the node's
conditions (`skip_step`, below).

A built-in stage calls the function the fixed pipeline called (`builtin_stage`):

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
}
```

The build stage is done (`build_done`) when every phase that is not removed by a blocked dependency has ended
its implement loop, and every phase stage of every passed phase is done. Because the built-in workflows chain
these calls in the order the fixed pipeline made them, the conformance fixtures written for that pipeline pass
unchanged.

## Custom stages

### Scopes

A custom stage runs once per instance of its scope (`stage_scopes`):

- `session`: once.
- `project`: once per project in the session's scope, with the scope value `project:<key>`.
- `phase`: once per plan phase whose review passed, with the value `phase:<n>`. Phase stages run inside the
  build stage (`phase_stages`), and a phase that depends on another waits for that phase's stages too
  (`deps_passed`), so a per-phase check holds the next phase.

### What the agent is given

Each run of a custom agent is an execution with the `Stage { node, scope, round }` purpose, in the Custom stage
kind. Its spawn block (`CustomParams` in [`crates/ostra-agents/src/spawn.rs`](../../crates/ostra-agents/src/spawn.rs))
names:

- the stage, the round, the request as it now stands, and the node's instructions;
- the spec and the master plan when they exist, and the research documents;
- for a phase stage, the phase, its phase file, and its implementer report;
- one line per earlier custom stage with its verdict, summary, and report (`Earlier stages`);
- the findings of the previous round on a retry, and what you answered at the stage's gates.

A session stage works in the session folder; a project or phase stage works in its project's session folder.

### How a stage ends

A custom stage's agent returns the `stage` contract or a plugin's own contract. A `stage` result carries a
verdict (Rule CA3), and the fold (`stage_finished`) applies it. A plugin contract's result waits for its
plugin (Rule PL5): once the run ends with a submit, the planner emits `Step::HandleResult`, the runner calls
the plugin's `handle_result` with the run's `ResultView` (session, execution, agent, contract, stage, scope,
and the submit), and appends `ResultHandled` with the verdict the plugin returned. A plugin that cannot handle it yields a
`fail` verdict naming the error, so the stage's `on_fail` decides what follows. The fold then applies that
verdict exactly as it would apply a `stage` agent's own ([Plugins](plugins.md)). Either way, the verdict
decides what follows:

| Verdict | What follows |
| --- | --- |
| `pass` | The stage instance is done. |
| `needs_user` | A `stage_review` gate with the agent's question and options. |
| `fail` with `on_fail = "retry"` | The next round, with the findings, while the round is below `max_rounds`; then a `stage_review` gate. |
| `fail` with `on_fail = "gate"` | A `stage_review` gate. |
| `fail` with `on_fail = "continue"` | The failure is recorded and the stages after it run. |
| `fail` with `on_fail = "fail"` | The session fails, naming the stage. |

A run that ended without a valid custom submit opens the gate as a failure. A run that errored opens the
`execution_failed` gate, where `retry` runs the stage again and `abandon` lets the workflow go on without it. A
run the pause interrupted resumes in the same round (Rule P2), and a run that waits for a message holds the
stage until a message wakes it.

At the `stage_review` gate ([Gates and judges](gates-and-judges.md#stage-review)) you pick `retry` with optional
guidance, `continue`, or `stop` after a failure, and an option, `other` with your answer, or `stop` for a question.
Your words become user notes in the next round's spawn. Each new round continues the agent's conversation instead
of starting cold (Rule H5), so it keeps what it read. Under YOLO a question takes its first option, and a failure
runs again while rounds are left; after the last round the gate waits for you.

### Talking to the stages around it

A stage's agent can message the subagents it works with (Rule WF6). `ListAgents` marks the author of the spec,
the author of the plan, the agent of each earlier custom stage, and the implementer of the phase or project it
checks. An audit that finds a problem can message the implementer, which continues its own conversation with
its tools to fix it, and wait for the reply before it decides its verdict ([Messages between subagents](agents.md)).

## Data between nodes

A node reads earlier results through references (Rule WB4). A reference is `<node>.<path>`, parsed by
`parse_ref` in [`crates/ostra-core/src/transform.rs`](../../crates/ostra-core/src/transform.rs): the first part
names a node, `session`, or `scope`, and each later part is an object key or an array index. A step that is not
there reads as null.

What a node's value is (`SessionState::node_value` in
[`crates/ostra-engine/src/workflow.rs`](../../crates/ostra-engine/src/workflow.rs)):

| Node | Value |
| --- | --- |
| Agent or plugin stage | Its last submit (`verdict`, `summary`, `findings`, `question`, `options`, `report_path`, `data`), with `output` set to its `data` and `scope` to its instance |
| Transform or prompt node | `{verdict: "pass", summary, data, output}`, where `output` is what it gave |
| Skipped node | null |
| `ostra:research` | `{research_docs}` |
| `ostra:track` | `{track}` |
| `ostra:spec` | `{spec_file}` |
| `ostra:stakes` | `{stakes}` |
| `ostra:plan` | `{master_plan, phases}` |
| `ostra:build` | `{phases}` |
| `ostra:feedback`, `ostra:closing` | `{}` |
| `session` | `{request, category, track, stakes, projects, title}` |
| `scope` | `{kind: "session"}`, `{kind: "project", project}`, or `{kind: "phase", phase, project}` |

Scope decides which instance a reference reads. A session node is read as its one instance. A node that runs per
project or per phase is read as its own instance from a node of the same scope kind (a project node reading
another project node gets the same project's instance), and as the list of all its instances from anywhere else.

A node may read only nodes it waits for, directly or through others, so their results exist when it runs; the
fix names the wait to add. `session` and `scope` need no wait.

`inputs` resolves each named reference (`node_inputs`). A transform gets them as its inputs. A custom agent's
spawn lists them, one `name = <json>` line each, under `Inputs` (`CustomParams.inputs`). A plugin stage's
`StageView` carries them as `inputs`, so its stage logic sees them when it decides.

## Conditions and skipping

`when` holds conditions on references (Rule WB5). Each has a `ref`, an `op`, and a `value` for the operators
that compare against one:

| Operator | True when |
| --- | --- |
| `eq`, `ne` | The value equals, or does not equal, `value` (`1` equals `1.0`) |
| `gt`, `ge`, `lt`, `le` | Both are numbers, or both are strings, and they compare that way |
| `contains` | A string holds the text, an array holds the value, or an object has the key |
| `in` | The value is one of `value`'s items, or a substring of a `value` string |
| `exists` | The value is not null |
| `empty`, `not_empty` | The value is, or is not, null, `""`, `[]`, or `{}` |
| `truthy`, `falsy` | The value is, or is not, anything but null, false, 0, `""`, `[]`, and `{}` |

A condition with an operator that takes a value and none given, or a value its operator does not take, is a
validation error. All conditions must hold, or one with `when_mode = "any"`.

The planner checks a node's conditions once the nodes it waits for are done and before it starts
(`skip_step`). Its conditions read only nodes it waits for, which are done, so the answer cannot change later.
When they do not hold, the planner emits `Step::SkipStage`, the runner appends `StageSkipped`, and the fold marks
the instance `Skipped`. A skipped node counts as done, so the nodes after it run and branches join again. A
plugin stage whose conditions do not hold is never asked to decide.

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

A transform node (`transform = "<function>"`) runs one of Ostra's typed functions, or one of the workspace's own
[composites](#your-own-transform-functions), inside the engine, with no agent (Rule WB2). A node may also call a
function a plugin runs in code, `transform = "<plugin>:<name>"` (Rule PL7): it is checked and typed like Ostra's
from the `TransformInfo` the plugin declares, and the runner calls the plugin through
`Services::plugin_transform` and records the output or error in `NodeRan`
([Plugins](plugins.md#plugin-transform-functions)). Each function declares its inputs (wired from earlier nodes), its arguments (fixed in `args`),
and its output type (`transforms()` in [`crates/ostra-core/src/transform.rs`](../../crates/ostra-core/src/transform.rs)):

| Function | Inputs | Arguments | Output |
| --- | --- | --- | --- |
| `pick` | `value` | `path` (string) | Any: the field at the dotted path |
| `count` | `items` | none | Number: items of a list, keys of an object, characters of a text; null is 0 |
| `filter` | `items` (list) | `field`, `op`, `to` | List: items whose field passes the comparison |
| `map` | `items` (list) | `field` | List: that field of each item |
| `concat` | any, in name order | none | List: list inputs joined, other inputs added as items |
| `unique` | `items` (list) | none | List: without repeats, first kept |
| `sort` | `items` (list) | `field`, `descending` (bool) | List |
| `group_count` | `items` (list) | `field` | Object: count per field value |
| `sum` | `items` (list) | `field` | Number |
| `compare` | `value` | `op`, `to` | Bool |
| `all`, `any` | any | none | Bool: every input, or one input, truthy |
| `not` | `value` | none | Bool |
| `merge` | objects, in name order | none | Object: a later key wins |
| `template` | any | `text` | String: `{{name}}` and `{{name.path}}` replaced, text as is, others as JSON |
| `constant` | none | `value` | Any |
| `findings` | node results or lists of them | none | List: their `findings`, joined |

`op` takes the condition operators above. A function with fixed inputs refuses a missing required input and an
unknown one; a function that takes any inputs needs at least one. A missing required argument, an unknown one, an
argument of the wrong JSON type, or an unknown `op` is refused at save time (`check_transform`). A transform node
takes no `instructions`.

The planner emits `Step::RunNode` with the node, its scope, and the round (`data_stage_action`). The runner
resolves the inputs, calls `run_transform`, and appends `NodeRan` with the output or the error. The output is
recorded, not recomputed, so the fold stays a function of the log even if a later Ostra computes a function
differently.

## Prompt nodes

A prompt node (`prompt = "..."`) makes one model call and gives JSON that matches its `output_schema`, which must
be an object schema (Rule WB3). It runs through the judge path, natively: the system prompt is
[`assets/judges/prompt-node.md`](../../assets/judges/prompt-node.md), the user message holds the prompt, the
inputs as JSON, and, after a failed round, what you said at the node's gate, and the answer is a forced `decide`
call whose input schema is the node's `output_schema`. The route is the judge route on the node's `tier`
(default `balanced`) at its `effort` (default `medium`). An answer that does not match the schema is asked for
once more; a second miss, or a failed call, fails the node with the reason.

Its cost is recorded in `NodeRan` (`cost_usd`) and added to the session's `node_cost`, which `spent_usd`
counts, so prompt nodes count toward the session budget. `Step::RunNode` for a prompt node has `model: true`,
and the budget guard holds it like a spawn ([Spend and limits](spend-and-limits.md)).

### How a transform or prompt node ends

A node that gave its output is done. A node that failed follows its `on_fail` like an agent's `fail` verdict:
`continue` goes on without its output, `fail` stops the session, `retry` runs it again while rounds are left, and
`gate` (or `retry` after the last round) opens a `stage_review` gate titled "Node <id> failed", with `retry`,
`continue`, and `stop`. A retry from the gate gives a prompt node your guidance.

## Checking data before it flows

A reference that reads nothing would otherwise surface only when the session runs, as a failed node. So
Ostra follows every reference through the shape of the node it names when it checks a workflow (Rule WB6,
`check_types`, which `check_runnable` calls at save time, in settings validation, and when a session resolves
the workflow).

Each node's value has a shape, written as a JSON Schema (`node_shape` in
[`crates/ostra-engine/src/workflow.rs`](../../crates/ostra-engine/src/workflow.rs)):

| Node | Shape |
| --- | --- |
| An agent stage | Its submit: `verdict`, `summary`, `question`, `report_path`, `scope` (text), `options` (a list of text), `findings` (a list of objects with `file`, `description`, `fix`), and `data` and `output`, both the agent's `data_schema`. |
| A plugin stage | The same submit, with `data` and `output` accepting anything. |
| A transform | `verdict`, `summary`, `scope`, and `output` of the function's declared output kind. `filter`, `sort`, and `unique` keep the shape of the list wired into `items`. |
| A prompt node | `output` is its `output_schema`. |
| A built-in stage | The facts it settles, as listed in [Data between nodes](#data-between-nodes). |
| `session`, `scope` | Their fields, as text, a list of text (`projects`), or a number (`phase`). |

A node that runs per project or phase, read from a node of another scope, is a list of those shapes.
`ref_shape` walks a reference's path through the shape and refuses:

- a field an object shape does not declare: ``Node `r` reads `audit.data.riks`, but `audit.data` has no field
  `riks`; it has `risk`.`` (add the field to the agent's `data_schema`, or fix the name);
- a field read on a list: the path must give an index (`audit.findings.0.file`), or the list goes into a
  transform;
- a field read on a text, a number, or true or false;
- an input of a transform wired from a value of another kind than the function takes: ``Wire a list into input
  `items` of node `f`: `n.output` is a number.``

A shape that declares nothing accepts any path: a plugin stage's data, an agent without a `data_schema`, or a
prompt schema without `properties`. Conditions are checked for their path the same way.

`POST /api/workspaces/:ws/workflows/:name/check` runs every check a save runs (`check_workflow` in
[`crates/ostra-workspace/src/builder.rs`](../../crates/ostra-workspace/src/builder.rs)) and returns the problems
without writing anything. The builder calls it half a second after each edit.

## Your own transform functions

A workspace can build a transform function from others, so a chain it uses in several workflows is written once
(Rule WB7). Each is a file in `.ostra/transforms/<name>.toml`:

```toml
# .ostra/transforms/high-files.toml
description = "The files of findings at a risk."
output = "files"                 # the step whose output the function returns

[[input]]                        # wired from a node, read as input.<name>
name = "findings"
kind = "array"

[[arg]]                          # set on the node, passed on as "$<name>"
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

An input or argument has `name`, `kind` (`any`, `bool`, `number`, `string`, `array`, or `object`), `required`
(true unless set), and `description`. The steps run in order; each runs one of Ostra's functions or another
composite. A string argument that is exactly `"$<name>"` takes the function's argument of that name. The
function's output kind is the output kind of its `output` step, followed through nested composites.

A workflow node uses it like any transform: `transform = "high-files"`, with `inputs = { findings = "..." }` and
`args = { risk = "high" }`. `function_info` in
[`crates/ostra-core/src/transform.rs`](../../crates/ostra-core/src/transform.rs) describes it with the same
`TransformInfo` as Ostra's functions, with `custom` set, so the node checks, the type check above, and the
builder's palette treat both alike.

`check_function` refuses a composite with no steps or more than 32, a name one of Ostra's functions holds, an
input or argument name that is not an identifier, a step id used twice, a step whose function does not exist or
leads back to the function itself, a step input that reads an undeclared input or a step that is not earlier, a
`"$<name>"` that names no declared argument (a declared one stands in with a value of its kind when the step's
arguments are checked), and an `output` that names no step. Calls nest at most 8 deep.

A composite cannot call a plugin's function (`<plugin>:<name>`): `check_function` refuses the step, because a
composite runs inside Ostra in one step and a plugin call is a request to another program.

`run_function` runs a composite: it puts the function's inputs under `input`, runs each step with its inputs
looked up and its `"$<name>"` arguments filled, keeps each step's result under its id, and returns the `output`
step's result. An error names the function and the step.

`WorkflowSet::load` reads `.ostra/transforms/` beside the workflow files (`load_functions`). When a workflow
resolves, `functions_used` collects every composite its transform nodes reach, directly or through other
composites, into `WorkflowDef.functions`. That map travels in `WorkflowResolved`, and the runner runs a node from
it, so a session runs the version of a function it started with and an edit of the file changes only later
sessions.

## The Workflow builder's API

The console's Workflow builder edits a workflow as a diagram: nodes, with an edge for each `after` entry
(Rule WB1). Its server side is in [`crates/ostra-workspace/src/builder.rs`](../../crates/ostra-workspace/src/builder.rs):

| Request | What it does |
| --- | --- |
| `GET /api/workspaces/:ws/builder` | The palette: built-in stages with their descriptions, the contracts each reads, and whether a workflow may leave it out; the stages running plugins serve; every transform function with its typed inputs and arguments, Ostra's first, then the workspace's composites (`custom`), then the functions of running plugins under their full names (`plugin`); the condition operators; the tiers. Agents come from `WorkspaceDetail.agents`. |
| `GET /api/workspaces/:ws/workflows/:name` | A `WorkflowDoc`: the workflow with every node written out, what it resolves to, its problems, and `plugin` when a plugin builds it. |
| `PUT /api/workspaces/:ws/workflows/:name` | Save a `WorkflowFile`, after the checks below. |
| `DELETE /api/workspaces/:ws/workflows/:name` | Delete the file, unless another workflow extends it. |
| `POST /api/workspaces/:ws/workflows/:name/check` | The problems a save of the given `WorkflowFile` would be refused for, without saving (Rule WB6). |
| `POST /api/workspaces/:ws/workflows/defaults` | Add the [default workflows](#default-workflows) the workspace has no copy of. |
| `GET /api/workspaces/:ws/transforms/:name` | A `FunctionDoc`: the composite's file, its `TransformInfo`, its problems, and the workflow nodes and composites that call it. |
| `PUT /api/workspaces/:ws/transforms/:name` | Save a `FunctionFile`, refused when it does not check or would break a workflow or composite that worked before. |
| `DELETE /api/workspaces/:ws/transforms/:name` | Delete the file, unless a workflow node or another composite calls it. |

A `WorkflowDoc`'s `file` comes from `WorkflowFile::from_def`: `extends`, `remove`, `before`, and the top-level
`[agents]` are applied, and every node carries its full `after`, so the diagram shows the whole graph and a save
writes it out. `flattened` says the file on disk used `extends` or `remove`, which a save from the builder
replaces with the written-out nodes. `builtin` says the workspace has no copy and the document is Ostra's
default; saving it writes the copy. `plugin` names the plugin that builds a `<plugin>:<name>` workflow, which the
console shows read only. `[layout]` and `default_for` are kept from the file.

A save is checked before anything is written. It is refused with every problem when the workflow does not
resolve or cannot run with the workspace's agents and plugin stages, when its `default_for` gives a category two
default workflows, or when it would break another workflow that resolved before (one that extends it, say). A
name must be lowercase kebab-case, and a save under a plugin workflow's name is refused with the instruction to
save under a name of your own, because its source is the plugin's code. The file is written as TOML through `trust::save_definitions`, which keeps an
approved workspace approved and a waiting one waiting (Rule A1).

### The builder in the console

The Workflows page (`/w/<ws>/workflows`) lists the workspace's workflows, flags Ostra's defaults the workspace has
no copy of with an action that adds them, marks each plugin workflow with its plugin and as read only (with no
delete action), and starts a new workflow from any base or any listed workflow, a plugin's included.
`/w/<ws>/workflows#<name>` opens the builder (`web/src/features/builder/WorkflowBuilder.tsx`), which draws the
graph with `@xyflow/react`:

- The palette on the left adds a node of any kind: one of Ostra's stages that the graph does not hold yet, an
  agent that returns `stage` or a plugin contract, a plugin stage, a transform, or a prompt. A new node waits for
  the selected node, if any.
- In the diagram, each edge is one `after` entry. Dragging from a node's right handle to another node's left handle
  adds one, and Delete removes the selected node or edge. A node with conditions shows them, and its incoming edges
  are animated.
- The inspector on the right edits the selected node: its id (a rename also rewrites `after` entries and references),
  the agent bound to each contract of an Ostra stage, a prompt node's prompt, tier, and output schema, the inputs
  (suggesting references to the other nodes and to `session` and `scope`), a transform's typed arguments, the
  conditions and how they combine, the scope, `on_fail`, and `max_rounds`.
- The workspace's composites are a separate palette group, "Your transforms", and the functions of running
  plugins another, "Plugin transforms".
- A plugin workflow opens read only: a banner says its plugin builds it in code, and the palette, the live check,
  and Save are off. Start a new workflow from it to change a copy.
- Half a second after each edit, the builder sends the graph to `POST .../workflows/<name>/check`. It lists the
  problems in a banner and outlines in red each node whose id a problem names.
- Save sends the whole graph and every node's position to `PUT .../workflows/<name>` and shows the problems a
  refused save lists. Positions of nodes without one come from their depth in the graph.

The Workflows page also has a Transforms panel: the workspace's composites and the running plugins' functions
with their output kind, and a name field that starts a new composite. A plugin's function is listed with its
plugin and has no editor, because the plugin runs it in code. `/w/<ws>/workflows#transform:<name>` opens the composite's editor
(`web/src/features/builder/TransformEditor.tsx`): the description, the inputs and arguments with their kind and
whether each is required, the steps in order (each picks a function, Ostra's or the workspace's but not a plugin's, wires that function's inputs with
suggestions of `input.<name>` and earlier `<step>.output`, and sets its arguments), and the output step. Save and
Delete show the problems the server refuses them for, and the editor lists who uses the function.

## Validation and approval

A workflow is checked when the workspace's settings are validated and again when a session resolves it
(`WorkflowDef::validate` and `check_runnable`). A workflow must keep:

- unique, well formed stage ids, and an `after` that names stages of the workflow, without loops;
- each built-in stage of its base pipeline, except `ostra:feedback`, `ostra:closing`, and `ostra:track` with a
  fixed `track` (Rule WF2), and no built-in stage its base lacks;
- each built-in stage waiting, directly or through other stages, for the built-in stage before it;
- at most 24 custom stages and `max_rounds` from 1 to 10;
- `agents` bindings only on built-in stages, and only for contracts the stage reads (Rule WF8);
- a phase stage unordered against the build stage, in a workflow that has one, and no stage waiting for a phase
  stage directly (Rule WF7);
- every agent and plugin stage it names present in the workspace, an `agent` stage's agent returning `stage`
  or a plugin contract, and every bound agent returning the contract it fills (`check_runnable`);
- on each node, `inputs` with valid names and references, references only to nodes it waits for, conditions
  with a `value` exactly when their operator takes one, `args` only on a transform node, `output_schema` only on
  a prompt node, a prompt and an object `output_schema` on a prompt node, and a transform's inputs and arguments
  matching its function, Ostra's or one of the workspace's composites (`StageDef::node_issues`);
- every input and condition reference reading a field the named node's shape gives, and every transform input
  wired from a value of the kind it takes (`check_types`, Rule WB6, [above](#checking-data-before-it-flows)).

Where an agent may run is decided by its contract, not by whether Ostra or the workspace defines it (Rule CA5):
a custom agent that returns `review` can fill the review contract of a built-in stage, and an `agent` stage
takes any agent that returns `stage` or a plugin contract.

A file that breaks one of these is a settings issue under `workflows.<name>`, a composite that fails
`check_function` one under `transforms.<name>`, and a plugin workflow that cannot run one under `plugins`. Workflow files and composite transform files, like custom agent
files and plugin programs, are part of the workspace content you approve (Rule A1): a folder that arrives with
them runs none until you approve it in Settings, where a composite is listed as a Transform. Agents cannot write
`.ostra/workflows/` or `.ostra/transforms/`, because both are protected paths.

## A worked example

A team wants each changed project audited before tests and docs, and a short release note in parallel.

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

A session classified IMPLEMENT records `audited`. After you accept the implementation, the planner starts an
audit run for each project and the release notes at once. The closing stages wait for both audits, not for the
notes. An audit that fails runs once more with its findings, continuing its conversation; if it fails again, the
board shows a `stage_review` gate with its findings, where you choose to run it again, go on without it, or stop.
The session completes once the closing stages, the audits, and the notes are all done.

## Where to look in the code

| What | Where |
| --- | --- |
| The file format, resolution, default workflows, validation | [`crates/ostra-core/src/workflow.rs`](../../crates/ostra-core/src/workflow.rs) |
| The default workflow files | [`assets/workflows/`](../../assets/workflows/) |
| References, conditions, transform functions, composites (`FunctionFile`, `check_function`, `run_function`, `functions_used`), plugin functions (`PluginTransforms`, `function_info_with`) | [`crates/ostra-core/src/transform.rs`](../../crates/ostra-core/src/transform.rs) |
| The builder's reads, checks, and saves of workflows and composites, restoring defaults | [`crates/ostra-workspace/src/builder.rs`](../../crates/ostra-workspace/src/builder.rs) |
| The builder and the composite editor in the console | [`web/src/features/builder/`](../../web/src/features/builder/) |
| `WorkflowResolved`, `StageSkipped`, `NodeRan`, `StageReview`, the `Stage` purpose | [`crates/ostra-core/src/event.rs`](../../crates/ostra-core/src/event.rs) |
| The walk: `workflow_flow`, `builtin_stage`, `build_done`, `phase_stages` | [`crates/ostra-engine/src/plan.rs`](../../crates/ostra-engine/src/plan.rs) |
| Stage fold, stage actions, node values, `skip_step`, `data_stage_action`, `check_runnable`, shapes and `check_types`, `agent_for`, `results_due`, `result_view`, partners | [`crates/ostra-engine/src/workflow.rs`](../../crates/ostra-engine/src/workflow.rs) |
| `Step::ResolveWorkflow`, `Step::HandleResult`, `Step::SkipStage`, `Step::RunNode`, `prompt_node` | `perform` in [`crates/ostra-engine/src/runner.rs`](../../crates/ostra-engine/src/runner.rs) |
| YOLO at a stage gate | `yolo_plan` in [`crates/ostra-engine/src/judge_input.rs`](../../crates/ostra-engine/src/judge_input.rs) |
| Fixtures (`wf*`, `wb*`, including `wb7_*` for composites, and `pl6_pl7_*` for plugin workflows and functions) | [`tests/conformance/main.rs`](../../tests/conformance/main.rs) |
