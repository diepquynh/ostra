# Settings and routing

Ostra must answer three questions before each execution can run:

- Which executor runs it: the agent loop of Ostra or one of the vendor CLIs.
- Which model it uses.
- Which reasoning effort the model uses.

Ostra gets the answers from settings, and it gets them again for each execution. This page explains where the
settings are and how a route becomes a concrete model. It also explains what Ostra checks when you save, and why
Ostra keeps some settings out of the files in your repository.

## Where settings live

Settings come from three files and one database. Each has a different owner and a different level of trust.

| Where | Path | Written by | Holds |
| --- | --- | --- | --- |
| Global config | `$OSTRA_CONFIG`, else `config.toml` in the OS config folder (`~/.config/ostra/` on Linux, `~/Library/Application Support/ostra/` on macOS) | You, by hand | Provider credential sources, the tier tables, harness commands, machine-wide permission rules, tool enforcement, the server's bind address and port, the sandbox |
| Workspace settings | `<workspace>/.ostra/workspace.toml` | The Settings screen, or you | Projects, routing, custom instructions, permission rules, notifications, MCP servers |
| Project profile | `<project>/.ostra/project.toml` | The init flow, then you | The project's stack, its build, test, and format commands, the module map, skills, and review rules |
| Registry | `registry.db` in the data folder (`$OSTRA_DATA_DIR`, else `~/.local/share/ostra/` on Linux) | Ostra | Per workspace: the permission mode, the YOLO default, the spend limits, the sandbox mode, network choice, and extra allowed hosts, and the approvals of folder-file commands. Machine-wide: provider keys and base URLs saved from the browser |

The global config belongs to the machine. It never goes with a repository. It is the only place that contains
the model names for each tier. The workspace and project files are in folders that a `git pull` or an edit by an
agent can change. This is the reason for the registry.

## Why some settings stay out of the folder

A workspace file can come from a different person. If Ostra obeyed all of the file, a clone of a repository
could do these changes:

- Increase your budget.
- Turn off your sandbox.
- Change you to YOLO.
- Start a program that the author selects.

Two rules prevent this.

**Rule A2: control settings live in the registry.** Ostra reads these settings from the registry and never from
`workspace.toml`:

- The permission mode, the YOLO default, and the spend limits.
- The tool enforcement of the workspace.
- The sandbox mode, network choice, extra allowed hosts, decoy files, readable credentials, and macOS loopback
  settings of the workspace.

The overlay that enforces the rule is short, so this page quotes it
([`crates/ostra-workspace/src/trust.rs`](../../crates/ostra-workspace/src/trust.rs)):

```rust
// Rule A2: the permission mode, YOLO, spend limits, tool enforcement, and the sandbox mode,
// network, hosts, decoys, readable credentials, and loopback choice come from the registry, never from a folder file,
// because a repository could otherwise lift its own budget, turn off its own guards, open its own
// sandbox, read the user's credentials, or plant decoys that pause every session.
pub fn overlay(registry: &RegistryDb, root: &Path, s: &mut WorkspaceSettings) {
    let a = access(registry, root);
    s.permissions.mode = a.mode;
    s.yolo.default = a.yolo;
    s.limits = a.limits.unwrap_or_default();
    s.sandbox_mode = a.sandbox_mode;
    s.tool_enforcement = a.tool_enforcement;
    s.sandbox_network = a.sandbox_network;
    s.sandbox_allowed_hosts = a.sandbox_allowed_hosts;
    s.sandbox_decoys = a.sandbox_decoys;
    s.sandbox_readable = a.sandbox_readable;
    s.sandbox_loopback = a.sandbox_loopback;
    s.sandbox_blocked_ports = a.sandbox_blocked_ports;
}
```

Ostra ignores these entries when you write them into `workspace.toml` by hand:

- A `[limits]` table or a `yolo` table.
- A `tool_enforcement`, `sandbox_mode`, `sandbox_network`, `sandbox_allowed_hosts`, `sandbox_decoys`,
  `sandbox_readable`, `sandbox_loopback`, or `sandbox_blocked_ports` key.
- A `permissions.mode` key.

Ostra removes them the next time that it saves the file. Change them on the Settings screen.

