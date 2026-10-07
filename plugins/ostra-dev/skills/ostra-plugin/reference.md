# Ostra plugin API reference

This file lists the API of `ostra-sdk`. The SDK exports the types of `ostra_core::plugin` again, so
`use ostra_sdk::*;` imports all of them. The rules (PL1 to PL8) are in HANDOVER section 10.10 of the Ostra source.

## The `Plugin` trait

```rust
#[async_trait::async_trait]
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> PluginManifest;
    fn is_alive(&self) -> bool;                                  // default: true
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

`stage`, `contract`, and `name` are the names inside the plugin, without the `<plugin>:` prefix.

## The manifest

```rust
pub struct PluginManifest {
    pub name: String,                       // lowercase kebab-case, at most 40 characters, not "ostra"
    pub version: String,
    pub description: String,
    pub agents: Vec<PluginAgent>,
    pub stages: Vec<PluginStage>,           // { name, description }
    pub contracts: Vec<PluginContractDef>,  // { name, description, schema }
    pub workflows: Vec<PluginWorkflow>,     // { name, workflow: WorkflowFile }
    pub transforms: Vec<TransformInfo>,
}
```

`PluginManifest` implements `Default`, so write `..Default::default()` for the parts that the plugin does not
offer.

## Agents

`PluginAgent::new(name, description)` and these builder methods:

| Method | Value | Default |
| --- | --- | --- |
| `prompt(text)` | The system prompt. Without a prompt, the agent is programmatic and `run_agent` runs it. | None |
| `tier(Tier)` | `Fast`, `Balanced`, `Advanced`, or `Frontier` | `Balanced` |
| `capabilities([Capability])` | See the capability table of the ostra-agent skill. Rust names are in CamelCase, for example `Capability::SearchText`. | `Read`, `SearchText`, `Glob`, `Report`, `Coordinate` |
| `write_scope(WriteScope)` | `ReadOnly`, `Session`, `Project`, or `Setup` | `Project` with `Write` or `Edit`, else `Session` |
| `effort(executor, Effort)` | Executor `native`, `claude`, `codex`, `grok`, or `agy`. Effort `Low`, `Medium`, `High`, `Xhigh`, or `Max`. | `High` |
| `timeout_seconds(n)` | At most 7200 | 1200 |
| `data_schema(json)` | The schema of `data` in a `stage` submit | None |
| `helper(bool)` | `SendMessage` can start the agent as a helper | `false` |
| `returns(contract)` | `stage`, a built-in contract, or `<plugin>:<name>` | `stage` |
| `brief([section])` | `stack`, `commands`, `testing`, `skills`, `conventions`, `review`, `modules` | All but `testing` and `review` |

A prompt uses the same template tokens as a Markdown agent (the ostra-agent skill). To read agents from files
in the plugin crate, use `definition::parse_markdown(name, text)` for a Markdown file with `+++` frontmatter, or
`definition::parse_toml(name, agent_toml, prompt)`.

## Programmatic runs

`run_agent` gets an `AgentTask`:

| Field | Meaning |
| --- | --- |
| `execution` | The execution ID |
| `session` | The session ID, or `None` outside a session |
| `agent` | The agent name |
| `first_message` | The spawn block, the repo brief, and the instructions of the node and of the stage logic |
| `repo_root`, `session_dir`, `workspace_root` | Paths. Write reports and large data to `session_dir`. |
| `model` | The model of the route of the agent, which `complete` uses |
| `tools` | The tool names that the agent can call |
| `submit_schema` | The schema that the result must match |
| `resumed` | `true` when the run continues after the program stopped or the session paused |

`AgentContext::new(calls)` gives these helpers. Each tool call goes through the policy, the permission ask, and
the sandbox of Ostra, and the console shows it in the Activity view of the run.

| Helper | Tool call |
| --- | --- |
| `read(path)` | `Read { file_path }` |
| `write(path, content)` | `Write { file_path, content }` |
| `bash(command)` | `Bash { command }`. Ostra keeps at most 30,000 characters of the output. |
| `grep(pattern, path)` | `Grep { pattern, path }` |
| `list_agents()` | `ListAgents`: the subagents of the session and the helpers that the agent can start |
| `send_message(to, message, wait)` | `SendMessage { to, message, wait }`. With `wait`, it returns the reply. |
| `start_helper(agent, message, wait)` | `SendMessage { agent, message, wait }` |
| `wait_for_message()` | `WaitForMessage` |
| `tool(name, input)` | Any other tool, by its Claude Code name and input shape |
| `complete(system, user)` | One model call on the route of the agent. Returns `Result<String, String>`. |
| `complete_with(CompleteRequest)` | One model call with several messages and `max_tokens` |
| `status(text)` | One line in the Activity view |

Each tool call returns `ToolReply { output: String, is_error: bool }`.

The result is `AgentOutcome { submit: Value }`. Ostra checks it against `submit_schema`. For the `stage`
contract, `pass(summary)`, `fail(summary, findings)`, and `outcome(CustomSubmit)` make it. A run must reply to
each sender that waits for it. If Ostra gave the run a report file, the run must write it. The `review` contract
is the exception.

## Stage logic

`decide_stage` gets a `StageView`:

| Field | Meaning |
| --- | --- |
| `session`, `node`, `stage`, `scope` | Where the stage runs. `scope` is `None`, `project:<key>`, or `phase:<n>`. |
| `request` | The request of the session |
| `workspace_root`, `projects` | The workspace path, and `(key, path)` for each project in scope |
| `instructions` | The `instructions` of the workflow node |
| `inputs` | The `inputs` of the node, resolved from earlier nodes |
| `decisions` | The earlier decisions of this stage instance, oldest first |
| `runs` | The runs that those decisions started, oldest first: `StageRunView { execution, agent, status, submit, error, handled }` |
| `answers` | The answers of the user at the gates of this stage, oldest first |
| `earlier_stages` | One line for each earlier custom stage: ID, verdict, summary |
| `spec_file`, `master_plan` | Paths, when they exist |

It returns a `StageDecision`. On the wire, it is a JSON object with a `kind` field:

| Decision | JSON | What follows |
| --- | --- | --- |
| `Run { agent, instructions }` | `{"kind":"run","agent":"...","instructions":"..."}` | The agent runs as this stage. A run past `max_rounds` counts as a failure. |
| `Pass { summary }` | `{"kind":"pass","summary":"..."}` | The stage instance is done. |
| `Fail { summary }` | `{"kind":"fail","summary":"..."}` | The `on_fail` of the node applies. |
| `Ask { question, options }` | `{"kind":"ask","question":"...","options":["..."]}` | A `stage_review` gate asks the user. The answer is in `answers` at the next call. |

Ostra calls `decide_stage` again after each run of the stage ends and after each answer at its gate. If the
`when` conditions of the node are false, Ostra skips the stage and does not call the plugin.

## Results of a plugin contract

`handle_result` gets a `ResultView { session, execution, agent, contract, stage, scope, submit }` and returns a
`CustomSubmit`:

| Field | Type | Meaning |
| --- | --- | --- |
| `verdict` | `StageVerdict::{Pass, Fail, NeedsUser}` | The verdict of the run |
| `summary` | `String` | Must not be empty |
| `findings` | `Vec<CustomFinding { description, file, fix }>` | Each problem |
| `question`, `options` | `Option<String>`, `Vec<String>` | Necessary with `NeedsUser` |
| `report_path` | `Option<String>` | A report that the run wrote |
| `data` | `Option<Value>` | Output in the shape of `data_schema` |

Ostra records the outcome as an event. A custom stage uses it as the verdict of the run. A plugin stage sees it
in `StageRunView.handled`. An agent that started the run as a helper gets it as the result message.

## Typed contracts (`ostra_sdk::contracts`)

| Item | Use |
| --- | --- |
| `ContractType` | Ties a contract to its struct: `contract()`, `schema()`, `parse(&Value)`, `to_submit(&Submit, &schema)` |
| Built-in types | `Research`, `Spec`, `FactCheck`, `Plan`, `Implementation`, `Review`, `PathAnalysis`, `Tests`, `Prompt`, `Advice`, `Answer`, `Setup`, `Stage`. `documentation` has no type. |
| `contract_def::<C>(description)` | The `PluginContractDef` for the manifest |
| `PluginAgentExt::returns_type::<C>()` | Sets `returns` on a `PluginAgent` |
| `ContractAgent` | A programmatic agent whose `run` returns the struct of its contract |
| `TypedAgents::new().with(agent)` | `definitions()` for the manifest, `run(name, task, calls, checkpoints)` from `run_agent` |
| `SubmitExt::submit_as::<C>()` | Reads a `StageRunView` or a `ResultView` as the struct of `C` |
| `StageViewExt::submits_of::<C>()`, `last_submit_of::<C>()` | Reads the runs of a stage as structs |
| `handled(run)` | The `CustomSubmit` that the handler gave a run |
| `ResultHandler`, `handle_result(handler, result, checkpoints)` | A typed handler for a plugin contract |

A plugin contract type:

```rust
#[derive(Serialize, Deserialize)]
pub struct Note { pub text: String }

