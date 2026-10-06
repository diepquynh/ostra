# Executors

An executor is the part of Ostra that runs one agent from its first message to its `submit_<agent>` call. The
engine decides what to run and when. The engine never runs a model itself. It gives an executor a finished
execution spec and gets one result back. The executor controls all the work between these two points:

- The model calls.
- The tool calls.
- The permission asks.
- The streaming output.
- The timeout.

Ostra has two kinds of executor:

- **Native.** Ostra runs the agent loop itself. It calls the provider's API with your API key and runs its own
  tools.
- **Harness.** Ostra starts an installed coding CLI (Claude Code, Codex, Grok Build, or Antigravity) in a
  terminal and lets that CLI run the loop. Ostra still checks each tool call, serves the tools that only Ostra
  has, and reads the result.

Both kinds use the same two traits. Thus, the engine does not know which kind it uses. The route in your
settings selects one kind for each agent ([Settings and routing](settings-and-routing.md)).
[Provider usage](../providers/README.md) lists the credentials that each path takes.

## The contract

The full interface is in `crates/ostra-core/src/exec.rs`. It has two traits:

```rust
pub trait Executor: Send + Sync {
    async fn run(&self, spec: ExecutionSpec, host: Arc<dyn ExecutionHost>, cancel: CancellationToken)
        -> ExecutionResult;
}

pub trait ExecutionHost: Send + Sync {
    fn emit(&self, delta: ExecutionDelta);
    async fn ask_permission(&self, call: &ToolCall, reason: &str, rule: &RuleRef) -> PermissionAnswer;
    fn record_message(&self, role: &str, content: &serde_json::Value) {}
    fn transcript(&self, execution: &ExecutionId) -> Vec<(String, serde_json::Value)> { vec![] }
    fn yolo(&self) -> bool { false }
}
```

An executor supplies `Executor`. The engine gives `ExecutionHost` to the executor for the duration of the run.
`run` never returns an error type. Each possible end of an execution becomes an `ExecutionResult` with a status.
This keeps the runner simple, because the runner handles only one shape of answer.

### What goes in

An `ExecutionSpec` holds all the data that the run needs. The engine resolves this data before the run:

| Field | What it holds |
| --- | --- |
| `route` | The executor and model. The engine resolves them from settings when it builds the spec. |
| `system_prompt` | The agent's prompt, rendered with this executor's tool names. |
| `first_message` | The spawn block, the repo brief, and any custom instructions. |
| `capabilities` | The tools that the agent can have, from its `agent.toml`. |
| `submit_schema` | The JSON schema of `submit_<agent>`. |
| `timeout_secs` | The hard time budget for this execution. |
| `ctx` | The data that the policy needs: the repo root, the session dir, the report path, the permission mode and rules, the protected paths, the memory database, and the sandbox mode. |
| `resume` | Set when this run continues an earlier run. After a pause, `resume.from` is the run's own id. |
| `harness_session_id` | A session id that Ostra selects before the start, for the CLIs that accept one. |

The executor does not read settings to select the model or the tools. The engine made this decision one time,
when it built the spec. The executor reads some settings fresh, for example the sandbox table. It reads them at
the start of each execution, so a change applies to the next execution.

### What comes out

An `ExecutionResult` has these parts:

- A status.
- The submit payload, if there is one.
- The last assistant text.
- The usage.
- The harness session id.
- An error message.

The statuses are:

| Status | Meaning |
| --- | --- |
| `ok` | The agent called its submit tool. |
| `stuck` | The agent submitted with `status: "stuck"`. This status asks the engine for a rescue. |
| `handoff` | The agent submitted with `status: "handoff"`. |
| `error` | The run ended without a usable submit. The cause is a timeout, a crash, a refusal, or a launch failure. |
| `denied` | The engine could not start the run. The route did not resolve, no executor serves it, or the engine could not build the spawn block. No executor ran. |
| `cancelled` | You or the engine stopped the run. |
| `interrupted` | Ostra stopped the run to resume it later, or the server restarted during the run. |

The engine reads only the submit payload. Ostra keeps the final text for display and for the transcript, but
no later step parses it. For this reason, each prompt ends with the instruction to call the submit tool. For
the same reason, both executors remind an agent that stops without the submit call.

### What streams while it runs

`ExecutionHost::emit` takes an `ExecutionDelta`. The engine stores each delta and broadcasts it to the Activity
view. Both executors send the same deltas, except `turn`:

- `text` and `thinking`: model output, in buffered chunks that a person can read.
- `tool_call`, `policy`, `tool_result`: one tool call, the decision of the policy on it, and the result.
- `tool_output`: live output of a shell command that runs now. You can see the output of a long build as it
  comes.
- `usage`: a running total of tokens and cost.
- `turn`: the tokens and cost of one model response, not a total, and the IDs of the tool calls in it. Only the
  native executor sends it.
- `status`: a one-line note, such as "Bash runs without a sandbox" or "The session went quiet; Ostra reminded
  it to submit."
- `native_session_id`: the harness CLI's own session id, after Ostra knows it.

Both executors emit the same deltas. Thus, the Activity view has no separate native mode and harness mode. A
harness run only shows no costs for each response.
A harness run also has a Terminal tab with its raw screen. The native executor has no equivalent tab.

## The native executor

The native executor is in `crates/ostra-exec-native/src/lib.rs`. It is a streaming agent loop over the
`Provider` trait. `ostra-providers` has the implementations for Anthropic (Messages API) and OpenAI (Responses
API).