**Rule A1: commands start only after you approved them.** Ostra holds back the parts of a folder file that
name a program. It holds them until the registry records your approval of that exact content, by hash. In
`workspace.toml`, these parts are the MCP servers, the `code_provider` and `language_servers` of each project,
and `permissions.allow`. In `project.toml`, the part is `commands.format`. This is the only project command that
Ostra runs itself and not through an agent. When a file waits for approval, these effects apply:

- No MCP server, language server, or code provider starts.
- Its allow rules do not apply. Its deny and ask rules still apply, because they only restrict.
- Ostra removes projects that point outside the workspace folder from the effective settings.
- Ostra records a format step as skipped, with no exit code.

The Settings screen shows the exact commands and an Approve button. It also shows the names of the environment
variables and headers that the commands use, but never the values. The approval contains the hash that the
browser showed. If the file changed between the display and the click, Ostra refuses the approval. When you save
through Ostra, an approved file stays approved with its new content, so your own edits never ask again. When you
save a file that still waits, it continues to wait. Thus a save cannot approve commands that you did not see.
See [the threat model](../security/threat-model.md) for the full security model.

The General tab edits two of the controls that Rule A2 keeps in the registry: YOLO and the limits. The
Permissions tab holds the other controls: the permission mode, and the mode, network choice, allowed hosts, and
decoy files of the sandbox. It also sets the tool enforcement of the workspace to one of three values:

- Enabled.
- Disabled.
- The global `tool_enforcement` key. The option shows the value of this key (`global_tool_enforcement` in the
  workspace response).

