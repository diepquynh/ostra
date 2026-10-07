---
name: ostra-workflow
description: Write an Ostra workflow file, `<workspace>/.ostra/workflows/<name>.toml`, or a composite transform function, `<workspace>/.ostra/transforms/<name>.toml`. A workflow is the pipeline as a graph of stages, with custom agent stages, plugin stages, transform nodes, prompt nodes, data references between nodes, and conditions that skip nodes. Use when the user asks to add a stage to the Ostra pipeline, change the order of stages, branch on a result, replace the agent behind a built-in stage, or make a workflow the default of a category. For the agent that a stage runs, use the ostra-agent skill. For a workflow built in Rust code, use the ostra-plugin skill.
---

# Write an Ostra workflow

An Ostra workflow is a TOML file that lists stages and the order between them. The built-in stages of Ostra
(research, spec, plan, build, and the others) are nodes. A workflow adds its own nodes between them. Each node can
read the results of earlier nodes and can have conditions that skip it.

`reference.md` in this folder holds the value of each node kind, the condition operators, the transform
functions, composite transforms, and the full validation list.

## Steps

1. Find the workspace folder. It holds `.ostra/workspace.toml`. Read the files in `.ostra/workflows/`,
   `.ostra/agents/`, and `.ostra/transforms/`, because a new workflow can extend a workflow there and use the
   agents there.