The Activity tab of a native run shows each tool call with its timing when the loop dispatches the call:

![The Activity tab of a native implementer run](../images/console/execution.png)

### Setup

Before the first model call, a run does the following, in order:

1. Resolves the model to a provider. If the API key is missing, the run fails here. The message tells which
   variable to set.
2. Opens the workspace's MCP servers for this execution ([MCP servers](mcp.md)). If the run cannot reach a
   server, it adds one status line and continues without that server.
3. Builds the policy for this execution from the spec's context and the build and test commands of the project
   profile. The build-streak guard uses those commands.
4. Builds the tool environment: the persistent shell directory, the set of files that the agent read, an HTTP
   client, and the list of environment variables that no child process can inherit.
5. Decides the sandbox. The run reads the `[sandbox]` table fresh. If the table asks for a sandbox, these
   conditions apply:
   - Bash runs under bubblewrap on Linux or Seatbelt on macOS, with a scratch `/tmp` for each execution.
   - Under `allowlist` or `public`, all Bash calls of the execution share one egress proxy. The proxy is a
     socket on bubblewrap and a loopback port on Seatbelt.
   - Ostra records the decisions of the proxy with the execution's activity ([sandboxing](../security/sandboxing.md#network)).

   If the table allows a run without a sandbox and no sandbox is available, the run shows this in the Activity
   view. [OS compatibility](../platforms/os-compatibility.md) lists the backend of each platform.
6. Makes the tool list from the agent's capabilities (refer to [Tools](tools.md)). Then it adds the typed
   `Document` tool for the agents that write a document, and the workspace MCP tools. It adds `submit_<agent>`
   last.
7. Builds the message list. A new run starts with the first message. A resumed run rebuilds the stored
   transcript and adds a resume turn (below).

Ostra marks the system prompt for caching. It also marks the last tool definition, the submit tool. Thus,
there is one cache breakpoint after the system prompt and one after the tool list. Each turn after the first
reads both from the cache. Each breakpoint uses the 1-hour TTL, because the time between two turns of one
execution can be more than five minutes. For example, a long build or an unanswered permission card can take
that time. With a shorter TTL, the cache expires.

### The loop