See [tool enforcement](../security/agent-containment.md#tool-enforcement) for the guards that it turns on. The
General tab:

![The General settings tab with the workspace name, YOLO, limits, and Delete workspace](../images/console/settings-general.png)

## The global config

A new machine runs with built-in defaults, so the file is optional. The defaults come from
`GlobalConfig::default()` in [`crates/ostra-core/src/config.rs`](../../crates/ostra-core/src/config.rs):

- **Providers.** `anthropic` reads `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, and `ANTHROPIC_BASE_URL`.
  `openai` reads `OPENAI_API_KEY` and `OPENAI_BASE_URL`. A key can also come from the OS keychain
  (`keychain_service`) or from the browser. Ostra keeps a key from the browser in the registry and never shows it
  again. Ostra looks for the key in this order: the environment, the saved key, the keychain. The base URL comes
  from `base_url` in this file, then `base_url_env`, then the saved URL.
- **Tier tables.** One table for each executor. The section below describes them.
- **Harness commands.** `claude`, `codex`, `grok`, and `agy`, found on `PATH`. `command` points to a different
  binary, and `args` adds arguments to each launch.
- **Permissions.** One deny rule, `Bash(rm -rf /*)`. Ostra merges the rules in this file with the rules of the
  workspace.
- **Server.** `127.0.0.1` on a free port. `bind`, `port`, `allowed_hosts`, and `use_ip_host` change this.
- **Sandbox.** `mode = "required"`. If Ostra cannot sandbox an execution, the execution does not start. See
  [OS compatibility](../platforms/os-compatibility.md) for the support on each platform. See
  [Agent containment](../security/agent-containment.md) for the functions of the sandbox.

Write full tables. A top-level table that you write, such as `[tiers.*]` or `[providers.*]`, replaces all of
that default table. Ostra does not merge it entry by entry. These examples show the result:

- A file with only `[tiers.native]` gives the harness executors no tier table. Thus validation fails for a
  workspace that routes an agent to Codex.
- A `[providers.anthropic]` table that sets only `api_key_env` does not read `ANTHROPIC_AUTH_TOKEN` or
  `ANTHROPIC_BASE_URL`. Also, the default `openai` entry is not there.

Harness commands are different in practice: a harness that is not in `[harness.*]` still launches by its own
name.

Ostra reads the global config again each time that it needs it. If the file does not parse, or if a sandbox path
in it is relative, Ostra continues to use the last version that worked. Ostra then writes a warning to the server
log. Thus a typo never changes your machine back to the defaults in the middle of a session.

## Workspace settings

`workspace.toml` is the file that the Settings screen edits. Its tables:

- `name` and `[[projects]]`: each project has a `key` (lowercase letters, digits, and dashes) and an absolute
  `path`. The optional `code_provider` and `[[projects.language_servers]]` supply code navigation in the Files
  view.
- `[routing.*]`: executor, model, and effort for each agent. The next section describes them.
- `[instructions]`: Ostra gives `all` to each agent, and `agents.<name>` to one agent. Ostra adds both to the
  spawn block of the agent, after the rules that the prompt already contains. Type `@` in either field to tag a
  project file or a workspace artifact. Each agent then gets the absolute path of the tagged item under the
  instruction. A tag that names a hidden or missing artifact fails at save time.
- `[permissions]`: `allow`, `ask`, and `deny` lists in the rule syntax of Claude Code, such as
  `Bash(npm run test *)`. Ostra merges them with the global rules, global first. Ostra parses each rule at save
  time.
- `[notifications]`: `push = true` sends Web Push for open gates and finished sessions.
- `[[mcp_servers]]`: external MCP servers, local (`command`) or remote (`url`). Each server has its own
  `disabled_tools`, an `agents` list, and a `timeout_secs` for each call from 1 to 600. A header or environment
  value can name a variable as `${VAR}`, so that secrets stay out of the file.

The Settings screen has a tab for each part. Projects shows the key, path, and stack of each project:

![The Projects settings tab with two projects](../images/console/settings-projects.png)

Instructions holds the text for all agents and for each agent:

![The Instructions settings tab with a workspace artifact tagged in the text for all agents](../images/console/settings-instructions.png)

Permissions holds the mode, the sandbox, and the rules. It shows the global rules as read-only:

![The Permissions settings tab with mode, sandbox, rules, and global rules](../images/console/settings-permissions.png)

Notifications turns on Web Push and subscribes this browser:

![The Notifications settings tab](../images/console/settings-notifications.png)

MCP servers edits `[[mcp_servers]]`. It shows the live state of each server and a switch for each tool
([MCP servers](mcp.md)):

![The MCP servers settings tab with three servers](../images/console/settings-mcp.png)

Two tabs hold state for the full machine and not for `workspace.toml`. Git saves credentials for clones and
pulls:

![The Git settings tab with the saved credentials table and the Add a git credential form](../images/console/settings-git.png)

Sign-in lists each browser that is signed in to this server:

![The Sign-in settings tab with two browser sessions](../images/console/settings-signin.png)

## The project profile

The inventory step of the init flow writes `project.toml`. The file describes one repository:

- Its stack.
- Its commands (`build`, `test`, `test_one`, `format`, `lint`, `typecheck`, `run`).
- Its test types, a module map, its skills, its conventions, and its review rules.

You can edit it by hand after the init flow.

Each `[test_types.<name>]` table describes one type of test that the repository runs, such as `unit`,
`integration`, or `e2e`. The table gives these items:

- The command for all tests of the type.
- The command for one test.
- The file globs that contain its tests.
- A note about what the tests need to run.

The detect step of the init flow finds the types from scripts, runner configs, and test directories. The test
stage can verify only at these levels. The execution path analyzer gives each check one of them, and write-test
runs each test with the command of its type. The repo brief gives both agents both commands of each type. The
first command runs the full level, and a regression run uses it. The second command runs a single test.

A project with only a `unit` type gets unit tests and a regression run. Ostra reports the flows that need a
running system as unverified. Thus, if the repository has integration or end-to-end tests that the detect step
did not find, add a type here.

Two details are important for behavior. First, Ostra itself runs only `commands.format`, after an implement step.
Thus only that command needs approval. Agents run the other commands, through the policy and the sandbox. Second,
a review rule with `auto_fixable` lets the engine apply the fix of a reviewer for that rule without another
implement pass. But this does not apply if the rule ID starts with `SEC-BLOCK` or `PHASE-REQ`. Those rules never
qualify, independent of the file (`ProjectProfile::auto_fixable_ids`).

Models write this file, so its parser gives clear explanations. If the initializer submits a `project.toml` that
does not parse, `check_profile` answers with the fix first, then the text of the parser:

```text
Fix /repo/.ostra/project.toml with the edit tool, then submit again, because Ostra cannot parse it.
Line 4: TOML has no null. Omit `test_framework` instead of writing `null`.
Line 9: `conventions` is one table. Write `[conventions]`, not `[[conventions]]`.
```

The fixes come first because some harnesses clip a long tool error. The message of the parser is the least
useful part.

The project screen shows the profile: its commands, skills, modules, and review rules:

![The backend project screen with commands, skills, modules, and review rules](../images/console/project.png)

## Routing: from agent to model

Ostra finds a route by its **route key**. The route key is the name of the agent (`explore`, `plan`,
`implementer`, and the other agents). For the small structured calls that the engine makes itself, the route
key is `judge`. Ostra resolves three items for each key.

The Routing tab of Settings has one row for each agent, the phase complexity table, and the native provider keys:

![The Routing settings tab with the per-agent table, the phase complexity table, and native providers](../images/console/settings-routing.png)

### Tiers are the indirection

Workspace settings almost never name a model. They name a **tier**: `fast`, `balanced`, `advanced`, or
`frontier`. The global config maps each tier to a model one time for each executor, because each executor uses
different model names:

| Executor | Tier table | Default `fast` / `balanced` / `advanced` / `frontier` |
| --- | --- | --- |
| `native` | `[tiers.native]` | `anthropic:claude-haiku-4-5-20251001` / `anthropic:claude-sonnet-5-5` / `anthropic:claude-opus-5-5` / `anthropic:claude-fable-5-1` |
| `harness:claude` | `[tiers.claude]` | `haiku` / `sonnet` / `opus` / `fable` |
| `harness:codex` | `[tiers.codex]` | `gpt-5.6-luna` / `gpt-5.6-terra` / `gpt-5.6-sol` / `gpt-5.6-sol` |
| `harness:grok` | `[tiers.grok]` | `grok-4.5` for every tier |
| `harness:agy` | `[tiers.agy]` | `flash` for every tier |

Native entries are `provider:model`, and the native providers are `anthropic` and `openai`. Harness entries are
the model slug of the CLI. Thus `plan = "advanced"` means Opus when the planner runs natively. It means
`gpt-5.6-sol` when the planner runs in Codex. You can move an agent to a different executor and not change its
model route.

### Executor

`[routing.executor]` selects what runs the agent: `native` or `harness:<claude|codex|grok|agy>`. An agent with
no entry runs natively. You cannot route two keys to a harness:

- `judge`, because a judge call is a single structured request from the engine and not an agent execution.
- `quick-answer`, the agent of the side panel.

### Model

`[routing.model]` gives each route key one of four value forms:

| Form | Example | Meaning |
| --- | --- | --- |
| Tier | `plan = "advanced"` | Find the tier in the tier table of the executor |
| `default` | `plan = "default"` | Use the `default_tier` from the `agent.toml` of the agent |
| Concrete model | `plan = "anthropic:claude-opus-5-5"` | Use this model as written, on the executor that runs the agent |
| Per executor | `plan = { native = "advanced", codex = "gpt-5.6-sol" }` | Use the entry for the executor that runs the agent. Each entry is a tier or a model |

Each route key must have a model route. Ostra gives a new workspace one route for each key
(`WorkspaceSettings::seeded`). These routes come from the inventory profile of Ultracode:

- Research, spec, plan, fact-check, documentation, system architecture, prompt generation, and the advisor use
  `advanced`.
- Review, execution-path analysis, the initializer, and quick answers use `balanced`.
- Judges use `advanced`, because a wrong route costs more than the call.

A workspace that you saved before an agent existed has no route for that agent. Thus an Ostra update that adds
an agent (for example, the advisor) gives that workspace a validation problem. The Settings screen then refuses
to save until you fix the problem. Ostra does not add the route itself, because the user selects the route. But
Ostra offers the fix:

- The workspace detail lists, under `fixes`, a `default` route for each route key that has no route
  (`keys_without_route` in `crates/ostra-core/src/config.rs`). `default` resolves to the default tier of the
  agent.
- The settings banner of the dashboard has a button that applies these routes through
  `POST /api/workspaces/:ws/settings/fix`. This request writes only those routes. The user must fix all other
  problems.
- On the Settings screen, the same fix is a button next to the problem. It changes the form, so Ostra saves it
  with the other edits.

A route for an agent that Ostra replaced stays valid, and Ostra ignores it (`RETIRED_AGENTS` in
`crates/ostra-core/src/agent.rs`). Thus a workspace saved with `module-documentation` still loads. Its
replacements, `documentation` and `system-architecture`, must have their own routes. The same fix supplies them.

### Effort

`[routing.effort]` sets the reasoning effort: `low`, `medium`, `high`, `xhigh`, or `max`. An agent with no entry
keeps the effort that its `agent.toml` gives for that executor. For example, the `agent.toml` of the implementer
asks for `high` on each executor. Each executor changes the level into the value that its model or CLI accepts.

The native executor spends all that the selected level allows, because the user selected it and knew the cost.
Each request asks for the full output limit of the model, from the models.dev catalog. If the catalog does not
list the model, the limit is 32,000 tokens. Thus the request never stops thinking early at a high level. The
Anthropic provider keeps each request at that limit. On Anthropic models, the level becomes thinking in one of
two ways ([`crates/ostra-providers/src/anthropic.rs`](../../crates/ostra-providers/src/anthropic.rs)):

- **Adaptive thinking** with `output_config.effort` on Claude 4.6 and each later model.
  - From Claude 4.7, it is the only thinking mode, because a fixed budget returns a 400.
  - Opus 4.6 and Sonnet 4.6 accept both modes and stay adaptive. The reason is that on Opus 4.6, only adaptive
    thinking reasons between tool calls.
  - The 4.6 models have no `xhigh`, so on these models `xhigh` becomes `high`.
- **A fixed thinking budget** on the models that accept nothing else (Claude Haiku 4.5, Sonnet 4.5, Opus 4.5,
  and earlier).
  - The budget is a part of the output limit of the model, from the models.dev list. It is one eighth at
    `medium`, one quarter at `high`, and half at `xhigh`.
  - At `max`, the budget is all but 16,000 tokens, which stay for the answer. At `low`, the model does not
    think.
  - For example, Haiku 4.5 at `high` thinks against 16,000 of its 64,000 tokens.
  - Only Opus 4.5 also accepts the effort itself.
  - With tools, Sonnet 4.5, Opus 4.5, and the Claude 4.0 and 4.1 models think between tool calls through the
    interleaved thinking beta. Their budget then covers the full turn.
  - Haiku 4.5 cannot do this, so it thinks only at the start of a turn, before its first tool call.

### Phase complexity

Two agents run one time for each plan phase: the implementer and write-test. Each phase file has a
`**Complexity:**` line. You can route these two agents by this line under `byPhaseComplexity`, for executor,
model, and effort. `byPhaseComplexity` has priority over `byAgent`. Work with no phase file, such as a quick
change, counts as `low`. The seeded settings run both agents on `fast` for `low` and `medium` phases and on
`balanced` for `high` phases. Thus the model increases with the phase, and not every phase pays for the largest
model.

### Overrides the engine applies

Settings are not the only input. The engine forces some choices. It records each one, so that the session can
show the reason:

- **Generate-skill runs on `advanced`.** The generate-skill mode of the initializer forces the tier,
  independent of the route of the initializer. The route sets the other modes of the initializer.
- **Harness failures can fall back to native.** When a harness execution fails, its gate offers a `native`
  answer. If you select it, Ostra records the agent in the `native_fallback` set of the session. Each later
  execution of that agent in the session then runs natively.
- **Quick changes and quick answers run natively**, because a harness start costs more than the change.

### A worked resolution

This example uses the workspace excerpt below and an implementer spawn for a phase with
`**Complexity:** high`:

```toml
[routing.executor.byAgent]
implementer = "harness:codex"

[routing.executor.byPhaseComplexity.implementer]
high = "native"

[routing.model.byPhaseComplexity.implementer]
low = "fast"
medium = "fast"
high = "balanced"
```

1. **Executor.** `byPhaseComplexity.implementer.high` exists and gives `native`, so it has priority over the
   `byAgent` entry. Low and medium phases go to Codex.
2. **Model.** `byPhaseComplexity.implementer.high` gives `balanced`, a tier.
3. **Tier table.** The executor is native, so Ostra reads `[tiers.native].balanced`:
   `anthropic:claude-sonnet-5-5`.
4. **Check.** A native model must be `anthropic:<model>` or `openai:<model>`. It is.
5. **Effort.** There is no effort route, so the `agent.toml` value of the implementer for `native` applies:
   `high`.

The result is a `ResolvedRoute { executor: native, model: "anthropic:claude-sonnet-5-5", tier: balanced }`.
This is `resolve_route` in [`config.rs`](../../crates/ostra-core/src/config.rs). The same function runs at save
time and at spawn time.

## Validation at save time

A route that does not resolve is a validation error when you save. It is never a silent fallback when an agent
starts. `validate_workspace` enforces this. A save goes through `PATCH /api/workspaces/{ws}`. The Settings screen
also calls `POST /api/workspaces/{ws}/validate` when you type. Both return all problems at the same time. Each
problem has the dotted path of its field:

```json
{
  "error": "...",
  "issues": [
    { "path": "routing.model.byAgent.plan",
      "message": "`plan` routes to tier `frontier` on `harness:grok`, but `[tiers.grok]` in the global config has no `frontier` entry" }
  ]
}
```

Validation checks these items:

- **Every route key resolves, on every complexity it can run at.** Ostra resolves the implementer and write-test
  three times, one time for each complexity. Without this check, a gap in the `high` row shows only when a high
  phase arrives.
- **The executor exists on this machine.** A route to a harness that does not have its CLI installed is an
  error.
- **The provider has a key.** A native route to `openai:...` with no usable OpenAI key is an error.
- **Keys name real agents.** Ostra reports a typo such as `implementor` under `byAgent`, and does not ignore it.
  Ostra accepts `byPhaseComplexity` effort entries only for the two agents that run for each phase.
- **Forbidden routes.** You cannot send `judge` and `quick-answer` to a harness.
- **Projects.** The checks are:
  - Keys are correct in form and unique.
  - Paths are absolute and are folders.
  - A stack name is valid.
  - Code providers and language servers name a program.
  - Each language has a maximum of one server.
  - Timeouts are from 1 to 120 seconds.
- **Permission rules parse** (`ostra_policy::validate_rule`).
- **MCP servers** have unique, valid names, exactly one of `command` or `url`, and a timeout in range.
- **Limits.** `max_parallel_executions` is 1 or more, and `session_budget_usd` is a finite amount of 0 or more.
  See [Spend and limits](spend-and-limits.md).

If an issue remains, Ostra writes nothing. Ostra saves the file atomically, through a temporary file and a
rename. Thus a crash during a save never leaves half a file.

The Settings screen runs the same checks when you edit. In this example, the implementer routes to a harness
that is not installed. Thus the row shows the issue, and the header shows the count of problems to fix before
the save:

![The Routing tab with an issue on the implementer row and 1 problem to fix before saving](../images/console/settings-validation.png)

### What save-time validation cannot see

Validation checks the settings against the machine at the time of the save. The machine can change after the
save. For example, you edit the global config by hand, remove a CLI, or unset an API key. Ostra then does not
select a substitute. The spawn resolves its route and fails. Ostra records it as a denied execution with the
reason, for example ``route: `plan` has no model route`` or `The harness:codex executor is not available.`
The session shows that failure in a place where you can act on it. Thus Ostra does not silently run a model that
you did not select.

One case falls back and does not fail. If `workspace.toml` itself does not parse, the engine runs with the
seeded defaults for that workspace until the file parses again. Fix the file, or save from the Settings screen,
which writes the file again.

## Read fresh, every execution

Ostra has no settings cache that needs a restart. The engine gets settings only through its `Services` trait.
The implementation of the server reads the files each time that the engine asks:

- `Services::workspace()` loads `workspace.toml` and applies the registry overlay and the approval filter.
- `Services::global()` loads the global config, with the last-good fallback that the section above describes.
- Each spawn reads `project.toml` and the project inventory from disk when it builds the input of the agent.
- The planner reads the session budget on each planning pass. The slot limiter reads the parallel limit each
  time that a spawn waits.

Thus an edit applies to the next execution, not to the execution that already runs. A running execution keeps
the route that it started with. Its record shows the executor and model that it used. A settings change does not
change the data that the event log of the session already contains. A gate answer, a fallback to native, or a
raised budget stays as Ostra recorded it. The reason is that the state of the session is a fold over its log.
See [The event log](event-log.md).

## An annotated example

The two files below parse with the types of Ostra. They pass `validate_workspace` on a machine that has Codex
installed and an Anthropic key set. The global file writes only the tier tables that this workspace uses. Thus a
route to Claude Code, Grok Build, or Antigravity needs their tables too. The global file also replaces the
`providers` table, so OpenAI models are not available with it.

```toml
# ~/.config/ostra/config.toml

[providers.anthropic]
api_key_env = "ANTHROPIC_API_KEY"   # the environment wins over a key saved in the browser
base_url_env = "ANTHROPIC_BASE_URL" # for a gateway; unset means api.anthropic.com

# Tier names used by the workspace resolve here, per executor.
[tiers.native]
fast = "anthropic:claude-haiku-4-5-20251001"
balanced = "anthropic:claude-sonnet-5-5"
advanced = "anthropic:claude-opus-5-5"
frontier = "anthropic:claude-fable-5-1"

[tiers.codex]
fast = "gpt-5.6-luna"
balanced = "gpt-5.6-terra"
advanced = "gpt-5.6-sol"
frontier = "gpt-5.6-sol"

[harness.codex]
command = "/opt/codex/bin/codex"    # a binary that is not on PATH

[permissions]
deny = ["Bash(rm -rf /*)", "Bash(git push --force *)"]  # applies to every workspace

[sandbox]
mode = "required"                   # refuse to run an execution that cannot be sandboxed
network = "allowlist"               # registries, source hosts, model APIs, and allowed_hosts
allowed_hosts = ["mirror.corp.example"]
extra_hidden = ["~/.aws"]           # absolute or ~/ only; a relative path is rejected
extra_readable = ["~/.m2/settings.xml", "~/.netrc"]  # credentials agents may read, read-only
```

```toml
# /home/me/shop/.ostra/workspace.toml
name = "shop"

[[projects]]
key = "backend"
path = "/home/me/shop/backend"

# Codex builds low and medium phases; high phases run natively.
[routing.executor.byAgent]
implementer = "harness:codex"

[routing.executor.byPhaseComplexity.implementer]
high = "native"

[routing.model.byAgent]
explore = "advanced"
generate-spec = "advanced"
plan = { native = "frontier", codex = "gpt-5.6-sol" }  # per executor
fact-check = "advanced"
code-reviewer = "balanced"
execution-path-analyzer = "balanced"
documentation = "default"    # the agent.toml default tier
system-architecture = "default"
prompt-generation = "advanced"
initializer = "balanced"
judge = "advanced"
quick-answer = "balanced"
advisor = "advanced"

[routing.model.byPhaseComplexity.implementer]
low = "fast"
medium = "fast"
high = "balanced"

[routing.model.byPhaseComplexity.write-test]
low = "fast"
medium = "fast"
high = "balanced"

[routing.effort.byAgent]
plan = "high"

[routing.effort.byPhaseComplexity.implementer]
high = "xhigh"

[instructions]
all = "Write British English in comments and docs."

[instructions.agents]
implementer = "Keep functions under 40 lines."

[permissions]
allow = ["Bash(npm run test *)"]    # waits for approval if this file changed outside Ostra
deny = ["Bash(git push *)"]
```

This example does not include some settings on purpose: the permission mode, YOLO, limits, and the sandbox mode
and hosts of the workspace. You set them on the Settings screen, and Ostra keeps them in the registry (Rule A2).

## Where to look in the code

| What | Where |
| --- | --- |
| Settings types, defaults, `resolve_route`, `validate_workspace`, `check_profile` | [`crates/ostra-core/src/config.rs`](../../crates/ostra-core/src/config.rs) |
| Tiers, effort levels, complexity | [`crates/ostra-core/src/model.rs`](../../crates/ostra-core/src/model.rs) |
| Executor names and parsing | [`crates/ostra-core/src/executor.rs`](../../crates/ostra-core/src/executor.rs) |
| Registry overlay and approvals (Rules A1, A2) | [`crates/ostra-workspace/src/trust.rs`](../../crates/ostra-workspace/src/trust.rs) |
| Extra checks beyond `validate_workspace` | [`crates/ostra-workspace/src/settings.rs`](../../crates/ostra-workspace/src/settings.rs) |
| Machine facts for validation | `WorkspaceHost::environment` in [`crates/ostra-server/src/app.rs`](../../crates/ostra-server/src/app.rs) |
| Saving settings | `save_settings` in [`crates/ostra-workspace/src/runtime.rs`](../../crates/ostra-workspace/src/runtime.rs) |
| Route resolution at spawn time | `perform_spawn` in [`crates/ostra-engine/src/runner.rs`](../../crates/ostra-engine/src/runner.rs) |
| Effort and instructions reaching the agent | [`crates/ostra-engine/src/factory.rs`](../../crates/ostra-engine/src/factory.rs) |
| The design brief for settings | [HANDOVER section 7](../../HANDOVER.md#7-settings) |
