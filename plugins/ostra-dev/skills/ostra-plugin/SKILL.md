---
name: ostra-plugin
description: Write an Ostra plugin in Rust with the `ostra-sdk` crate. A plugin adds programmatic agents (agents whose work is code), workflow stage logic, result contracts with handlers, workflows built in code, and transform functions. Use when the user asks for an Ostra plugin, a programmatic agent, a `plugin = "<plugin>:<stage>"` stage, a `<plugin>:<contract>` contract, or a `<plugin>:<name>` transform, or asks to register a plugin with `ostra plugin add` or `[[plugins]]`. For an agent that is only a prompt, use the ostra-agent skill. For a workflow file, use the ostra-workflow skill.
---

# Write an Ostra plugin

An Ostra plugin is Rust code that implements the `Plugin` trait of the `ostra-sdk` crate. Use a plugin only for
the parts that need code. A Markdown agent (the ostra-agent skill) and a workflow file (the ostra-workflow skill)
need no compiler and no approval of a program.

`reference.md` in this folder holds the full API: the trait, the types, the helpers, the stdio protocol, and the
limits. `template/` holds a plugin crate that compiles and passes its tests. Start from the template.

## Select the parts

Read the request, then select each part that it needs:

| The plugin must | Part | Method that runs it |
| --- | --- | --- |
| Do the work of an agent in code: run commands, read files, call a model, send messages | A programmatic agent: a `PluginAgent` without a prompt | `run_agent` |
| Give a model agent with the plugin, in place of a file in `.ostra/agents/` | A `PluginAgent` with `.prompt(...)` | None. Ostra runs it on a model |
| Decide in code which agent a workflow stage runs next, and when the stage passes or fails | A `PluginStage` | `decide_stage` |
| Return a result shape of its own, and change each result into a verdict | A `PluginContractDef` and the `returns` of its agents | `handle_result` |
| Give a full workflow that a session can name | A `PluginWorkflow`, built with `workflow::Workflow` | None. Ostra runs it |
| Change data between workflow nodes with code | A `TransformInfo`, built with `workflow::TransformFn` | `transform` |

Only `manifest` is necessary. Each other method returns an error by default, so implement only the methods of
the selected parts.

## Select how Ostra runs it

| Mode | How | When to use it |
| --- | --- | --- |
| A program (use this mode by default) | `main` calls `ostra_sdk::stdio::serve(Arc::new(MyPlugin)).await`. A workspace lists the program under `[[plugins]]`. | For most plugins. It needs only `ostra-sdk`, and each workspace selects its plugins. |
| Built into the binary | A binary calls `ostra_server::cli::main_with(Registry::new().with(Arc::new(MyPlugin)))`. | The user builds their own `ostra` binary. This mode needs the full Ostra source and the `web/dist` build, and the plugin serves all workspaces with no approval. |

## Steps

1. If the request does not name the plugin, ask the user for a name. The name is lowercase kebab-case, starts
   with a letter, has at most 40 characters, and is not `ostra`.
2. Run `ostra --version` to get the Ostra release. If the command is not found, ask the user for the release.
3. Copy `template/` into a new folder. Replace `PLUGIN_NAME` in `Cargo.toml` and `src/main.rs` with the plugin
   name. Replace `OSTRA_TAG` with `v<release>`, for example `v0.2.1`. The SDK and the server must come from the
   same release, because the stdio types must match.