Each turn sends the full conversation and streams the answer back. The loop buffers text and thinking deltas.
It flushes them every 160 characters or at a newline, so the Activity view shows sentences, not one row for
each token. When the provider runs a server-side tool (web search, or the provider's fetch), the Activity view
shows a status line. If the provider offers its own fetch tool, Ostra removes its `WebFetch`. Thus, the model
sees one fetch tool.

On Anthropic, Ostra sends the `web_search_20250305` and `web_fetch_20250910` tools. These tools call the search
and fetch backends directly. The 2026 versions run inside Anthropic's code-execution sandbox. When code
execution is rate limited (`too_many_requests`), these events occur:

- Each search fails.
- Each failure costs a model turn.
- The model concludes that it has no internet access.

The next action of the loop depends on the reason that the model stopped:

| Stop reason | What the loop does |
| --- | --- |
| Tool calls present | Runs them (below), appends the results, and starts the next turn. |
| `pause_turn` | The provider paused a long server-side tool turn. The loop continues the turn. |
| Refusal | Ends the run as `error` with the refusal category and text. |
| Output limit, no tool call | Asks the model to continue from the point where it stopped, at most 3 times. |
| Plain end of turn, no tool call | If messages wait for the run, the loop gives them to the model and continues. If no messages wait, the loop reminds the model one time to call the submit tool. If a sender waits for a reply from this run (Rule SM6), the reminder tells the model to send that reply first. If a second turn has no tool call, the run ends as `ok` with no submit. The engine sees this result as a failed step. |

The loop has a limit of 400 turns. If a run gets to this limit without a submit, it ends as an error. An agent
without a submit after 400 turns is in a loop.

### Tool dispatch

The model often asks for many tools in one turn. The loop runs them in sequence, with one exception. A
sequence of read-only calls runs concurrently. The read-only tools are `Read`, `Grep`, `Glob`, `WebFetch`,
`MemoryRecall`, `DocsSearch`, `Skill`, and the code navigation tools. Writes and shell commands always run one
at a time, in the sequence that the model gave. The reason is that the second call can depend on the first.

Each call follows the same path:

1. **Canonicalize.** The loop makes these changes:
   - It makes relative paths absolute against the shell's current directory.
   - It gives `Bash` its `cwd`.
   - It replaces a `WebFetch` URL with its parsed form.

   The policy then checks this call, and the tool runs this same call. Thus, the check and the run always have
   the same target. For a URL such as `https://evil.example\@docs.rs/x`, the policy judges the host that the
   URL really reaches.
2. **Check.** `ExecutionPolicy::check` runs the guards and then the permission rules. The decision goes to the
   Activity view.
3. **Deny or ask.** A denial becomes an error result that starts with the correction. An ask goes to
   `ExecutionHost::ask_permission`, which shows a card in the browser and waits. "Always in this workspace"
   also adds a session allow rule. Thus, the same call does not ask again in this execution.
4. **Run.** The tool runs with a live-output callback, so Bash output streams during the command.
5. **Observe.** The policy sees the result. The build-streak guard counts failures with this step:
   - After two failed builds, it appends the project's recorded lessons to the tool result.
   - After five failed builds, it refuses build commands and tells the agent to return `STUCK:`.

A separate page, [Agent containment](../security/agent-containment.md), describes guards and permissions. On
this page, the important fact is that the native loop has no path to a tool that skips step 2.

### The submit call

The submit call is not a tool that does an action. It ends the run. When the model calls `submit_<agent>`,
the loop does these steps:

1. Changes stringified JSON fields back into objects, because models sometimes send a nested object as a
   string.
2. Runs the policy on the call, the same as on all other calls.
3. Asks the engine if a sender waits for a reply from this run (`ExecutionHost::submit_blocked`, Rule SM6). If a
   sender waits, the loop refuses the submit. The refusal tells the agent to send a message to that sender first,
   and gives the subagent ID of the sender.
4. Validates the call against the schema of the run (`validate_submit_with`). The run's result contract
   (`ExecContext.contract`, Rule CA5) selects the schema, not the agent's name. The schema is one of these:
   - The struct of a built-in contract.
   - The `stage` contract with the `data` shape that the agent declares.
   - The schema of a plugin contract.

   The document checks (`doc::check_submit`) also follow the contract.
5. Refuses an `ok` submit if the agent has a declared report file that does not exist, because the engine
   reads that file next. This step does not apply to a `review` run (`Contract::report_required`), because its
   ledger exists only when the review found a problem.

A refused submit is an error result, and the model tries again. An accepted submit ends the run immediately.
The loop answers "Not run" to all other tool calls in the same turn, and these calls never run. The submit's
`status` field selects `ok`, `stuck`, or `handoff`.

### Long runs

An implementer can run for hundreds of turns, so a conversation can become larger than the model's context
window. The loop compacts the conversation before this occurs. When the next request will fill 95% of the window,
the loop does these steps:

1. It asks the provider to summarize the conversation.
2. It replaces each message with the result.
3. It continues.

The context window comes from the models.dev catalog: 1,000,000 tokens for current Opus and Sonnet models, and
200,000 for Haiku 4.5. For a model that is not in the catalog, the loop uses 200,000.

The loop knows the size of the next request, but it does not count tokens itself. Each response reports its
prompt tokens (input, cache reads, and cache writes). The next request sends that prompt and the response
again. Thus, their sum is the start size of the next request. For tool results that the loop added after the
response, it estimates four characters for each token. The loop keeps the latest size as `context_tokens` in
the execution's usage.

Compaction runs only when the last message is a user turn. The reason is that a provider refuses to summarize
when a tool call waits for its result. The method of the summary depends on the provider:

| Provider | How it compacts | What replaces the conversation |
| --- | --- | --- |
| Anthropic, on models with on-demand compaction (Opus 4.6 and later, Sonnet 4.6 and later, Fable, Mythos) | The same request with `compaction: {type: "summarize"}` and Ostra's summarization instructions, beta `compact-2026-09-04` | The signed `compaction` block, sent back verbatim first in every later request |
| OpenAI | `POST /v1/responses/compact` with the conversation and the system prompt | The returned window: the messages it kept and an encrypted `compaction` item |
| Anthropic Haiku 4.5 and older models, or any endpoint that rejects the server-side form | The same request with the instructions as a last user turn and with the tools off. Thus, the cached prefix stays valid. | The summary text, as a user message |

The instructions ask for these items, with paths and errors kept verbatim:

- The task and its requirements.
- The facts from the files read that the agent still needs.
- Each changed file.
- The decisions and their reasons.
- The failed commands.
- The work that remains.
- The exact next step.

After the summary, the loop adds one user turn. The turn says that Ostra compacted the context. It tells the
model to continue from the next step and to read a file again when it needs the content. A compaction starts a
new prompt cache, because each message after the system prompt and the tools changes. For this reason, the
loop waits until the window is almost full. A summary costs one request over the full conversation. Ostra adds
this cost to the execution's usage.

Ostra stores the new window in the transcript as one `compaction` record. When Ostra reads a transcript back,
it starts from the latest `compaction` record. Thus, a resumed run, a plan revision round, or a message run continues
from the summary, not from the full history. No summary can come back because the summary was cut off, the model
refused, or the request failed. In this case, the run continues without a summary. It tries again only after
the context increases by 2% of the window, because each try sends the full conversation.

Earlier versions cleared old tool results with Anthropic's `clear_tool_uses` context edit. That edit changes
the conversation near its start on each turn. Thus, after the first cleared result, each request missed the
prompt cache and wrote the remaining conversation to the cache again. One plan revision spent about $6 of $7 on
those writes. Compaction changes the conversation one time. After that, the cache is valid again.

### Provider errors

The provider clients retry rate limits and server errors with exponential backoff and jitter. They do at most
4 retries, and the maximum delay is 60 seconds. They obey a `Retry-After` header. They never retry after the
output starts to stream, because a retried turn repeats text that the Activity view showed. If a call fails
after the retries, the run ends with the provider's error.

### Transcript and resume

Each message that the loop sends or receives goes to `ExecutionHost::record_message`, which stores it. A
resumed execution reads that transcript back and adds one user turn:

- If the last stored message is from the assistant, the loop answers its open tool calls with "Interrupted:
  this call did not finish.". The reason is that the provider refuses a conversation with tool calls that have
  no results.
- Then the loop adds the resume note. The note is the note from the engine ("Continue the workflow." after a
  pause) or "The previous run was interrupted. Continue from where it stopped."
- If the last stored message is a user turn, the note becomes part of it. This occurs when the run stopped
  during a wait for the model. The reason is that Ostra merges two sequential user turns when it reads the
  transcript.

A run resumed after a pause is the same execution. Thus, Ostra already stores its transcript under its id. The
loop records only the new turn. Its request starts with the same messages that the interrupted run sent. Thus,
the provider's prompt cache covers that prefix until the cache expires. A run resumed from a different
execution, such as a retry after a failure, copies the full rebuilt transcript into its own transcript.

Messages between subagents use the same two paths. A `SendMessage` with `wait: true`, or a `WaitForMessage`,
ends the run with status `waiting`. Ostra records its tool result the same as a submit. Later, the next message
for the run resumes it in place, with the messages as the note. A pair-loop round or a message run continues the
conversation of a different execution. A message run is a subagent that Ostra continues for messages that
arrived after its run ended. Thus, the round or the message run copies that transcript. Then it adds the new
spawn block, or the messages, as the note.

### Messages at the turn boundary

Ostra never puts a message into a request that is in progress (Rule SM2). After the tool calls of a turn run,
the loop asks the host for the messages that wait for this run (`ExecutionHost::take_messages`). The loop adds
them as one text block after the tool results, in the same user turn. All turns before that turn stay the same.
Thus, the next request still starts with the cached prefix.

A turn with no tool calls is also a boundary. The waiting messages come first, and the submit reminder comes
later. `take_messages` records each delivery as a `MessagesDelivered` event before it returns the text. Thus, the
fold knows which run read which message.

### Programmatic agents

A plugin agent without a prompt runs in the code of its plugin, not in this loop (Rule PL2,
`crates/ostra-exec-native/src/program.rs`). The engine sends it to `NativeExecutor::run_program`, whatever
executor its settings name. The engine resolves its route again on the native executor. The model of that route
serves the model calls of the agent.

The run uses the same setup as the loop (`Run::setup`): the provider, the tool environment with its sandbox,
the MCP tools, and the policy. Then the plugin gets its task and an `AgentCalls` handle. The task contains these
items:

- The spawn block and the brief, as `first_message`.
- The repo root and the session dir.
- The model.
- The submit schema.
- The names of its tools.

The `AgentCalls` handle has these methods:

- `tool(name, input)` runs one tool through the same `tool_end` path as a model's call: canonicalize, check,
  ask, run, observe. The plugin can call only the tools that its capabilities give. Any other name, and the
  submit tool, return an error result. The messages that wait for the run follow the output of the tool. A
  `SendMessage` with `wait` or a `WaitForMessage` keeps the call open until a message arrives
  (`ExecutionHost::wait_for_wake`). The message is the rest of its output. Thus, the agent waits with its process
  alive, the same as a harness run.
- `complete(request)` makes one model call on the route of the agent, with the effort of the run. It streams the
  call to the Activity view and adds its usage to the usage of the run.
- `status(text)` adds a line to the Activity view.

A run can make at most 2,000 tool and model calls in total (`MAX_PROGRAM_CALLS`). This limit corresponds to the
400 turns of the loop. When the plugin returns, the result goes through the same checks as a submit from a model:

- If a sender still waits for a reply, the run fails.
- Ostra validates the submit against the schema of the run (`validate_submit_with`).
- A declared report file must exist, unless the contract is `review`.

The document check of the model loop (`doc::check_submit`) does not run on a programmatic result. A plugin
error, an invalid result, or a missing reply ends the run as `error`. The timeout and the cancel of the run
apply the same as for each native run.

Known weakness: if the effort of a continued conversation changes, the conversation loses the prompt cache.
Ostra resolves the effort fresh for each execution from the agent's default and the workspace's
`routing.effort` settings. It sends the effort as the top-level `output_config.effort`. If that value is
different from the earlier requests, the provider invalidates the cached messages. Then the provider writes the
full replayed transcript to the cache again at the full write price.

This occurs when you edit the effort settings between two runs of one conversation, for example between spec
rounds. In one session, a spec round with only one tool call cost $1.73. The cause was that it wrote about
200,000 tokens of the previous round's transcript to the cache again. The executor does not record the effort
of a run, so a continuation cannot keep it.

### Cost

The loop adds the usage of each response to a running total and emits it immediately. Thus, the cost in the
header changes during the work of the agent. Prices come from the models.dev catalog. Ostra caches the catalog
in the data dir and refreshes it daily. The price of an Anthropic 1-hour cache write is twice the input rate,
because models.dev lists only the 5-minute write price. [Spend and limits](spend-and-limits.md) tells how these
numbers become part of the session budget.

Ostra records the wall time of build and test commands separately as `build_ms`. The build-loop metric uses this
value. `context_tokens` is the size of the latest request's context, not a sum. Thus, it shows how near a run is
to compaction.

## Harness executors

The harness executor is in `crates/ostra-exec-harness`. It runs one agent inside an installed CLI, under a
real pseudo-terminal, in the project directory. The PTY bytes go to the browser, where xterm.js shows the CLI's
own interface. You can see the work of the CLI and type into it.

The four supported CLIs:

| Harness | Binary | Session id | Cost source |
| --- | --- | --- | --- |
| Claude Code | `claude` | Chosen by Ostra (`--session-id`) | `~/.claude/projects/*/<id>.jsonl` |
| Codex | `codex` | Captured from the first event or transcript | `rollout-*.jsonl` |
| Grok Build | `grok` | Chosen by Ostra (`--session-id`) | `sessions/<cwd>/<id>/updates.jsonl` |
| Antigravity | `agy` | Captured from the screen or transcript | None: its transcript records no usage |

You can change the binary name for each harness with `[harness.<name>].command`. Put more arguments in
`[harness.<name>].args`. We checked the launch flags against claude 2.1.280, codex 0.153.4, grok 1.0.30, and
agy 1.2.6.

### Why run a harness at all

A harness route bills the run to the account of the CLI's sign-in. It uses the provider's own sign-in and the
provider's own client. Ostra uses a subscription only in this way. It never copies a token, and it never signs
in for you. The harness also has its own tool implementations and its own tuning for each model.

But Ostra has less control. Ostra did not write the loop and cannot change it. Thus, the harness executor adds
three controls around a loop that Ostra does not own:

1. Each tool call of the CLI passes Ostra's policy. The native loop uses the same policy.
2. The CLI gets Ostra's own tools (submit, report, document, memory, code navigation, and the workspace MCP
   servers). It gets no tool that lets it go around them.
