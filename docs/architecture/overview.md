# Architecture overview

Ostra is one Rust binary and one browser app. The binary runs on your machine, holds every piece of state,
and runs the models. The browser shows what the binary is doing and sends your answers back. This page
covers how the binary is divided into crates, why the dependencies point the way they do, and how the
browser code is organized.

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

When you start `ostra`, the server opens the machine registry, attaches every registered workspace, and
gives each one its own engine. The engine recovers any sessions that were running when the server last
stopped, and then waits. From then on, nothing happens without an event: you create a session, the engine
appends `SessionCreated`, the planner reads the new state and decides the next steps, the runner performs
them, and each result becomes another event.

## The crates

Ostra is fifteen crates in one Cargo workspace. Each has one job.

| Crate | What it owns |
| --- | --- |
| `ostra-core` | Ids, settings and route resolution, pipeline enums, submit schemas, the event types, API types, the `Executor` and `ExecutionHost` traits, every derived path |
| `ostra-sandbox` | The sandbox under agent commands and project programs: profiles, the bubblewrap and Seatbelt backends, the egress proxy, decoys, and one OS layer per platform |
| `ostra-store` | SQLite: the workspace database, the machine registry, each project's lesson memory |
| `ostra-policy` | Guards and permissions over canonical tool calls, and the bash parser |
| `ostra-tools` | Native tool implementations: Read, Write, Edit, Bash, Grep, Glob, Skill, WebFetch, and the rest |
| `ostra-providers` | Streaming clients for Anthropic and OpenAI, retry, and a scripted provider for tests |
| `ostra-agents` | The embedded prompts, skills, and refs; prompt rendering per executor; typed spawn structs; the repo brief |
| `ostra-engine` | Event-sourced session state, the pure planner, judges, the runner, the spawn factory |
| `ostra-exec-native` | The native agent loop: a provider, the tools, and the policy in one loop |
| `ostra-exec-harness` | Harness executors: the PTY, one adapter per CLI, the hook bridge, the MCP stdio shim |
| `ostra-code` | A tokenizer, the per-project code index, a language server client, and the code providers for the Files view |
| `ostra-mcp` | The MCP client: stdio and streamable HTTP transports, and OAuth sign-in |
| `ostra-notify` | Web Push, written without OpenSSL |
| `ostra-workspace` | Workspaces: settings checks, projects, command approvals, creating and deleting a workspace, and the runtime of an open one |
| `ostra-server` | The `ostra` binary: axum, auth, REST, WebSocket, the embedded web build, the CLI |

## Which way the dependencies point

```
layer 0   ostra-core                     depends on no Ostra crate
          ostra-mcp                      depends on no Ostra crate

layer 1   ostra-store      → core
          ostra-policy     → core
          ostra-agents     → core
          ostra-providers  → core
          ostra-sandbox    → core
          ostra-notify     → core

layer 2   ostra-tools        → core, store, sandbox
          ostra-code         → core, sandbox
          ostra-engine       → core, store, agents, sandbox

layer 3   ostra-exec-native  → core, store, tools, policy, providers, sandbox
          ostra-exec-harness → core, tools, code, sandbox
          ostra-workspace    → core, store, engine, agents, policy, sandbox

layer 4   ostra-server       → every crate above
```

An arrow points at what a crate depends on. Nothing depends on a crate in a higher layer.

Three rules keep this shape:

1. `ostra-core` depends on no other Ostra crate. Every other crate can use its types, so a type that two
   crates share lives there.
2. `ostra-engine` knows no executor, no provider, and no server. It depends on `ostra-core`, `ostra-store`,
   `ostra-agents`, and `ostra-sandbox` (for the project commands it runs itself), and nothing else from Ostra.
3. `ostra-server` is the one place where the real pieces meet. It builds the providers, the executors, the
   MCP gateway, and the notifier, and hands them to each engine through traits.