2. Select the base pipeline. Use [Base pipelines](#base-pipelines). To add stages to a pipeline, extend its
   default: `extends = "ostra:<base>"`.
3. Make sure that each agent that a node names exists. If an agent does not exist, write it first with the
   ostra-agent skill. An `agent` node takes an agent that returns `stage` or `<plugin>:<contract>`.
4. Write `.ostra/workflows/<name>.toml`. The name is lowercase kebab-case, at most 48 characters. Use
   [The file](#the-file) and [Nodes](#nodes).
5. Wire data with `inputs` and conditions with `when`. Read only nodes that the node waits for, directly or
   through other nodes. Use [Data and conditions](#data-and-conditions).
6. Check the file against [Check your work](#check-your-work) and the validation list in `reference.md`.
7. Tell the user to approve the workspace file in the Settings of the Ostra console. A file that you add or
   change outside Ostra makes the workspace file wait for approval again.
8. Tell the user to read the Settings issues under `workflows.<name>`, or to open the workflow in the Workflow
   builder (`/w/<ws>/workflows#<name>`), which lists each problem and outlines each node with a problem.
9. Tell the user how to run it: select the workflow in the Workflow menu of the New task form. With
   `default_for`, a session of that category runs it when the user selects no workflow.

## Base pipelines

| Base | Built-in stages, in order |
| --- | --- |
| `research` | `research` |
| `spec` | `research`, `spec` |
| `plan` | `research`, `spec`, `plan` |
| `implement` | `research`, `track`, `spec`, `stakes`, `plan`, `build`, `feedback`, `closing`, `book` |
| `verify`, `prompt`, `quick-change` | `build` |
| `test`, `docs` | `closing`, `book` |

The default workflows give the node of `ostra:book` the ID `docs`. Each other built-in node has the stage name as
its ID, for example `build`. A workflow must keep each built-in stage of its base, in this order. The exceptions
are `feedback`, `closing`, and `book`, which a workflow can remove, and `track`, which a fixed `track` can
replace.

## The file

```toml
# <workspace>/.ostra/workflows/secure.toml
description = "The implement pipeline with a security audit before the closing stages."
extends = "ostra:implement"
default_for = ["implement"]

[[stage]]
id = "audit"
agent = "security-auditor"
after = ["build"]
before = ["closing"]
scope = "project"
on_fail = "retry"
max_rounds = 3
instructions = "Check every changed file for secrets and unsafe input handling."

[agents]
review = "strict-reviewer"
```

Put the top-level keys before the first `[[stage]]`, and put `[agents]` and `[layout]` after the last stage,
because TOML puts each key after a table header into that table.

| Top-level key | Meaning |
| --- | --- |
| `description` | The text that Ostra shows with the workflow. The default is the description of the extended workflow. |
| `extends` | The workflow to copy: a file of this workspace, `ostra:<base>` for the default of Ostra, or `<plugin>:<name>`. A chain has a depth of at most 8. A file cannot extend its own name, so `implement.toml` uses `extends = "ostra:implement"`. |
| `base` | The base pipeline. Omit it with `extends`. A workflow with the name of a category keeps that base. |
| `remove` | IDs of stages of the extended workflow to remove. A stage that waited for a removed stage then waits for the stages that the removed stage waited for. |
| `default_for` | Categories that run this workflow when the user selects none, for example `["implement"]`. Only one workflow can list a category. |
| `track` | `light` or `full`. Only with base `implement`. It replaces the Track judge. |
| `[agents]` | `contract = "agent"`. It binds an agent to a contract in each built-in stage that reads the contract. |
| `[layout]` | Node positions for the Workflow builder. The engine does not read it. |

## Nodes

Each `[[stage]]` has an `id` (lowercase kebab-case, at most 48 characters, unique) and exactly one of these keys:

| Key | Node kind | Example |
| --- | --- | --- |
| `uses` | A built-in stage | `uses = "ostra:build"` |
| `agent` | A custom agent stage | `agent = "security-auditor"` |
| `plugin` | The stage logic of a plugin | `plugin = "release-gate:release"` |
| `transform` | A transform function, with no model | `transform = "filter"` |
| `prompt` | One model call that returns JSON | `prompt = "Rate the risk of the change."` |

Other keys of a node:

| Key | Nodes | Meaning |
| --- | --- | --- |
| `after` | All | The stages that this stage waits for. Without `after`, the stage waits for the stage before it in the file. In a workflow that extends a different workflow, the first new stage waits for the last stage of the extended workflow. |
| `before` | All | The stages that wait for this stage. A stage with `before` does not become the stage that the next stage in the file waits for. |
| `scope` | Custom | `session` (default): one run. `project`: one run for each project in scope. `phase`: one run for each plan phase whose review passed, inside the build stage. |
| `on_fail` | Custom | `gate` (default): ask the user. `retry`: run again with the findings, until `max_rounds`, then ask. `continue`: record the failure and go on. `fail`: stop the session. |
| `max_rounds` | Custom | 1 to 10. Default 3. |
| `instructions` | `agent`, `plugin` | Text that the agent gets in its spawn block. |
| `inputs` | Custom | Names mapped to references, for example `{ files = "audit.data.files" }`. |
| `args` | `transform` | The fixed arguments of the function. |
| `when`, `when_mode` | Custom | Conditions that skip the node. `when_mode` is `all` (default) or `any`. |
| `output_schema` | `prompt` | An object JSON Schema that the answer must match. Necessary on a prompt node. |
| `tier`, `effort` | `prompt` | The model of the prompt node. Defaults `balanced` and `medium`. |
| `agents` | `uses` | Contract bindings for this one built-in stage, for example `agents = { spec = "my-spec-writer" }`. |

A built-in stage takes no `scope`, `inputs`, `args`, `when`, or `output_schema`. Do not make a stage wait for a
phase stage, and do not order a phase stage against the build stage.

### Replace the agent of a built-in stage

A built-in stage reads results by contract, so an agent that returns the same contract can replace the standard
agent:

| Stage | Contracts that it reads |
| --- | --- |
| `ostra:research` | `research` |
| `ostra:spec` | `spec`, `fact-check` |
| `ostra:plan` | `plan`, `fact-check` |
| `ostra:build` | `implementation`, `review`, `prompt`, `advice` |
| `ostra:closing` | `path-analysis`, `tests`, `review`, `documentation`, `implementation`, `advice` |
| `ostra:book` | `documentation`, `fact-check` |
| `ostra:track`, `ostra:stakes`, `ostra:feedback` | None |

`[agents] review = "strict-reviewer"` changes the reviewer in the build stage and in the closing stage. To change
only one stage, put `agents = { review = "strict-reviewer" }` on that stage node. The agent must return the
contract that it fills. A binding for a contract that no built-in stage of the workflow reads is an error.

## Data and conditions

A reference is `<node>.<path>`, for example `audit.data.risk` or `audit.findings.0.file`. The first part is a node
ID, `session`, or `scope`. Each later part is an object key or an array index. A missing step reads as null.

- An agent or plugin stage gives its last submit: `verdict`, `summary`, `findings`, `question`, `options`,
  `report_path`, `data`, and `output`, which equals `data`.
- A transform node or a prompt node gives its result in `output`.
- A skipped node gives null.
- `session` gives `request`, `category`, `track`, `stakes`, `projects`, and `title`.

A project or phase node, read from a node of a different scope, gives a list with one item for each instance. To
read a field from the list, use an index or send the list to a transform such as `map`.

A condition is `{ ref = "<reference>", op = "<operator>", value = <value> }`. Give `value` only to an operator
that compares: `eq`, `ne`, `gt`, `ge`, `lt`, `le`, `contains`, `in`. Give no `value` to `exists`, `empty`,
`not_empty`, `truthy`, and `falsy`. A skipped node counts as done, so two nodes with opposite conditions make a
branch, and the branches join again at the next node:

```toml
[[stage]]
id = "fix"
agent = "security-fixer"
after = ["audit"]
inputs = { findings = "audit.findings" }
when = [{ ref = "audit.findings", op = "not_empty" }]

[[stage]]
id = "notes"
agent = "release-notes"
after = ["audit"]
when = [{ ref = "audit.findings", op = "empty" }]
```

Ostra checks each reference against the shape of the node that it reads, at save time. A reference to a field of
`data` needs that field in the `data_schema` of the agent.

## Transform and prompt nodes

A transform node runs a function in the engine, with no model and no cost:

```toml
[[stage]]
id = "high"
transform = "filter"
after = ["audit"]
inputs = { items = "audit.findings" }
args = { field = "description", op = "contains", to = "secret" }
```

The functions of Ostra are `pick`, `count`, `filter`, `map`, `concat`, `unique`, `sort`, `group_count`, `sum`,
`compare`, `all`, `any`, `not`, `merge`, `template`, `constant`, and `findings`. `reference.md` gives the inputs,
arguments, and output of each. A node can also call a composite of the workspace by its file name, or a plugin
function as `<plugin>:<name>`.

A prompt node makes one model call. Its cost counts toward the session budget:

```toml
[[stage]]
id = "risk"
prompt = "Rate the risk of these findings for a public release."
after = ["audit"]
inputs = { findings = "audit.findings" }
output_schema = { type = "object", required = ["risk"], properties = { risk = { type = "string", enum = ["low", "high"] } } }
```

A transform node and a prompt node take no `instructions`.

## Check your work

Before you finish, make sure that each item is true:

- The file name and each stage ID are lowercase kebab-case, and the IDs are unique.
- The workflow keeps each built-in stage of its base, in order, unless the base allows its removal.
- Each `after` and `before` names a stage of the workflow, and the graph has no loop.
- Each node has exactly one of `uses`, `agent`, `plugin`, `transform`, and `prompt`.
- Each named agent exists and returns `stage` or a plugin contract. Each bound agent returns the contract that
  it fills.
- Each reference reads a node that the node waits for, and a field that the shape of that node declares.
- Each condition has a `value` only when its operator compares.
- `max_rounds` is from 1 to 10, and the workflow has at most 24 custom stages.
- No other workflow lists a category of `default_for`.
- You told the user to approve the workspace file in Settings.