3. The run ends when the agent submits, and Ostra gets this information.

The next sections describe the parts that do each of these controls.

### The launch plan

`launch.rs` changes an execution spec into a `LaunchPlan`: a program, arguments, environment, working
directory, and the files to write before the start. Each CLI takes its configuration in a different way. Thus,
each CLI has its own planner. The planners share these steps:

- **The prompt.** Ostra renders the agent prompt with the tool names of that harness. The result becomes the
  addition to the CLI's system prompt:
  - `--append-system-prompt-file` for Claude Code.
  - `developer_instructions` for Codex.
  - `--rules` for Grok.
  - An agent file for Antigravity.

  The first message becomes the CLI's first user message. If the first message is more than 96 KiB, Ostra
  writes it to a file and gives the file to the CLI. The reason is that command lines have a length limit.
- **Hooks.** Each PreToolUse, PostToolUse, Stop, and SessionStart event goes to
  `ostra hook --execution <id> --harness <name> --event <event>`, a subcommand of the same `ostra` binary. The
  hook timeout is 24 hours. The reason is that a permission ask waits for you, and the hook must not time out
  before you answer.
- **The MCP server.** Ostra registers one stdio MCP server with the name `ostra`: `ostra mcp-stdio --execution <id>`.
- **No subagents.** Ostra turns off the CLI's own subagent tool (`--disallowed-tools Agent,Task` for Claude
  Code, `--disable multi_agent` for Codex, `--no-subagents` for Grok). Each Ostra agent is a leaf. The reason is
  that a delegated task runs where Ostra cannot see it, and its result never gets to the engine.