pub struct ReleaseNote;

impl ContractType for ReleaseNote {
    type Submit = Note;
    fn contract() -> Contract {
        "my-plugin:release-note".parse().expect("a valid contract name")
    }
    fn schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["text"],
            "properties": {"text": {"type": "string", "description": "The release note."}}
        })
    }
}
```

An agent that returns a built-in contract fills the same contract as the agent of Ostra. A workflow can bind it
to a built-in stage with `[agents]` (Rule WF8). For example, a `ContractAgent` with `type Contract = Review` can
replace `code-reviewer` in the build stage.

## Workflow and transform builders (`ostra_sdk::workflow`)

| Builder | Methods |
| --- | --- |
| `Workflow::new(name, base)`, `Workflow::extending(name, parent)` | `description`, `remove(id)`, `track(Track)`, `bind(contract, agent)`, `node(Node)`, `build()` |
| `Node::stage(id, stage)`, `Node::agent(id, agent)`, `Node::plugin_stage(id, "<plugin>:<stage>")`, `Node::transform(id, function)`, `Node::prompt(id, prompt, output_schema)` | `after([ids])`, `before([ids])`, `input(name, reference)`, `arg(name, value)`, `when(reference, CondOp, Option<Value>)`, `instructions(text)`, `scope(StageScope)`, `on_fail(OnFail, max_rounds)`, `bind(contract, agent)` |
| `TransformFn::new(name, description)` | `input`, `optional_input`, `arg`, `optional_arg` (each with a name, a `ValueKind`, and a description), `output(ValueKind)`, `build()` |

`ValueKind` is `Any`, `Bool`, `Number`, `String`, `Array`, or `Object`. `StageScope` is `Session`, `Project`, or
`Phase`. `OnFail` is `Gate`, `Retry`, `Continue`, or `Fail`. `CondOp` has one variant for each condition
operator of the ostra-workflow skill, for example `CondOp::NotEmpty`. The SDK does not export `Track`
(`Light` or `Full`). To call `track`, add `ostra-core` from the same git tag as `ostra-sdk` and use
`ostra_core::pipeline::Track`.

## Checkpoints

```rust
checkpoints.get(key) -> Option<Value>
checkpoints.all() -> BTreeMap<String, Value>
checkpoints.save(key, value).await -> Result<(), String>   // returns after the save is in the session log
checkpoints.remove(key).await -> Result<(), String>
```

| Limit | Value |
| --- | --- |
| Keys for each plugin and session | 256 |
| Bytes for each value, as JSON | 64 KiB |
| Saves for each plugin and session | 5,000 |
| Key | 1 to 200 characters, no control characters |

A save of the same value does not count. Over stdio, a call sees the checkpoints from the time that Ostra sent it,
and its own saves. Two calls at the same time do not see the saves of each other. `MemoryCheckpoints::default()`
keeps checkpoints in memory with the same limits, for tests. A call outside a session gets `NoCheckpoints`.

## The stdio protocol

The program and Ostra send newline-delimited JSON-RPC 2.0 in both directions. `ostra_sdk::stdio::serve`
implements it. Use the SDK. Write the protocol by hand only for a plugin in a different language.

| Direction | Method | Answer |
| --- | --- | --- |
| Ostra to plugin | `initialize {protocol}` | The `PluginManifest` |
| Ostra to plugin | `stage/decide {stage, view, call, checkpoints}` | A `StageDecision`, in `timeout_secs` |
| Ostra to plugin | `agent/run {agent, task, call, checkpoints}` | An `AgentOutcome`, in the timeout of the execution |
| Ostra to plugin | `agent/cancel {execution}` (notification) | None |
| Ostra to plugin | `result/handle {contract, result, call, checkpoints}` | A `CustomSubmit`, in `timeout_secs` |
| Ostra to plugin | `transform/run {name, inputs, args}` | The output, in `timeout_secs` |
| Plugin to Ostra | `host/tool {execution, name, input}` | A `ToolReply` |
| Plugin to Ostra | `host/complete {execution, request}` | `{text}` |
| Plugin to Ostra | `host/status {execution, text}` (notification) | None |
| Plugin to Ostra | `host/checkpoint {call, key, value}` | `{}`. A `null` value removes the key. |

The protocol version is 1. Each request runs on its own task. The transport skips a line longer than 16 MiB and a
line that is not JSON.

## Registration

```toml
# <workspace>/.ostra/workspace.toml
[[plugins]]
name = "my-plugin"                       # equal to manifest().name
command = ["/abs/path/to/my-plugin"]     # the program, then its arguments
env = { MY_SETTING = "value" }           # optional
enabled = true                           # optional, default true
timeout_secs = 120                       # optional, 1 to 3600
```

`ostra plugin add <name> [--env KEY=VALUE] [--timeout N] [--disabled] [--workspace DIR] -- <program> [args]`
writes the same entry. The program starts in the workspace folder, so a relative path resolves against that
folder. `GET /api/workspaces/:ws/plugins` shows the state of each plugin: `running`, `starting`, `disabled`,
`waiting_approval`, or `failed` with its error.