`ostra-workspace` sits beside the executors for the same reason as the engine: it knows no provider,
executor, or server. What an open workspace needs from the process (the registry, the global config, the
machine facts for validation, provider and harness status, and the engine's `Services`) arrives through the
`WorkspaceHost` trait in `crates/ostra-workspace/src/host.rs`, which the server implements on its shared
state in `crates/ostra-server/src/app.rs`.

## Why the engine knows so little

The engine decides what a session does next. That decision has to be the same whether the agent runs on the
native loop against the Anthropic API, inside Claude Code in a terminal, or inside a test with a scripted
model. If the engine imported an executor, a provider, or the web server, each of those would leak into the
decision: a harness quirk would become a planner branch, and a test would need a real HTTP stack.

So the engine sees the world through three traits, and the server fills them in.

**`Services`** (`crates/ostra-engine/src/services.rs`) is everything the engine asks of the outside:

- `global()` and `workspace()` return the settings. They are read again on every execution, so an edit in
  Settings applies to the next execution without a restart.
- `executor(kind)` returns the executor for `native` or `harness:<name>`.
- `factory()` returns the spawn factory.
- `judge(route, system, user, schema, effort)` makes one judge call: a single model request with a forced
  `decide` tool whose input must match the schema.
- `notify(notice)` asks for a push notification. The server decides whether push is on.
- `protected_paths()` lists what no agent may touch: Ostra's own binary, config, and databases.
- `command_approved()` and `add_allow_rule()` connect the engine to the approvals you make in the browser.

**`SpawnFactory`** turns a planner request into a rendered spawn: the system prompt for that executor's tool
names, the first message with the `Label: value` spawn block, the repo brief and custom instructions, the
report path, and the effort. The engine decides who to spawn and with what inputs; the factory decides how
that looks for Claude Code, Codex, or the native loop.

**`Executor`** (`crates/ostra-core/src/exec.rs`) has one method:

```rust
async fn run(&self, spec: ExecutionSpec, host: Arc<dyn ExecutionHost>, cancel: CancellationToken)
    -> ExecutionResult;
```

The engine hands over everything the execution needs in `ExecutionSpec` (route, prompt, capabilities, the
submit schema, a timeout, and the policy context) and gets back one `ExecutionResult` with a status, usage,
and the `submit_<agent>` payload. While it runs, the executor calls back through `ExecutionHost`, which the
engine implements: `emit` for live deltas (text, thinking, tool calls, usage), `ask_permission` when the
policy wants your answer, `record_message` so a native execution can be resumed, and `yolo` because you can
turn YOLO on in the middle of an execution.

The payoff shows in the tests. `crates/ostra-engine/tests/runner.rs` runs a real engine with fake services,
and `tests/conformance/main.rs` checks planner decisions with no executor at all. The same engine code runs
in production with the server's implementations in `crates/ostra-server/src/services.rs`.

When the engine needs something new from outside, it becomes a new method on one of these traits. It never
becomes a dependency.

## Types shared with the browser

The API types live in `ostra-core` and carry `#[ts(export)]` from the `ts-rs` crate. Running
`cargo test -p ostra-core` writes one TypeScript file per type into `web/src/api/gen/` (the path is set by
`TS_RS_EXPORT_DIR` in `.cargo/config.toml`). There are close to 300 of them: session views, gate payloads,
execution deltas, WebSocket messages, settings, and validation issues.

The browser therefore cannot drift from the server. Rename a field in Rust, regenerate, and `npm run
typecheck` in `web/` shows every place the console used the old name. The same holds for enums: an
`ExecutionStatus` the server can send is a string union the console must handle.

## The browser code

The browser code is one npm workspace, installed once at the repository root with `npm ci`. It has three
packages:

- `design/` (`@ostra/design`): design tokens and shared React components.
- `web/`: the console, the app you use to run sessions. Its build is embedded into the `ostra` binary.
- `site/`: the homepage and these docs, deployed as a static site.

`web/` and `site/` both import from `@ostra/design` and never from each other. The homepage shows a live
copy of the console, built from `web/` with mock data (Vite's `shot` mode, which sets `VITE_MOCK=1 VITE_SHOT=1`) into `site/public/console`,
so the picture on the homepage is the real console rather than a screenshot.

## Where to go next

- [The pipeline](../internals/pipeline.md): the stages a session moves through.
- [The event log](../internals/event-log.md) and [the planner](../internals/planner.md): how state is folded
  and how the next step is chosen.
- [Executors](../internals/executors.md): the native loop and the harness CLIs behind the `Executor` trait.
- [Agents](../internals/agents.md): prompts, spawn blocks, and structured submits.
- [Storage](storage.md): what lives in each SQLite database and on disk.
- [The server](server.md): routes, the WebSocket, the CLI, and how the server starts and stops.
- [Server and browser security](../security/server-and-browser.md): sign-in, host and origin checks, and the
  browser hardening.