- **No questions in the terminal.** Ostra disables Claude Code's `AskUserQuestion` and plan-mode tools,
  because no person monitors the terminal for questions. If an agent needs you, it puts the question in its
  submit call. The engine changes the question into a gate.
- **The model and effort.** Ostra passes the routed model explicitly. It maps the effort from `agent.toml` to
  the CLI's word for it, clamped to the values that the CLI accepts.
- **Environment.** The CLI gets your environment, so it starts with a sign-in. Ostra removes the credential
  variables that its own config names. It keeps the variables that the CLI reads to sign in (for example, Claude
  Code keeps `ANTHROPIC_API_KEY`). Ostra itself can start from a Claude Code session. Ostra also removes the
  variables that make the CLI a nested child of that session. The reason is that Claude Code does not save a
  nested session, and the session cannot be resumed. [Secrets and data](../security/secrets-and-data.md) tells
  which variables are credentials.

Ostra writes the config files that hold the bridge token with owner-only permissions.

### Keeping the repository's own config out

A coding CLI reads the settings of the folder in which it starts. These settings can contain hooks and MCP
servers. These hooks and servers run outside Ostra's guards. Each planner blocks them in a different way for
its CLI:

- **Claude Code** loads only your user settings and Ostra's settings (`--setting-sources user`). It loads only
  Ostra's MCP config (`--strict-mcp-config`). Ostra passes its hooks as flag settings, and a user settings file
  cannot turn them off. Ostra answers yes to the folder-trust prompt, because in this mode, trust adds nothing
  from the repository.
- **Codex** gets a profile for each execution. The profile marks the repository and each folder up to its git
  root as `untrusted`. Thus, Codex opens with restrictions. The repository's `.codex/` config, hooks, and exec
  policies stay off, and Ostra does not change your saved trust. If the prompt of Codex offers only "Trust and
  continue", the run stops. It tells you to remove `-p`/`--profile` from your extra args, because trust loads
  the repository's config. Ostra deletes the profile file when the execution ends. Codex also runs with
  `check_for_update_on_startup = false`. The reason is that a self-update stops the CLI and ends the run before
  it starts.
- **Grok Build** gets a `GROK_HOME` for each execution. This `GROK_HOME` links to your real one. Its config,
  its trust list, and its leader socket are exceptions: they are private copies. Ostra trusts only the session
  dir. If Grok asks to trust the repository, the run stops and asks you to trust it in Grok yourself. The reason
  is that Grok runs the hooks and MCP servers of a trusted folder, and Ostra does not make that decision for you.
- **Antigravity** reads hooks and MCP servers only from global config. Ostra installs one global integration.
  Its hook and MCP commands do nothing if `$OSTRA_EXECUTION` is not set. Thus, the integration has no effect in
  your own Antigravity sessions.

