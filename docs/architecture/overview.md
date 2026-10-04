# Architecture overview

Ostra is one Rust binary and one browser app. The binary runs on your machine. It holds all the state, and it
runs the models. The browser shows what the binary does, and it sends your answers back to the binary. This
page tells how the binary is divided into crates and why the dependencies point in their directions. It also
tells how the browser code is organized.

## The shape of a running Ostra

```
Browser (React console)
        │  REST for reads and commands, one WebSocket for everything live
        ▼
ostra (axum server on 127.0.0.1)
  ├── engine          one per workspace: session state, planner, judges, runner
  │     ├── executors     native agent loop, or a harness CLI in a PTY
  │     ├── policy        every tool call passes here first
  │     └── store         SQLite event log per workspace
  ├── code index      per project: symbols, usages, imports, language servers
  ├── MCP gateway     the workspace's MCP servers, shared by every execution
  └── notify          Web Push to your phone or desktop
```

When you start `ostra`, the server opens the machine registry. Then it attaches each registered workspace and
gives each workspace its own engine. The engine recovers the sessions that ran when the server last stopped.
Then the engine waits. After this point, each change starts from an event:

1. You create a session.
2. The engine appends `SessionCreated`.
3. The planner reads the new state and decides the next steps.
4. The runner runs the steps.
5. Each result becomes another event.

## The crates

Ostra has fifteen crates in one Cargo workspace. Each crate has one job.

| Crate | What it owns |
| --- | --- |
| `ostra-core` | Ids, settings and route resolution, pipeline enums, submit schemas, the event types, API types, the `Executor` and `ExecutionHost` traits, every derived path |
| `ostra-sandbox` | The sandbox for agent commands and project programs: profiles, the bubblewrap and Seatbelt backends, the egress proxy, decoys, and one OS layer for each platform |
| `ostra-store` | SQLite: the workspace database, the machine registry, each project's lesson memory |
| `ostra-policy` | Guards and permissions over canonical tool calls, and the bash parser |
| `ostra-tools` | Native tool implementations: Read, Write, Edit, Bash, Grep, Glob, Skill, WebFetch, and the other native tools |
| `ostra-providers` | Streaming clients for Anthropic and OpenAI, retry, and a scripted provider for tests |
| `ostra-agents` | The embedded prompts, skills, and refs, prompt rendering for each executor, typed spawn structs, and the repo brief |
| `ostra-engine` | Event-sourced session state, the pure planner, judges, the runner, the spawn factory |
| `ostra-exec-native` | The native agent loop: a provider, the tools, and the policy in one loop |
| `ostra-exec-harness` | Harness executors: the PTY, one adapter per CLI, the hook bridge, the MCP stdio shim |
| `ostra-code` | A tokenizer, the code index of each project, a language server client, and the code providers for the Files view |
| `ostra-mcp` | The MCP client: stdio and streamable HTTP transports, and OAuth sign-in |
| `ostra-notify` | Web Push, written without OpenSSL |
| `ostra-workspace` | Workspaces: settings checks, projects, command approvals, the create and delete operations, and the runtime of an open workspace |
| `ostra-server` | The `ostra` binary: axum, auth, REST, WebSocket, the embedded web build, the CLI |

## Which way the dependencies point

```
layer 0   ostra-core                     depends on no Ostra crate

layer 1   ostra-store      → core
          ostra-policy     → core
          ostra-agents     → core
          ostra-providers  → core
          ostra-sandbox    → core
          ostra-notify     → core
          ostra-mcp        → core

layer 2   ostra-tools        → core, store, sandbox
          ostra-code         → core, sandbox
          ostra-engine       → core, store, agents, sandbox

layer 3   ostra-exec-native  → core, store, tools, policy, providers, sandbox
          ostra-exec-harness → core, tools, code, sandbox
          ostra-workspace    → core, store, engine, agents, policy, sandbox

layer 4   ostra-server       → every crate above
```

An arrow points to the crates that a crate depends on. No crate depends on a crate in a higher layer.

Three rules keep this structure:

1. `ostra-core` depends on no other Ostra crate. All other crates can use its types. Thus a type that two
   crates share is in `ostra-core`.
2. `ostra-engine` knows no executor, no provider, and no server. From Ostra, it depends only on `ostra-core`,
   `ostra-store`, `ostra-agents`, and `ostra-sandbox`. It uses `ostra-sandbox` for the project commands that it
   runs itself.
3. `ostra-server` is the only place where the real parts connect. It builds the providers, the executors, the
   MCP gateway, and the notifier. It gives them to each engine through traits.

`ostra-workspace` is in the same layer as the executors, for the same reason as the engine. It knows no
provider, executor, or server. An open workspace needs these items from the process:

- The registry.
- The global config.
- The machine facts for validation.
- The provider and harness status.
- The engine's `Services`.

These items come through the `WorkspaceHost` trait in `crates/ostra-workspace/src/host.rs`. The server
implements this trait on its shared state in `crates/ostra-server/src/app.rs`.

