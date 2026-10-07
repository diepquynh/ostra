# Ostra workflow reference

The rules are WF1 to WF9, WB1 to WB7, and PL6 and PL7 in HANDOVER sections 10.9, 10.10, and 10.11 of the Ostra
source. The page `docs/internals/workflows.md` there describes the same behavior in more detail.

## Workflow names

| Name | Resolves to |
| --- | --- |
| `ostra:<base>` | Always the default workflow of Ostra for that base |
| `<plugin>:<name>` | A workflow that a plugin builds in code. It resolves only when that plugin runs for the workspace, and it is never the default of a category. |
| Any other name | The file `.ostra/workflows/<name>.toml`. If no such file exists, the default of Ostra with that name. |

A new workspace gets a copy of each default in `.ostra/workflows/`. A copy is a usual file, and you can edit it.
Ostra refuses a save under the name of a plugin workflow.

## Node values

A reference `<node>.<path>` reads these values:

| Node | Value |
| --- | --- |
| Agent or plugin stage | Its last submit: `verdict`, `summary`, `findings` (each with `file`, `description`, `fix`), `question`, `options`, `report_path`, `data`. Also `output`, equal to `data`, and `scope`. |
| Transform or prompt node | `{verdict: "pass", summary, data, output}`. `output` is the result of the node. |
| Skipped node | null |
| `ostra:research` | `{research_docs}` |
| `ostra:track` | `{track}` |
| `ostra:spec` | `{spec_file}` |
| `ostra:stakes` | `{stakes}` |
| `ostra:plan` | `{master_plan, phases}` |
| `ostra:build` | `{phases}` |
| `ostra:feedback` | `{}` |
| `ostra:closing` | `{docs}`. From a project instance: `true` when the user selected docs for the project. From a different scope: the list of the projects with docs. |
| `ostra:book` | `{book}`: the ID of the book that the session wrote, or null |
| `session` | `{request, category, track, stakes, projects, title}` |
| `scope` | `{kind: "session"}`, `{kind: "project", project}`, or `{kind: "phase", phase, project, projects}` |

The scope decides which instance a reference reads:

- A reference to a session node reads its one instance.
- A node reads a project node or a phase node of the same scope kind as its own instance.
- A node of a different scope reads a project node or a phase node as a list of all its instances.

`session` and `scope` need no wait. Each other reference needs a wait on the node that it reads, directly or
through other nodes.

## Shapes for the type check

Ostra checks each reference and each transform input against these shapes at save time:

| Node | Shape |
| --- | --- |
| Agent stage | The submit fields. `data` and `output` have the `data_schema` of the agent. |
| Plugin stage | The submit fields. `data` and `output` accept all values. |
| Transform | `verdict`, `summary`, `scope`, and `output` of the output kind of the function. `filter`, `sort`, and `unique` keep the shape of their `items`. |
| Prompt node | `output` has the `output_schema`. |
| Built-in stage | The fields in [Node values](#node-values). |

The check refuses a field that an object shape does not declare, a field on a list without an index, a field on
a text, number, or boolean, and a transform input of a kind that the function does not take. A shape that
declares nothing accepts all paths. An example is an agent without a `data_schema`.

## Condition operators

| Operator | Takes `value` | True when |
| --- | --- | --- |
| `eq`, `ne` | Yes | The value is equal, or not equal, to `value`. `1` equals `1.0`. |
| `gt`, `ge`, `lt`, `le` | Yes | Both are numbers or both are strings, and the comparison is true. |
| `contains` | Yes | A string contains the text, an array contains the value, or an object has the key. |
| `in` | Yes | The value is an item of the `value` list, or a substring of a `value` string. |
| `exists` | No | The value is not null. |
| `empty`, `not_empty` | No | The value is, or is not, null, `""`, `[]`, or `{}`. |
| `truthy`, `falsy` | No | The value is, or is not, other than null, false, 0, `""`, `[]`, and `{}`. |

Ostra checks the conditions after the nodes that the node waits for are done. If the conditions are false, the
node is skipped, and a plugin stage is not asked.

## Transform functions

| Function | Inputs | Arguments | Output |
| --- | --- | --- | --- |
| `pick` | `value` | `path` (string) | Any: the field at the dotted path |
| `count` | `items` | None | Number: the items of a list, the keys of an object, or the characters of a text. Null is 0. |
| `filter` | `items` (list) | `field`, `op`, `to` | List: the items whose field passes the comparison |
| `map` | `items` (list) | `field` | List: that field of each item |
| `concat` | Any, in name order | None | List: the list inputs joined, and the other inputs added as items |
| `unique` | `items` (list) | None | List with no repeats. The first item stays. |
| `sort` | `items` (list) | `field`, `descending` (bool) | List |
| `group_count` | `items` (list) | `field` | Object: the count for each field value |
| `sum` | `items` (list) | `field` | Number |
| `compare` | `value` | `op`, `to` | Bool |
| `all`, `any` | Any | None | Bool: all inputs truthy, or one input truthy |
| `not` | `value` | None | Bool |
| `merge` | Objects, in name order | None | Object. A later key has priority. |
| `template` | Any | `text` | String. `{{name}}` and `{{name.path}}` become the input. Text stays as it is, and other values become JSON. |
| `constant` | None | `value` | Any |
| `findings` | Node results or lists of them | None | List: their `findings`, joined |

`op` takes the condition operators. A function with fixed inputs refuses a missing required input and an unknown
input. A function that takes any inputs needs at least one input. At save time, Ostra refuses a missing required
argument, an unknown argument, an argument of the wrong JSON type, and an unknown `op`.

If a transform fails, the `on_fail` of the node applies, the same as a `fail` verdict.

## Prompt nodes

A prompt node runs natively through the judge route on its `tier` (default `balanced`) at its `effort` (default
`medium`). The model gets the prompt and the inputs as JSON, and answers with a forced call whose schema is the
`output_schema`. If the answer does not match, Ostra asks one more time. A second mismatch fails the node. The
cost counts toward `session_budget_usd`.

## Composite transforms

A composite is a function that the workspace builds from other functions, in
`.ostra/transforms/<name>.toml`. A node calls it as `transform = "<name>"`.

```toml
# .ostra/transforms/high-files.toml
description = "The files of the items at one risk. Each item has `risk` and `file`."
output = "files"                 # the step whose output the function returns

[[input]]                        # from a node, read in steps as input.<name>
name = "items"
kind = "array"

[[arg]]                          # set in the args of the node, given to steps as "$<name>"
name = "risk"
kind = "string"

[[step]]
id = "high"
transform = "filter"
inputs = { items = "input.items" }
args = { field = "risk", op = "eq", to = "$risk" }

[[step]]
id = "files"
transform = "map"
inputs = { items = "high.output" }   # an earlier step, read as <step>.output
args = { field = "file" }
```

An input or an argument has `name`, `kind` (`any`, `bool`, `number`, `string`, `array`, `object`), `required`
(default `true`), and `description`. A string argument that is exactly `"$<name>"` takes the argument of the
function.

Ostra refuses a composite with one of these errors:

- No steps, or more than 32 steps.
- The name of a function of Ostra.
- An input name or an argument name that is not an identifier.
- A step ID that occurs two times.
- A step whose function does not exist, calls the composite again, or is a plugin function.
- A step input that reads an undeclared input or a step that is not earlier.
- A `"$<name>"` that names no declared argument.
- An `output` that names no step.

Calls nest to a depth of at most 8. A session runs the version of a composite that it started with.

## Validation

Ostra checks a workflow at save time, in settings validation, and when a session resolves it. A workflow must
keep each of these conditions:

- The stage IDs are unique and well formed. Each `after` names a stage of the workflow, and the graph has no
  loop.
- Each built-in stage of the base is present, except `ostra:feedback`, `ostra:closing`, `ostra:book`, and
  `ostra:track` with a fixed `track`. No built-in stage is present that the base does not have.
- Each built-in stage waits for the built-in stage before it, directly or through other stages.
- The workflow has at most 24 custom stages, and each `max_rounds` is from 1 to 10.
- `agents` bindings are only on built-in stages, and only for contracts that the stage reads.
- A phase stage has no order against the build stage, and no stage waits for a phase stage directly.
- Each named agent and plugin stage exists in the workspace. The agent of an `agent` stage returns `stage` or a
  plugin contract. Each bound agent returns the contract that it fills.
- `inputs` have valid names and references, and each reference names a node that the node waits for.
- A condition has a `value` only when its operator compares.
- `args` occur only on a transform node, and `output_schema` only on a prompt node.
- A prompt node has a prompt and an object `output_schema`.
- The inputs and arguments of a transform match its function.
- Each reference reads a field that the shape of the named node gives (the type check).
- `default_for` gives no category two default workflows.
- The save does not break a different workflow that resolved before, for example a workflow that extends it.

A session records its resolved workflow when it starts. A later edit of the file changes only later sessions.

## How a custom stage ends

| Result | What follows |
| --- | --- |
| `pass` | The stage instance is done. |
| `needs_user` | A `stage_review` gate with the question and options of the agent. |
| `fail` with `on_fail = "retry"` | The next round, with the findings, when rounds are left. Then a `stage_review` gate. |
| `fail` with `on_fail = "gate"` | A `stage_review` gate. |
| `fail` with `on_fail = "continue"` | Ostra records the failure, and the stages after it run. |
| `fail` with `on_fail = "fail"` | The session fails. |
| A run error | The `execution_failed` gate: `retry` or `abandon`. |

At a `stage_review` gate after a failure, the user selects `retry` with optional guidance, `continue`, or `stop`.
A new round continues the conversation of the agent, so the agent keeps what it read.