### The hook bridge

The CLI runs `ostra hook` for each hook event and pipes the event payload to it on stdin. That process does
little work. It forwards the payload to the running server at `POST /internal/policy` with the execution's
bearer token. Then it prints the answer of the server and exits. The server's `HarnessBridge` (`bridge.rs`)
does the work:

```
harness CLI ── stdin ──> ostra hook ── POST /internal/policy ──> HarnessBridge
                                                                   │ adapter.parse_pre
                                                                   │ ExecutionPolicy::check
                                                                   │ (ask: wait for the browser)
harness CLI <── stdout ── ostra hook <──── adapter.pre_response ───┘
```

The bridge fails closed. A PreToolUse answers with a denial that tells the agent to stop if one of these
conditions occurs:

- `ostra hook` cannot reach the server.
- `ostra hook` cannot read the answer.
- `ostra hook` does not have its URL or token.

An unknown execution, an old token, or a hook with a harness that is different from the execution also causes a
denial. Ostra compares tokens in constant time. `/internal/*` accepts only local peers.

A sandboxed harness can reach the server's TCP port only under the network choice `host`. Thus, at startup,
the server also serves the two `/internal/*` routes, and nothing else, on a Unix socket in the data dir
(`serve_bridge_socket`):

- In a bubblewrap sandbox, Ostra's helper listens on the bridge's port on the sandbox's own loopback. It
  forwards each connection to that socket.
- A Seatbelt sandbox shares the host's loopback. A new port of `127.0.0.1` splices to the socket, and the policy
  lets the CLI connect to that port.

In both cases, the sandbox cannot reach the console's API. The launch sets `OSTRA_URL` to
`http://127.0.0.1:<port>` for that port. It also writes the same URL into each config file that it made that
names the bridge. The reason is that Grok's config passes `OSTRA_URL` to the MCP server that Grok starts.

A Stop event also goes through the bridge. With this event, Ostra prevents an agent stop without a result. If
the agent stops before its submit, the bridge blocks the stop and tells the agent to call its submit tool. The
bridge does this two times. On the third stop, the run ends as an error ("The agent ended its turn three times
without calling submit").

The Tool calls tab of a harness run lists each call that the hook bridge saw. It shows the policy decision and
the rule of the decision:

![The Tool calls tab of a Claude Code run with allowed, denied, and asking calls](../images/console/harness-toolcalls.png)

### Adapters

Each CLI has different tool names and expects a different answer shape. `adapters/` has one adapter for each
CLI. Each adapter changes its CLI's payload into a canonical `ToolCall` (Claude Code's tool names and input
shapes). It also changes Ostra's decision back into the exact response that the CLI accepts. The policy engine
sees only canonical calls. Thus, a guard has one implementation, and it applies to all four CLIs.

| CLI | Tool names it uses | Answer shape and its quirks |
| --- | --- | --- |
| Claude Code | Already canonical. `MultiEdit` and `NotebookEdit` map to `Edit`, `LS` to a read. | Decisions and rewrites go only in `hookSpecificOutput`, because a top-level field does not agree with its schema. |
| Codex | `exec_command`/`shell` to `Bash`, `apply_patch` to the files the patch touches. | Rejects an `allow` without `updatedInput`, and rejects `ask`. Thus, an allowed call answers `{}`. Its hooks get no exit status. Thus, the policy judges the output text. |
| Grok Build | `read_file`, `write_file`, `search_replace`, `run_terminal_command`, `grep`, `list_dir`, `web_fetch`, and others. | Grok clips reasons to 256 characters. Thus, Ostra shortens them and keeps the correction and the final instruction. A payload over 128 KiB loses its tool input. Ostra refuses such a call, because the policy cannot judge it. |
| Antigravity | `view_file`, `write_to_file`, `replace_file_content`, `run_command`, `grep_search`, `find_by_name`, `read_url_content`, `search_web`. | Antigravity validates the output with proto, and an unknown field discards the full response. Thus, Ostra answers only `decision` and `reason`. PostToolUse has no tool result. |

The adapter refuses some calls before the policy sees them:

- It refuses a subagent spawn with the leaf rule.
- It refuses a question to the user in the terminal. The refusal tells the agent to put the question in the
  submit call.
- It refuses an input that it cannot read. It does not let this input through.

### The MCP stdio shim

`ostra mcp-stdio` is a stdio MCP server. It forwards each JSON-RPC message to `POST /internal/mcp` and prints
the answer. Ostra uses stdio because it is the only MCP registration shape that works on all four CLIs. The
shim uses MCP protocol versions 2024-11-05 through 2025-11-25. If it starts outside an Ostra execution, it
answers the handshake with no tools. Thus, a global registration has no effect.

It serves:

- `submit_<agent>`, the only submit tool that this agent can call, with the schema of the run. The executor gives
  the contract of the run to the live execution (`LiveExecution::set_contract`). The bridge validates the submit
  and runs the document checks by that contract, the same as the native loop.
- `report`, `memory`, `memory_recall`, and `docs_search`, if the agent's capabilities allow them. Also
  `document`, with a schema for each typed document that the run has a grant for (Rule CA6).
- `code_outline`, `code_find`, and the rest of the code navigation tools.
- `list_agents`, `send_message`, and `wait_for_message`, to an agent with the `coordinate` capability.
- `project_list` and `project_create`, only to an execution with an agent that has the `manage_projects`
  capability (the implementer by default). The reason is that the shim lists them only when the server gave the
  execution a management handle.
