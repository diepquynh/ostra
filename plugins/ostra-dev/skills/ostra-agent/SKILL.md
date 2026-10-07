---
name: ostra-agent
description: Write a custom Ostra agent as one Markdown file, `<workspace>/.ostra/agents/<name>.md`, with TOML frontmatter between `+++` lines and a prompt body. Use when the user asks to add, change, or debug an Ostra agent, for example an agent for a workflow stage, a helper that other agents start, or an agent that replaces a built-in agent such as the code reviewer or the spec writer. For an agent whose work is Rust code, use the ostra-plugin skill. To put the agent in a workflow, use the ostra-workflow skill after this one.
---

# Write an Ostra agent

An Ostra agent is a model with a prompt, a set of tools, and a result contract. A custom agent is one Markdown
file in the `.ostra/agents/` folder of a workspace. Ostra treats it the same as its own 13 agents: it can fill a
custom workflow stage, run as a helper, or replace the agent behind a built-in stage.

## Steps

1. Find the workspace folder. It holds `.ostra/workspace.toml`. If you find no such folder, ask the user for the
   path.
2. Select the name: lowercase kebab-case, at most 40 characters, starting with a letter. Do not use the name of a
   built-in agent (see [Result contracts](#result-contracts)), `module-documentation`, or `judge`. Read the file
   names in `.ostra/agents/` and select a name that is not there.
3. Select the result contract (`returns`). Use [Select the contract](#select-the-contract).
4. Select the capabilities and the write scope. Give only the tools that the job needs. Use
   [Capabilities](#capabilities) and [Write scopes](#write-scopes).
5. Write `.ostra/agents/<name>.md`: the frontmatter, then the prompt body. Use [The file](#the-file) and
   [The prompt body](#the-prompt-body).
6. If the user wants the agent in a workflow, use the ostra-workflow skill. An agent that returns `stage` runs in
   an `agent = "<name>"` node. An agent that returns a built-in contract replaces the standard agent through
   `[agents]`.
7. Tell the user to approve the workspace file in the Settings of the Ostra console. A file that you add or
   change outside Ostra makes the workspace file wait for approval again. Until the user approves it, no session
   runs the agent.
8. Tell the user to read the Settings issues under `agents`. Ostra reports there a file that does not parse, a
   name that is used two times, a bad template token, and a contract that no plugin defines.

## Select the contract

| The agent must | `returns` |
| --- | --- |
| Do one step of a workflow and give a verdict: an audit, a check, release notes, a design review | `stage` (the default) |
| Replace the agent of Ostra behind a built-in stage, for example a stricter reviewer | The built-in contract of that agent, for example `review` |
| Return the result shape of a plugin, which the plugin then judges | `<plugin>:<contract>` |

Use `stage` if the request does not name a built-in stage. An agent with a built-in contract must submit every
field of that contract. Read the prompt of the standard agent for the fields: `assets/agents/<agent>/prompt.md`
in the Ostra source, or the agent page in the console, which also shows the submit schema.

## The file

```markdown
+++
description = "Audits the changed files for secrets and unsafe input handling. Read-only on project source."
default_tier = "balanced"
capabilities = ["read", "search_text", "glob", "shell", "coordinate"]
write_scope = "session"
timeout_seconds = 1200

[effort]
native = "high"

[data_schema]
type = "object"
required = ["risk"]

[data_schema.properties.risk]
type = "string"
enum = ["low", "medium", "high"]
+++
You audit the change of one project for secrets and unsafe input handling.
...
```

Put the tables (`[effort]`, `[data_schema]`) after all keys, because TOML puts each key after a table header into
that table. Only `description` is necessary:

| Field | Values | Default |
| --- | --- | --- |
| `description` | When the agent runs and what it can change. The console shows it. | Necessary |
| `name` | Lowercase kebab-case, at most 40 characters | The file name without `.md` |
| `returns` | A contract from [Result contracts](#result-contracts), or `<plugin>:<contract>` | `stage` |
| `default_tier` | `fast`, `balanced`, `advanced`, or `frontier` | `balanced` |
| `capabilities` | Names from [Capabilities](#capabilities) | `read`, `search_text`, `glob`, `report`, `coordinate` |
| `write_scope` | `read_only`, `session`, `project`, or `setup` | `project` with `write` or `edit`, else `session` |
| `brief` | Sections of the repo brief: `stack`, `commands`, `testing`, `skills`, `conventions`, `review`, `modules` | `stack`, `commands`, `skills`, `conventions`, `modules` |
| `timeout_seconds` | 1 to 7200 | 1200 |
| `helper` | `true` lets `SendMessage` start the agent as a helper | `false` |
| `[effort]` | Keys `native`, `claude`, `codex`, `grok`, `agy`. Values `low`, `medium`, `high`, `xhigh`, `max`. | `high` for each executor |
| `[data_schema]` | The JSON Schema of `data` in the submit, as TOML tables. Only with `returns = "stage"`. | None |

Ostra refuses an unknown key, `read_only` together with `write` or `edit`, and a file larger than 128 KiB. The
route of the agent is `default_tier` on its executor, unless the workspace sets `routing.*.byAgent.<name>`.

## Capabilities

| Capability | Tool or right | Prompt token |
| --- | --- | --- |
| `read` | Read a file | `{{ tool_read }}` |
| `write` | Write a file | `{{ tool_write }}` |
| `edit` | Edit a file | `{{ tool_edit }}` |
| `shell` | Run a shell command | `{{ tool_shell }}` |
| `search_text` | Search file contents | `{{ tool_search_text }}` |
| `glob` | Find files by pattern | `{{ tool_glob }}` |
| `skill` | Load a skill of the project | `{{ tool_skill }}` |
| `web_search` | Search the web | `{{ tool_web_search }}` |
| `web_fetch` | Fetch a URL | `{{ tool_web_fetch }}` |
| `report` | Write the report file that the spawn block names. Only the runs of built-in stages get one, so a `stage` agent writes its report with `write`. | `{{ tool_report }}` |
| `memory` | Save a lesson to the project memory | `{{ tool_memory }}` |
| `memory_recall` | Read lessons from the project memory | `{{ tool_memory_recall }}` |
| `docs_search` | Search the documentation books of the workspace | `{{ tool_docs_search }}` |
| `code` | The code index: outline, find, callers, callees, implementations, neighbors, impact, map | `{{ tool_code_find }}` and the other `tool_code_*` tokens |
| `coordinate` | `ListAgents`, `SendMessage`, `WaitForMessage` | `{{ tool_list_agents }}`, `{{ tool_send_message }}`, `{{ tool_wait_for_message }}` |
| `manage_projects` | List and create projects | `{{ tool_project_list }}`, `{{ tool_project_create }}` |
| `document_research`, `document_spec`, `document_plan` | The Document tool for that document, and the right to write it | `{{ tool_document }}` |
| `review_ledger` | The right to write the ledger of a review loop | None |
| `security_block` | The right to write the security block file | None |
| `progress_log` | The right to write the progress log of the implementer | None |
| `test_files` | The right to write test files and test folders in the repo | None |

The policy of Ostra checks each tool call. Each capability is only a first filter.

## Write scopes

| Scope | The agent can write |
| --- | --- |
| `read_only` | No file |
| `session` | Its session folder and the temp folder of the OS |
| `project` | The repo root and its session folder |
| `setup` | The `.ostra/` runtime of the project and its skills folder |

Select `session` for an agent that only reads code and writes a report. Select `project` only for an agent that
changes code. No agent can write `.ostra/agents/`, `.ostra/workflows/`, or `.ostra/transforms/`.

## Result contracts

| Contract | Standard agent | Built-in stages that read it |
| --- | --- | --- |
| `stage` | None | Custom workflow stages |
| `research` | `explore` | `ostra:research` |
| `spec` | `generate-spec` | `ostra:spec` |
| `fact-check` | `fact-check` | `ostra:spec`, `ostra:plan`, `ostra:book` |
| `plan` | `plan` | `ostra:plan` |
| `implementation` | `implementer` | `ostra:build`, `ostra:closing` |
| `review` | `code-reviewer` | `ostra:build`, `ostra:closing` |
| `prompt` | `prompt-generation` | `ostra:build` |
| `advice` | `advisor` | `ostra:build`, `ostra:closing` |
| `path-analysis` | `execution-path-analyzer` | `ostra:closing` |
| `tests` | `write-test` | `ostra:closing` |
| `documentation` | `documentation` | `ostra:closing`, `ostra:book` |
| `answer` | `quick-answer` | None (the side-panel answers) |
| `setup` | `initializer` | None (project setup) |

## The `stage` submit

An agent that returns `stage` ends with one call to the submit tool. Ostra adds a short guide to the submit before
the prompt body, so the body does not have to explain the call. The fields are:

| Field | Meaning |
| --- | --- |
| `verdict` | `pass`, `fail`, or `needs_user`. The workflow continues, follows the `on_fail` of the node, or asks the user. |
| `summary` | What the run did and found. Necessary, and not empty. |
| `findings` | Each problem, with `description`, `file`, and `fix` |
| `question`, `options` | Necessary with `needs_user`. Put the recommended option first. |
| `report_path` | A report file that the run wrote |
| `data` | Output in the shape of `data_schema`. Necessary when the agent declares a schema. |

Ostra checks `data` with these JSON Schema keywords only: `type`, `properties`, `required`, `items`, `enum`,
`minItems`, `maxItems`, and `additionalProperties`. Ostra loads other keywords but checks nothing with them.
Later workflow nodes read `data` as `<node>.data.<field>` or `<node>.output.<field>`, and Ostra checks each
reference against `data_schema` at save time. Declare every field that a later node reads.

## The prompt body

Ostra renders the body with minijinja in strict mode, one time when it loads the file and again for each run.

- Name a tool only through its token, for example "Read each changed file with {{ tool_read }}." The token becomes
  the name of the tool on the executor of the run: native Ostra, Claude Code, Codex, Grok Build, or
  Antigravity. A literal tool name such as `Read` is wrong on some executors.
- Use only the tokens in [Capabilities](#capabilities), `{{ tool_submit }}`, and `{{ assets_dir }}`. An unknown
  token is a settings error.
- Put literal `{{` or `{%` text, for example a template example, between `{% raw %}` and `{% endraw %}`.
- Do not repeat what Ostra adds before the body: the output rules, the stage contract guide, the messaging
  guide (with `coordinate`), and the code tools guide (with `code`).
- Do not describe the spawn block. Ostra sends the stage, the round, the request, the instructions of the node,
  the inputs, the spec, the plan, and the results of earlier stages in the first message.
- Write steps in order, one instruction for each sentence, in the imperative. Say what proves a `pass`, because
  the workflow moves on or stops on the verdict.
- For a built-in contract, describe each field of that contract and when to set it.

## Example

```markdown
+++
description = "Writes release notes for the change of the session. Writes only its report."
default_tier = "fast"
capabilities = ["read", "write", "shell", "search_text", "glob"]
write_scope = "session"
timeout_seconds = 600

[data_schema]
type = "object"
required = ["breaking"]

[data_schema.properties.breaking]
type = "boolean"
+++
You write the release notes for the change of this session.

1. Get the changed files with {{ tool_shell }}: `git diff --stat HEAD`.
2. Read each changed file that a user of the project sees with {{ tool_read }}.
3. Write the notes to `release-notes.md` in your session folder with {{ tool_write }}: one line for each change
   that a user sees, in the words of a user. Set `report_path` to that file.
4. Set `data.breaking` to `true` if a change removes or renames a public item.

Set `verdict` to `pass` when the report lists each change that a user sees. Set `verdict` to `needs_user` when
you cannot tell if a change is breaking, and ask the question in `question`.
```

## Check your work

Before you finish, make sure that each item is true:

- The file starts with `+++` on line 1, and a second `+++` line ends the frontmatter.
- The frontmatter has `description`, and it has no key outside the field table.
- The tables come after all keys.
- Each tool that the body names has its capability and uses its token.
- The write scope is the narrowest scope that works, and it is not `read_only` with `write` or `edit`.
- `data_schema` declares each field that a later node reads, and the agent returns `stage`.
- The body tells the agent what proves each verdict.
- You told the user to approve the workspace file in Settings.