## Why the engine knows so little

The engine decides the next action of a session. That decision must be the same in each of these cases:

- The agent runs on the native loop with the Anthropic API.
- The agent runs in Claude Code in a terminal.
- The agent runs in a test with a scripted model.

If the engine imports an executor, a provider, or the web server, that part changes the decision. Then a
special behavior of a harness becomes a planner branch, and a test needs a real HTTP stack.

Thus the engine gets all outside data through three traits, and the server implements them.

**`Services`** (`crates/ostra-engine/src/services.rs`) holds all the requests that the engine makes to the
outside:

- `global()` and `workspace()` return the settings. The engine reads them again for each execution. Thus an
  edit in Settings applies to the next execution without a restart.
- `executor(kind)` returns the executor for `native` or `harness:<name>`.
- `factory()` returns the spawn factory.
- `judge(route, system, user, schema, effort)` makes one judge call. A judge call is one model request with a
  forced `decide` tool. The input of this tool must match the schema.
- `notify(notice)` asks for a push notification. The server decides if push is on.
- `protected_paths()` lists the paths that no agent can touch: Ostra's own binary, config, and databases.
- `command_approved()` and `add_allow_rule()` connect the engine to the approvals that you make in the browser.

**`SpawnFactory`** changes a planner request into a rendered spawn. A rendered spawn has these parts:

- The system prompt for the tool names of that executor.
- The first message with the `Label: value` spawn block.
- The repo brief and the custom instructions.
- The report path.
- The effort.

The engine decides which agent to spawn and with which inputs. The factory decides the form of the spawn for
Claude Code, Codex, or the native loop.

**`Executor`** (`crates/ostra-core/src/exec.rs`) has one method:

```rust
async fn run(&self, spec: ExecutionSpec, host: Arc<dyn ExecutionHost>, cancel: CancellationToken)
    -> ExecutionResult;
```

The engine gives all the data that the execution needs in `ExecutionSpec`. This data is the route, the
prompt, the capabilities, the submit schema, a timeout, and the policy context. The engine gets back one
`ExecutionResult` with a status, the usage, and the `submit_<agent>` payload.

When the execution runs, the executor calls back through `ExecutionHost`. The engine implements these methods:

- `emit` sends live deltas: text, thinking, tool calls, and usage.
- `ask_permission` asks for your answer when the policy needs it.
- `record_message` records messages, so that Ostra can resume a native execution.
- `yolo` reads the YOLO setting, because you can turn YOLO on during an execution.

The tests show the result of this design. `crates/ostra-engine/tests/runner.rs` runs a real engine with fake
services. `tests/conformance/main.rs` checks planner decisions with no executor. In production, the same engine
code runs with the server's implementations in `crates/ostra-server/src/services.rs`.

When the engine needs a new thing from outside, the thing becomes a new method on one of these traits. It
never becomes a dependency.

## Types shared with the browser

The API types are in `ostra-core`, and they have the `#[ts(export)]` attribute from the `ts-rs` crate.
`cargo test -p ostra-core` writes one TypeScript file for each type into `web/src/api/gen/`. The
`TS_RS_EXPORT_DIR` setting in `.cargo/config.toml` sets this path. There are 335 files. They hold
session views, gate payloads, execution deltas, WebSocket messages, settings, and validation issues.

Thus the browser types always agree with the server types. If you rename a field in Rust, regenerate the
types. Then `npm run
typecheck` in `web/` shows each place where the console uses the old name.
The same rule applies to enums.
An `ExecutionStatus` that the server can send is a string union that the console must handle.

## The browser code

The browser code is one npm workspace. You install it one time at the repository root with `npm ci`. It has
three packages:

- `design/` (`@ostra/design`): design tokens and shared React components.
- `web/`: the console, the app that you use to run sessions. The `ostra` binary embeds its build.
- `site/`: the homepage and these docs. It is deployed as a static site.

`web/` and `site/` both import from `@ostra/design`. They never import from each other. The homepage shows a
live copy of the console. The build makes this copy from `web/` with mock data into `site/public/console`. It
uses Vite's `shot` mode, which sets `VITE_MOCK=1 VITE_SHOT=1`. Thus the picture on the homepage is the real
console, not a screenshot.

## Where to go next

- [The pipeline](../internals/pipeline.md): the stages of a session.
- [The event log](../internals/event-log.md) and [the planner](../internals/planner.md): how the engine folds
  the state and how the planner chooses the next step.
- [Executors](../internals/executors.md): the native loop and the harness CLIs behind the `Executor` trait.
- [Agents](../internals/agents.md): prompts, spawn blocks, and structured submits.
- [Storage](storage.md): what lives in each SQLite database and on disk.
- [The server](server.md): the routes, the WebSocket, the CLI, and how the server starts and stops.
- [Server and browser security](../security/server-and-browser.md): sign-in, host and origin checks, and the
  browser hardening.