- The tools of each workspace MCP server, as `<server>__<tool>`. The CLI sees them as
  `mcp__ostra__<server>__<tool>`.

A `tools/call` goes through the same policy check as a hook. The reason is that the hook bridge does not check
calls to Ostra's own MCP server (rule M2). A check in both places asks you two times.

The result comes in the submit call. The bridge validates the call against the schema and the document
checks, and records it. Then it answers "Recorded. Your run is complete: end your turn now, without further tool
calls." The bridge refuses a second submit. The prompt's tool vocabulary tells the agent to reply only `Done!`
after the submit. The reason is that all other text costs output tokens, and no part of Ostra reads it.

### Supervising the terminal

During the CLI run, the executor's `supervise` loop wakes on each hook event or MCP call, and at least each
half second. On each pass, it checks these conditions:

- **Submitted.** After Ostra records a submit, the run ends at the CLI's next Stop event. If no Stop comes, the
  run ends 20 seconds later. Antigravity has no Stop hook. Thus, for Antigravity, 4 seconds with no terminal
  output after the submit end the turn.
- **Startup screens.** In the first two minutes, and only until the first tool call, the loop reads the screen:
  - It answers folder trust as the section above describes.
  - It declines a new-model offer (Codex's "Meet GPT-6 Luna") with the choice "Use existing model". The
    reason is that a yes changes the run's model and your default model.
  - Sign-in text such as "please log in", "select login method", or "finish signing in" ends the run
    immediately as a sign-in failure. Without this check, the CLI stays on its sign-in screen for the full
    budget.
  - "unknown model" or "failed to construct executor" ends the run as a launch failure.
- **Quiet.** If a session has no hook event and no terminal output for 4 minutes, Ostra sends a reminder. Ostra
  types the submit instruction into the terminal, the same as a person. After two reminders, Ostra ends the run
  as an error.
- **Waiting.** A `send_message` with `wait` or a `wait_for_message` puts the run into a wait. The bridge marks the
  run as waiting. Then the loop stops its checks and waits for the engine's message
  (`ExecutionHost::wait_for_wake`). These conditions apply during the wait:
  - Ostra releases the run's execution slot.
  - Ostra lets a Stop without a submit through.
  - Ostra adds the wait time to the deadline.

  Ostra types the message into the terminal, and the checks start again. On the side of the engine
  (`EngineHost::wait_for_wake` in `crates/ostra-engine/src/runner.rs`), the wait checks the fold for messages
  at least every 2 seconds. It also checks each time an event is appended to a session of the workspace, because
  `Inner::append` signals the wait. The wait releases the slot on its first empty check. It gets a slot again
  before it returns. It returns nothing only when the session ended or the run no longer waits. If a run can get
  no message, Ostra wakes it with a notice (Rule SM3).
- **Messages for a running run.** Ostra never types a message into a turn that is in progress (Rule SM2). When
  the turn of the CLI ends, the Stop hook asks the engine if messages wait for the run
  (`ExecutionHost::has_messages`). If messages wait and the run did not submit, Ostra lets the Stop through and
  marks the run as waiting. Thus, the next pass of the loop takes the messages immediately and types them in.
- **Owed replies.** A sender can wait for a reply from this run (Rule SM6). Then the blocked Stops and the typed
  reminders name `send_message` and the ID of that sender. The bridge refuses the submit until the run sends the
  reply. The bridge asks the engine each time (`submit_blocked`). Thus, a reply duty that arrives with a message
  during the run also counts.
- **The budget.** After `timeout_secs`, the run ends.

The terminal itself is a `vt100` screen model in the server (`pty.rs`). If a browser attaches in the middle of
a run, Ostra sends it the current screen and its scrollback. Only Ostra answers the terminal queries that TUIs
send (cursor position, device attributes, colors). The browsers do not send their answers. Thus, the CLI gets
exactly one answer, with zero tabs open or with five. Ostra also writes the raw bytes to an owner-only file with
a size limit. Thus, the Terminal tab can replay an ended run.

The Terminal tab of a harness run shows the CLI on its PTY. This Claude Code run is paused on a permission ask
that the hook bridge holds:

![The Terminal tab of a Claude Code harness run with a paused permission ask](../images/console/terminal.png)

### Failures that are not the agent's fault

Some CLI errors are not task failures, because the task never started:

- The CLI is not installed.
- The CLI has no sign-in.
- The CLI stops within 20 seconds without one tool call.

These errors have a `harness-auth:` or `harness-launch:` prefix. For them, the planner opens a `HarnessFailure`
gate, not the generic "execution failed" gate. The gate offers two choices: log in and retry, or run this one
execution on the native executor. Under YOLO, the engine routes to native and records this change
([Gates and judges](gates-and-judges.md)). Settings validation already refuses a route to a harness that is
not installed. Thus, this gate catches mostly expired sign-ins.

If a harness fails to start, Ostra opens a harness failure gate with its exit message. An error in the run
itself opens an execution failed gate:

![A harness failure gate for Claude Code and an execution failed gate](../images/console/gate-harness-failure.png)

### Usage and cost

Harnesses do not report cost through hooks. Ostra does not use hooks for cost, because hooks are for policy
enforcement. `usage_watch.rs` monitors the CLI's own session file with a file watcher. It reads each appended
line one time, so the cost shows live. When the run ends, Ostra reads the full file again for the final number.
Ostra reads the file of each CLI in a different way:

- Claude Code repeats the usage of a message on each line of the message. Thus, Ostra counts usage one time
  for each message id.
- Codex writes running totals, and their input includes cached input.
- Grok gives the cost of each turn in units of 10^-10 dollars.
- Antigravity records no usage, so its runs show no cost.

### Sandbox

When the sandbox is on, the CLI itself runs inside it. Thus, its shell tool and all the processes that it
starts inherit the sandbox. Ostra tells the CLI not to start its own sandbox, because macOS cannot nest Seatbelt
profiles. The CLI gets one egress proxy for the full launch. With all network choices, the proxy lets the CLI
reach these hosts:

- Its own model and sign-in hosts (`egress::model_hosts`).
- The hosts of the model endpoints in its configuration: `*_BASE_URL` variables, `env.*_BASE_URL` in Claude
  Code's `settings.json`, and Codex's `model_providers.*.base_url`.

Under bubblewrap, Ostra puts an overlay on your home folder. Thus, the CLI can write its state dirs, but it
leaves no files that your later shells or CLI sessions run. Under Seatbelt, there is no overlay, so the
remaining part of the home folder stays read-only. Harness executors use POSIX PTYs and process groups, so they
run on Linux and macOS.

### Resume and read-only sessions

To resume a harness run, Ostra starts the CLI with its own resume flag (`--resume`, `codex resume`,
`--conversation`) and the saved session id. Thus, the agent continues in its earlier conversation.

On an ended run, "Open the session" starts a read-only copy. The CLI opens that session again in a new
terminal. You can scroll through the work and ask about it. The copy has no first prompt, so it spends nothing
until you type. These rules apply to the copy:

- Ostra refuses each tool call (guard `read-only-session`).
- Ostra serves no Ostra MCP tool.
- The Stop hook never asks the copy to submit.
- Ostra sends no reminders when the copy is idle.

The copy ends when you leave the CLI or cancel it, or after 4 hours.

An ended harness run replays its stored transcript with input off. Resume opens the CLI's own session again:

![An ended Codex run replaying its transcript with a Resume button](../images/console/codex-transcript.png)

## Timeouts and cancellation

Each agent's `agent.toml` sets `timeout_seconds`, which becomes `timeout_secs` in the spec:

| Agent | Budget |
| --- | --- |
| quick-answer | 5 minutes |
| advisor | 15 minutes |
| code-reviewer, execution-path-analyzer | 20 minutes |
| explore, generate-spec, fact-check, plan, prompt-generation | 30 minutes |
| implementer, write-test, initializer, documentation | 40 minutes |

The budget is a hard limit on wall time. It includes the time of a wait for a permission card. The native
executor enforces it with a `tokio::select!` around the full run. The harness executor checks it in the
supervise loop, with a minimum of 30 seconds.

A cancellation occurs immediately. The runner holds a `CancellationToken` for each execution. Stop on the
board or `POST /api/sessions/<id>/stop` cancels it.

- **Native.** The loop sees the cancel immediately, also in the middle of a streaming response, and returns
  `cancelled`. Tools that run get 3 seconds to stop. Ostra kills a Bash command with its full process group.
  Thus, if a build started 12 compilers, no compiler stays.
- **Harness.** Ostra first sends Esc. Esc ends the CLI's turn, so the CLI saves its full session, and Ostra can
  resume it. Then Ostra signals the process group, waits 3 seconds, and kills the remaining processes. These
  include all processes that the CLI started inside its sandbox.

In both cases, Ostra keeps the usage until the cancel in the result. A harness resumed after a pause adds to its
terminal log and does not replace it. Thus, the Terminal tab replays both parts.

The engine sometimes cancels a run intentionally, for example to deliver context that you added during the run.
The runner records the reason. Then the `cancelled` result becomes `interrupted` with that reason. The planner
reads this result as "resume this", not as "this failed". If the server restarts during executions, it marks
each execution `interrupted` with "The server restarted while this execution ran.". It keeps the cost that the
executions spent. Thus, recovery can resume them (refer to [The event log](event-log.md)).

## Where to look in the code

| Concern | File |
| --- | --- |
| The traits and result types | `crates/ostra-core/src/exec.rs` |
| The native loop | `crates/ostra-exec-native/src/lib.rs` |
| Retries | `crates/ostra-providers/src/retry.rs` |
| Compaction per provider | `crates/ostra-providers/src/lib.rs` (`Provider::compact`), `anthropic.rs`, `openai.rs` |
| Harness start, supervise, and end | `crates/ostra-exec-harness/src/executor.rs` |
| Per-CLI command lines and config | `crates/ostra-exec-harness/src/launch.rs` |
| Hook bridge and MCP shim | `crates/ostra-exec-harness/src/bridge.rs` |
| Per-CLI payload translation | `crates/ostra-exec-harness/src/adapters/` |
| How a harness run ended | `crates/ostra-exec-harness/src/outcome.rs` |
| Terminal and screen model | `crates/ostra-exec-harness/src/pty.rs` |
| Live harness cost | `crates/ostra-exec-harness/src/usage_watch.rs`, `transcript.rs` |
| Running an execution | `crates/ostra-engine/src/runner.rs` |

Next: [Tools](tools.md) describes each tool that an agent can call.