4. Remove the parts of the template that the plugin does not need. Write the selected parts. Follow
   [Rules for each part](#rules-for-each-part).
5. Write a unit test for each decision rule of `decide_stage`, each branch of `handle_result`, and each
   transform. Build a `StageView` from JSON, as the template does. Use `MemoryCheckpoints` for checkpoints.
6. Run `cargo test`, then `cargo build --release`. The crate needs Rust 1.99 or later and edition 2024.
7. Register the program. Give the user this command to run from the workspace folder:
   `ostra plugin add <name> -- <absolute path to target/release/<crate binary>>`. Add `--env KEY=VALUE` for each
   environment variable and `--timeout <seconds>` (1 to 3600, default 120) if a decision or a transform can take
   longer than 120 seconds.
8. Tell the user to approve the workspace file in the Settings of the Ostra console. Ostra starts no plugin
   program until the user approves the file, and an edit outside Ostra makes the file wait for approval again.
9. Use the plugin. A session names a plugin workflow as `<plugin>:<name>`. A workflow file names a stage as
   `plugin = "<plugin>:<stage>"` and a transform as `transform = "<plugin>:<name>"` (the ostra-workflow skill).
10. If the plugin does not appear, tell the user to read the issue under `plugins[<i>]` in Settings. The issue
    shows the error or the end of the stderr of the program.

To get the source of the SDK examples, run `cargo metadata --format-version 1` in the plugin crate and read the
`manifest_path` of the `ostra-sdk` package. The examples are in `examples/` next to that file:
`release_gate.rs` (a stage, a programmatic agent, messages, a checkpoint, a transform, a workflow),
`typed_review.rs` (typed contracts and handlers), and `web_dev/` (a full plugin with a model agent, a programmatic
agent, a contract, and tests).

## Rules for each part

### All parts

- Write logs to stderr only (`eprintln!` or a `tracing` subscriber on stderr). Stdout carries the protocol, and
  one stray line on stdout breaks it.
- Make `manifest().name` equal to the `name` of the `[[plugins]]` entry. Ostra refuses a program whose manifest
  has a different name.
- Return `Err(String)` with a message that tells the reader what to fix. Ostra records the message as a `fail`
  decision, a `fail` result, or a failed node.
- Write each name (agent, stage, contract, workflow, transform) in lowercase kebab-case. Inside Ostra, the
  stage, contract, workflow, and transform names get the prefix `<plugin>:`. Agent names get no prefix, so
  select agent names that do not collide with the agents of the workspace.

### Programmatic agents (Rule PL2)

- Wrap the calls in `AgentContext::new(calls)`. Use its helpers: `read`, `write`, `bash`, `grep`, `list_agents`,
  `send_message`, `start_helper`, `wait_for_message`, `complete`, and `status`. Use `tool(name, input)` for any
  other tool, with the Claude Code tool name and input shape.
- Give the agent each capability whose tool it calls. A tool outside its capabilities returns an error.
- Read `ToolReply.is_error` after each call. A policy denial or a sandbox denial is an error reply, not a panic.
- Return `pass(summary)` or `fail(summary, findings)` for the `stage` contract. For a built-in contract or a
  contract of the plugin, use `contracts::ContractAgent` and `TypedAgents`, because they check the result the same
  way as the engine.
- Keep a run under 2,000 tool calls and model calls in total.
- If the run sends a message with `wait: true`, save a checkpoint before the call. See
  [Checkpoints](#checkpoints-rule-pl8).

### Stage logic (Rule PL3)

- Decide only from the `StageView`: `runs`, `decisions`, `answers`, `inputs`, and `instructions`. Ostra records
  each decision as an event and does not ask again after a restart, so keep no state in the plugin process.
- Return one of `Run { agent, instructions }`, `Pass { summary }`, `Fail { summary }`, or
  `Ask { question, options }`. Put the recommended option first.
- Put a limit on the runs. Count `view.runs` and return `Ask` or `Fail` at the limit. The `max_rounds` of the node
  (1 to 10, default 3) also stops the stage.
- Read the verdict of a run of a plugin contract from `contracts::handled(run)`, not from `run.submit`.

### Result contracts (Rule PL5)

- Implement `contracts::ContractType` for a struct, with `contract()` that parses `"<plugin>:<name>"` and
  `schema()` that returns the JSON Schema. Add `contract_def::<C>(description)` to `manifest().contracts`.
- Use only the schema keywords that Ostra checks: `type`, `properties`, `required`, `items`, `enum`, `minItems`,
  `maxItems`, and `additionalProperties: false`.
- Set the verdict in `handle_result` from the data. Do not copy a verdict that the agent wrote.
- An agent that returns a contract of a plugin that is not running is left out of the catalog.

### Workflows and transforms (Rules PL6 and PL7)

- Build a workflow with `Workflow::extending(name, "ostra:<base>")` or `Workflow::new(name, base)`. The same
  validation applies as for a workflow file (the ostra-workflow skill).
- Declare each transform with `TransformFn`: the inputs, the arguments, and the output kind. Ostra checks the
  wiring of each node against this declaration at save time.
- Make a transform a pure function of its inputs and arguments. Ostra records its output, and a transform gets
  no checkpoints.

### Checkpoints (Rule PL8)

A plugin program can stop at any time. Ostra starts it again and repeats the call, at most 2 times. A checkpoint is
one key and one JSON value that Ostra keeps in the session log.

- Save a checkpoint after each step that has an effect outside the program, and before the next step.
- Read the checkpoint first when a call starts. If `task.resumed` is `true`, the run continues an earlier start.
- Key a checkpoint by the work that it describes, for example the stage node, not only by the execution ID.
- Keep a value under 64 KiB of JSON. Write large data to a file in `task.session_dir` and save its path.

```rust
let key = format!("asked:{}", task.execution);
if checkpoints.get(&key).is_none() {
    checkpoints.save(&key, json!(implementer)).await?;
    ctx.send_message(&implementer, "Fix the changelog, then reply to me.", true).await;
}
```

## Check your work

Before you finish, make sure that each item is true:

- `cargo test` and `cargo build --release` pass.
- Nothing in the program writes to stdout except `serve`.
- `manifest().name` equals the name that you gave the user for `ostra plugin add`.
- Each agent has the capabilities of each tool that it calls, and the narrowest `write_scope` that works.
- Each `Run` decision names an agent that exists in the manifest or in the workspace.
- `decide_stage` has a limit on runs, and each `Ask` has its recommended option first.
- Each call that has an effect outside the program saves a checkpoint first, and a repeated call is safe.
- You told the user to approve the workspace file in Settings.
