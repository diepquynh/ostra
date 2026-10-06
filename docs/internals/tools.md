# Tools

An agent can touch a file, the shell, the web, the memory of the project, or the engine only through a tool.
This page describes each tool that Ostra gives to an agent and what the tool does. It also tells how an agent
gets its tools.

Two ideas control the design:

- **The names are the names of Claude Code.** The native tools of Ostra are `Read`, `Write`, `Edit`, `Bash`,
  `Grep`, `Glob`, `Skill`, `WebSearch`, and `WebFetch`. They have the same input fields as the Claude Code tools
  with those names. The agent prompts were written and tuned for those tools. Ostra uses the same shape, so the
  prompts work on both executors. The policy also judges each call in that shape, from each executor.
- **Tools do the work, and the policy decides.** No code in `ostra-tools` checks permissions or guards. The
  executor runs `ExecutionPolicy::check` before a tool runs (see [Executors](executors.md)). A tool gets only a
  call that the policy allowed. Thus each tool stays small, and all rules are in one place:
  [Agent containment](../security/agent-containment.md).

## How an agent gets its tools

Agents do not ask for tools by name. The `agent.toml` of each agent lists **capabilities**. Each executor maps
a capability to its own tool:

```toml
# assets/agents/implementer/agent.toml
capabilities = ["read", "edit", "write", "shell", "search_text", "glob", "skill", "memory_recall",
                "memory", "report", "code", "manage_projects", "coordinate", "docs_search",
                "review_ledger", "progress_log"]
```

| Capability | Native tool | Claude Code | Codex | Grok Build | Antigravity |
| --- | --- | --- | --- | --- | --- |
| `read` | `Read` | `Read` | `exec_command` | `read_file` | `view_file` |
| `write` | `Write` | `Write` | `apply_patch` | `search_replace` | `write_to_file` |
| `edit` | `Edit` | `Edit` | `apply_patch` | `search_replace` | `replace_file_content` |
| `shell` | `Bash` | `Bash` | `exec_command` | `run_terminal_command` | `run_command` |
| `search_text` | `Grep` | `Grep` | `exec_command` | `grep` | `grep_search` |
| `glob` | `Glob` | `Glob` | `exec_command` | `list_dir` | `find_by_name` |
| `skill` | `Skill` | `Read` on the SKILL.md | `exec_command` on it | `read_file` on it | `view_file` on it |
| `web_search` | `WebSearch` | `WebSearch` | `web_search` | `web_search` | `search_web` |
| `web_fetch` | `WebFetch` | `WebFetch` | `web_search` | `web_fetch` | `read_url_content` |
| `report` | `Report` | `mcp__ostra__report` | `report` | `report` | `report` |
| `document_research`, `document_spec`, `document_plan` | `Document` | `mcp__ostra__document` | `document` | `document` | `document` |
| `memory` | `Memory` | `mcp__ostra__memory` | `memory` | `memory` | `memory` |
| `memory_recall` | `MemoryRecall` | `mcp__ostra__memory_recall` | `memory_recall` | `memory_recall` | `memory_recall` |
| `docs_search` | `DocsSearch` | `mcp__ostra__docs_search` | `docs_search` | `docs_search` | `docs_search` |
| `code` | `CodeOutline`, `CodeFind`, ... | `mcp__ostra__code_*` | `code_*` | `code_*` | `code_*` |
| `manage_projects` | `ProjectList`, `ProjectCreate` | `mcp__ostra__project_list`, `mcp__ostra__project_create` | `project_*` | `project_*` | `project_*` |
| `coordinate` | `ListAgents`, `SendMessage`, `WaitForMessage` | `mcp__ostra__list_agents`, ... | `list_agents`, ... | `list_agents`, ... | `list_agents`, ... |

The table is `assets/tool-mapping.toml`. The prompt renderer reads it. The renderer writes the prompt of each
agent with the tool names of the executor that the agent runs on. Thus the prompt tells an implementer on Codex
to edit with `apply_patch`, and it tells an implementer on the native loop to use `Edit`. Some capabilities work
differently in a CLI. For these capabilities, the renderer adds a short instruction. An example is how to load
a skill in a CLI that has no skill tool.

The table has two results:

- **On a harness, the file and shell tools are the tools of the CLI.** Ostra does not replace the `Read` tool
  of Claude Code or the `apply_patch` tool of Codex. Ostra checks each call through the hook bridge and lets
  the CLI run it. Before the policy sees a call, the adapters change it back into the canonical shape. Thus the
  policy judges `apply_patch` as writes to the files that the patch touches.
- **The tools of Ostra come from Ostra on all executors.** These tools are Ostra code on each executor:
  `Report`, `Document`, `Memory`, `MemoryRecall`, `DocsSearch`, the code navigation tools, the project
  management tools, the submit tool, and the workspace MCP tools. The native loop calls them directly. A
  harness gets them through the `ostra` MCP server.

Capabilities set the upper limit. A reviewer has no `write` capability, so it has no `Write` tool on any
executor. Then the policy sets more limits on what the tools of an agent can touch.

### Who gets what

| Agent | Capabilities |
| --- | --- |
| explore | read, shell, search_text, glob, web_search, web_fetch, memory_recall, memory, document_research, code, coordinate, docs_search |
| generate-spec | read, shell, search_text, glob, web_search, web_fetch, document_spec, code, coordinate, docs_search |
| fact-check | read, write, shell, search_text, glob, web_search, web_fetch, code, coordinate, docs_search |
| plan | read, shell, search_text, glob, document_plan, code, coordinate, docs_search |
| implementer | read, edit, write, shell, search_text, glob, skill, memory_recall, memory, report, code, manage_projects, coordinate, docs_search, review_ledger, progress_log |
| write-test | read, edit, write, shell, search_text, glob, skill, memory_recall, memory, report, code, coordinate, docs_search, review_ledger, test_files |
| code-reviewer | read, shell, search_text, glob, code, coordinate, docs_search, review_ledger, security_block |
| execution-path-analyzer | read, shell, write, search_text, glob, report, code, docs_search |
| documentation | read, shell, search_text, glob, code, docs_search |
| prompt-generation | read, edit, write, shell, search_text, glob, skill, report, code, test_files |
| initializer | read, write, edit, shell, search_text, glob, code, test_files |
| quick-answer | read, search_text, glob, web_search, web_fetch, memory_recall, code, docs_search |
| advisor | read, shell, search_text, glob, web_search, web_fetch, memory_recall, docs_search |

A custom agent lists its own capabilities in its definition. If it has no list, it gets read, search_text, glob,
report, and coordinate. Each agent can request each capability, because no capability is reserved (Rule CA6).
This includes the document grants, the ownership grants (`review_ledger`, `security_block`, `progress_log`,
`test_files`), and `manage_projects`. The user approves the workspace file, and this approval lets the agent
hold them. The ownership grants add no tool. They only let the guards accept more writes from the agent.

The implementer holds `manage_projects` by default. Each agent that holds it can create a project, if the agent
runs a phase in a project that the plan names as new. Each agent also gets its own `submit_<agent>` tool and
the MCP tools of the workspace. [Agents](agents.md) tells what each agent is for.

## How the policy sees each tool

The permission layer puts tools into families. The family sets which rules apply to a call:

| Family | Tools | How a call is judged |
| --- | --- | --- |
| Read | `Read`, `Grep`, `Glob` | Allowed, unless a `deny` or `ask` rule names the path. Rules in the style of `Read(~/.ssh/**)` apply. |
| Edit | `Write`, `Edit`, and harness equivalents (`MultiEdit`, `NotebookEdit`, `ApplyPatch`) | The policy judges each target path. The session dir and the OS temp dir are allowed. Other paths follow the rules, then the permission mode. `acceptEdits` allows edits inside the project. The mode judges a path that runs code later, also inside the project. |
| Bash | `Bash` | Tree-sitter-bash parses the command. The policy matches each simple command separately, so `npm test && curl evil` does not pass on `Bash(npm test *)`. If Ostra cannot parse a command, or the command uses shell syntax that Ostra does not model, the policy asks. Plan mode refuses it. |
| Other (opaque shells) | `PowerShell`, `Cmd` (Windows only) | Not parsed. Refused when the text names the files of Ostra, the engine state, or a credential store. Else the policy matches it against `PowerShell(...)` and `Cmd(...)` rules, then the mode. The mode never allows one without an ask. |
| WebFetch | `WebFetch` | Matched against `WebFetch(domain:...)` rules. If no rule matches, the policy asks, but not in bypass mode. |
| Other (changes Ostra) | `ProjectCreate` | Always asks, in each mode, also in bypass mode. No allow rule can replace the answer (rule O1). A `deny` rule refuses it, and plan mode refuses it. Only YOLO answers the ask. |
| Other | `Skill`, `WebSearch`, `Report`, `Document`, `Memory`, `MemoryRecall`, `DocsSearch`, `ProjectList`, the code tools, `submit_*`, workspace MCP tools | The tools of Ostra are allowed. The policy judges `Skill` with a `path` as a Read of that file. Workspace MCP tools are allowed if no rule says differently. Plan mode allows only the MCP tools that their server marks read-only (rule M1). |

The guards run before the family rules, on each family. The guards are write scope, state ownership, the
report path, the lesson gate, the build streak, self-protection, and the management tools guard. Write scope,
the report path, and most of self-protection run only when tool enforcement is on.
[Agent containment](../security/agent-containment.md) describes them.

The Permissions tab of Settings sets the mode and the allow, ask, and deny rules. It also sets the sandbox: its
mode, the network choice, the allowed hosts, and the decoy files. It shows the global rules as read-only:

![The Permissions settings tab with mode, sandbox, network, decoy files, rules, and global rules](../images/console/settings-permissions.png)

## One call, start to finish

Before the policy sees a call, `ToolEnv::canonical_call` changes it into the form that the tool will run:

- A relative `file_path` or `path` becomes absolute, relative to the current directory of the shell.
- `Bash` gets a `cwd` field set to that directory. This field replaces a `cwd` that the model sent.
- Ostra replaces a `WebFetch` URL with its parsed form.

The policy checks that changed call, and the tool runs the same call. This step stops two problems:

- Without it, the policy can check a relative path against one directory, and the tool can write it in a
  different directory after a `cd`.
- Without it, a URL such as `https://evil.example\@docs.rs/x` can look like `docs.rs` to a rule and go to
  `evil.example`.

Then `execute` changes each field that the model sent as a JSON string back into the type that its schema
gives. It runs the tool and measures the time. Each tool except `Bash` stops immediately on cancel. `Bash`
handles cancel itself, because it has a process group to kill.

Expand a call in the Activity tab to see the policy decision and the change. The `acceptEdits` permission mode
allowed this Edit:

![An expanded Edit call showing the permission rule that allowed it and its diff](../images/console/tool-edit.png)

## File tools

### Read

Reads a file and returns it with line numbers, in the same format as `cat -n`.

| Input | Meaning |
| --- | --- |
| `file_path` | The file. A relative path resolves against the directory of the shell. |
| `offset`, `limit` | The 1-based first line and the number of lines. The default is the first 2000 lines. |

Read cuts lines that are longer than 2000 characters. Read refuses files that are larger than 50 MB, and tells
the model to use `head`, `tail`, or `sed -n`. For a binary file, Read returns its size and not its bytes. A
directory gives an error that tells the model to use `Glob`.

Each successful Read marks the file as read in this execution. `Write` and `Edit` check that mark.

### Write

Writes a full file. Inputs: `file_path`, `content`.

If the file exists, the agent must read it in this execution first. Else the call fails with "Read it first,
then write it, so you do not overwrite content you have not seen." Write creates the parent directories. The
result has a unified diff for the Activity view, with a limit of 50,000 bytes. Thus you see what changed and
you do not have to open the file.

When the policy refuses a Write, the Activity view shows the guard rule and what to do in its place:

![An implementer run with a denied Write that names its guard rule](../images/console/execution.png)

### Edit

Replaces an exact string. Inputs: `file_path`, `old_string`, `new_string`, and `replace_all`.

- The agent must read the file in this execution first.
- `old_string` must match exactly one time. If it matches zero times, the call fails and tells the model to
  read the file again. If it matches more than one time, the call fails with the count. Then it tells the model
  to add the lines around the string, or to set `replace_all`.
- An empty `old_string` on a file that does not exist creates the file.
- If `old_string` is equal to `new_string`, the call fails, because the edit changes nothing.

The result shows the lines around the change. Edit requires an exact and unique match. Thus Edit can apply a
change safely without a person who reads it: the model cannot change a location that it did not name.

## Bash

Runs a command in `bash` and returns stdout and stderr together. On Windows, the shell is the
`bin\bash.exe` of Git for Windows. Ostra finds it by its absolute path (`shells::bash` in
[`shells.rs`](../../crates/ostra-core/src/shells.rs)). Ostra never uses the first `bash` on `PATH`. That is
frequently `C:\Windows\System32\bash.exe`, the WSL launcher, which runs the command in a Linux VM with a
different view of the files. If Git for Windows is not installed, the tool fails with a message that tells you
to install it. The setup screen shows the same check ("Shell for the Bash tool").

| Input | Meaning |
| --- | --- |
| `command` | The command line. |
| `timeout` | Milliseconds. The default is 120,000 (2 minutes). The maximum is 600,000 (10 minutes). |
| `description` | A short label for the Activity view. |

**The working directory persists.** The shell starts at the project root. The command runs through `eval` in a
wrapper script. The exit trap of the script writes the last directory to a file, so a `cd` stays in effect for
the next call. But each call is a new process: shell variables do not go to the next call. Under Git Bash, the
trap writes `pwd -W`, the Windows form of the directory (`C:/Users/me/repo`, not `/c/Users/me/repo`). The
reason: the other tools resolve paths against it.

**Output streams and has a limit.** The output goes to the Activity view when the command prints it. For very
long output, Ostra keeps only the start and the end in memory. The model gets at most 30,000 characters, from
the start and the end of the output. The reason: a build error is usually at one end or the other.

**Nothing stays after the call.** The command runs in its own process group. When the command returns, times
out, or is cancelled, Ostra kills the full group. Thus a server or a watcher started with `&` stops when the
command returns. For this reason, the prompt tells agents not to start one. On Windows, the shell starts
suspended and joins a Job Object. The Job Object kills its processes when its handle closes. Only after this
does the shell run, so its first child is already in the job
([`proctree.rs`](../../crates/ostra-core/src/proctree.rs)). Ostra closes the job when the call ends.

**Ostra removes secrets from the environment.** Before the command starts, Ostra removes these variables:

- Each credential variable that the config of Ostra names.
- Each workspace MCP secret.
- Each `OSTRA_*` variable.

Ostra sets `GIT_PAGER` and `PAGER` to `cat`, because a pager waits for input that never comes. Ostra removes
`GIT_EXTERNAL_DIFF` and closes stdin. [Secrets and data](../security/secrets-and-data.md) tells which
variables are credentials.

**It can run in a sandbox.** When the sandbox is on, the command runs under bubblewrap on Linux or Seatbelt on
macOS. The scratch dir of the execution is its `/tmp`. The other file tools map `/tmp` in the same way. Thus a
path that the shell printed is the same file for `Read`. [OS compatibility](../platforms/os-compatibility.md)
tells which backend each platform uses.

Some commands match the configured build and test commands of the project. These commands also add their wall
time to the `build_ms` of the execution, and their failures count toward the build-streak guard.

If no rule allows a Bash command, the command waits for the user. The same run shows a Write that the
`write-scope` guard refused:

![A code reviewer run with a Bash call asking for permission and a denied Write](../images/console/reviewer-ask.png)

## PowerShell and Cmd

On Windows, the `shell` capability also gives an agent two more tools:

- A `PowerShell` tool: Windows PowerShell 5.1, `powershell.exe` under the system dir.
- A `Cmd` tool: `cmd.exe` under the system dir.

Linux and macOS builds do not have them. They take the same inputs as Bash (`command`, `timeout`,
`description`). They have the same limits: the same timeouts, the same 30,000-character output limit, and the
same environment without secrets ([`winshell.rs`](../../crates/ostra-tools/src/winshell.rs)).

- **The working directory persists.** PowerShell runs `Set-Location` first and writes `(Get-Location).Path` in
  a `finally` block. Cmd runs `cd /d` first and `cd > <file>` last. Thus the `ERRORLEVEL` of the command stays
  the exit code.
- **The cmd line goes to cmd unchanged.** Rust quotes an inner `"` as `\"`, and cmd does not read that form.
  Thus Ostra sends the full `/d /s /c "..."` line to cmd without escapes. Ostra does not put the command in
  parentheses, because a `)` in it ends the group.
- **Nothing stays after the call.** Both tools start suspended in a Job Object, the same as Bash on Windows.
- **Your execution policy applies.** PowerShell runs with `-NoProfile -NonInteractive` and no
  `-ExecutionPolicy` override. Thus a `.ps1` script follows the policy that the machine or your organization
  sets. An override also caused the behavior monitor of Kaspersky to identify the build as a trojan.

Ostra cannot parse either shell. Thus the policy never allows one without an ask. The policy refuses a command
whose text names the files of Ostra or a credential store
([PowerShell and Cmd](../security/agent-containment.md#powershell-and-cmd)). For this reason, their
descriptions tell agents to use Bash first.

## Search tools

Both tools use the ripgrep crates (`grep-searcher`, `grep-regex`, `ignore`, `globset`). Thus they work like
`rg`, but `rg` does not have to be installed.

### Grep

| Input | Meaning |
| --- | --- |
| `pattern` | A Rust regex. |
| `path` | A file or directory. The default is the directory of the shell. |
| `glob`, `type` | File filters: `*.ts`, `**/*.{ts,tsx}`, or a type such as `rust` or `py`. |
| `output_mode` | `files_with_matches` (default, newest first), `content`, or `count`. |
| `-i`, `-n`, `-A`, `-B`, `-C` | Case, line numbers, context. |
| `multiline` | Lets `.` match newlines and lets a pattern match across lines. |
| `head_limit` | Sets a limit on the lines or files returned. |

The output limit is 30,000 characters.

### Glob

Finds files by a name pattern, such as `src/**/*.ts`. Inputs: `pattern`, `path`. Returns at most 100 paths,
newest first.

Both tools search hidden files, so `.ostra/` and `.agents/` are visible. The sandbox controls which ignore files
they obey:

- In a sandboxed execution, they obey each `.*ignore` file: `.gitignore`, `.dockerignore`, `.npmignore`, and
  each other name of that shape. They skip each hidden folder and do not go into it. The reason: a sandboxed
  agent never searches what an ignore file hides (Rule G2,
  [Ignored paths](../security/agent-containment.md#ignored-paths)).
- Without a sandbox, they obey the set that ripgrep reads: `.gitignore`, `.ignore`, `.git/info/exclude`, and
  the global excludes file.

Both tools skip credential stores and the data dir of Ostra, but not its agent assets. They skip them also when
a search starts from a parent folder. Thus a `Grep` from your home folder does not read `~/.ssh`.

## Web tools

### WebSearch

`WebSearch` has no local implementation. Anthropic and OpenAI both offer a server-side search tool. If an agent
has the `web_search` capability and the provider offers such a tool, the native loop enables it on the request.
Then the provider runs the search. The Activity view shows each search as a status line. On a harness, the CLI
uses its own search tool.

### WebFetch

Gets a URL and returns it as markdown. Inputs: `url`, and an optional `prompt` that tells what to look for.

On Anthropic, Ostra uses the fetch tool of the provider and does not offer the local tool. On other providers,
Ostra gets the page itself:

- Only `http` and `https`.
- **Public addresses only.** The resolver refuses names that resolve to loopback, private, link-local, CGNAT,
  unique-local, multicast, or reserved ranges. It also checks the address that it connects to. Thus a name
  cannot send the fetch to your machine or your network with a DNS answer that changes between the check and
  the connect. Ostra refuses a private IP literal before a lookup. A host that a `WebFetch(domain:<host>)` allow
  rule names exactly is exempt. With such a rule, you can let an agent read an internal docs server.
- **Redirects stay on one host.** Ostra does not follow a redirect to a different host. The model gets the new
  URL and must get it with a new call, which the policy checks. The policy judged only the first URL. Thus a
  followed redirect skips that check.
- Ostra changes HTML to markdown, returns text types unchanged, and refuses binary content. The body limit is
  5 MB, and the text limit is 100,000 characters.

![A running fact-check with Read, Grep, and WebFetch calls](../images/console/factcheck-run.png)

## Skill

Loads a `SKILL.md` and returns its text with the instruction to follow it. Give `name` or `path`.

With `name`, the tool looks in three locations, in this order:

1. The `.agents/skills/<name>/SKILL.md` of the project.
2. The older `.ostra/skills/<name>/SKILL.md`.
3. The skills that Ostra ships, such as `meta-author`.

The tool refuses a name with `/` or `..`, because a name is not a path. With `path`, the tool reads that file,
and the policy judges it as a `Read` of that path. The tool marks the skill file as read. Thus a later `Edit` of
it passes the read-first check.

## Ostra's own tools

Claude Code has no tools that agree with these tools. An agent uses them to give work to the engine and to
later runs.

### Report

Writes the report of the agent to its declared `Report file:` path. Input: `content`, and an optional `reason`.

The agent never chooses the path. The engine names each report path when it builds the spawn. Later stages read
the report from that path. If an agent has no declared report file, it gets an error that says so.

The lesson gate also applies to this call. Sometimes the agent goes from a verified failure to a recovery and
records no lesson. Then Report refuses the report until the agent records a lesson with `Memory`. The agent can
also give a `reason` that tells why no lesson applies. The lesson gate is a guard, so the containment page
describes it.

### Document

Writes a typed document: a research document, a spec, or a plan. The schemas are the structs in
`crates/ostra-core/src/doc`. An agent gets the tool when it holds a document grant (Rule CA6):
`document_research`, `document_spec`, or `document_plan`. The name of the agent does not matter. The schema of
the tool covers each kind that the run has a grant for (`document_tool_definition` in
`crates/ostra-tools/src/defs.rs`). It is the schema of one kind, or a choice of schemas when the agent holds
more than one grant.

The tool gets the kind from the prefix of the file name (`ostra-research-`, `ostra-spec-`, `ostra-plan-`). It
refuses a name that matches no granted kind. An agent with only one grant must also start the name with the
prefix of that kind, because later stages find the file by the prefix (`crates/ostra-tools/src/doc.rs`). The
standard agents `explore`, `generate-spec`, and `plan` each hold one grant.

| Input | Meaning |
| --- | --- |
| `path` | The `.md` path of the document in the session dir: `ostra-research-*`, `ostra-spec-*`, or the master `ostra-plan-*`. |
| `document` | The full document. It replaces the stored document. |
| `update` | A partial revision. Each top-level field that it names replaces the stored field. The exception is a list whose items have an `id`: such a list merges by id. An item that the update names takes the fields that it sends and keeps the other fields. The keyed lists inside it merge in the same way. Thus the agent can send the `action` of one step alone. |
| `remove` | Ids to remove at any depth: a requirement, a phase by number, a step. If an id is in more than one item, the agent names it with its parent, such as `2.3/E1`. |

Ostra stores the JSON next to the path. Ostra renders the markdown from the JSON with the section names that
the downstream prompts and the fact-check read. A plan also gets one `-phase-{N}.md` file for each phase. Ostra
computes the parts that code can compute, and the model never writes them:

- For the spec: the delivery order, the traceability tables, and the counts.
- For the plan: the indexes and the step counts.

Each write runs the document checks and returns their results. Errors are problems that code can decide: a
dangling requirement or criterion id, an uncovered criterion, a cycle between phases, or broken `AC{n}.{m}`
numbering. Warnings are judgment calls, such as more than one `SHALL` in a statement. The submit call is refused
when an error remains. Thus an agent cannot give a broken spec to the next stage.

Writes and submits also check the code that the document names (Hard rule 4). These items must exist:

- The paths of a research document.
- The criterion groundings and the consumed contract sources of a spec.
- The step files and the `read_first` paths of a plan.

A `path:Symbol` must name a word that the file contains. A plan step can name a file that an earlier step
creates. A write of a research document also records a hash of each file that it names. Later stages compare
the files against these hashes (Rule D2a). The result names the nested items that an `update` kept but did not
send. Thus the agent sees what a partial update left unchanged.

Only `Document` can write these files. The policy refuses a `Write`, `Edit`, or shell write to them, and tells
the agent to call `Document`. The reason: the next render overwrites the change, and the browser does not show
it. An agent without the matching grant cannot write them.

The other grants, `review_ledger`, `security_block`, `progress_log`, and `test_files`, add no tool. They only let
the write guards accept more paths from the file tools of the agent (see
[agent containment](../security/agent-containment.md)).

### Memory and MemoryRecall

`Memory` records one durable lesson in the memory database of the project. Inputs:

- `area`: a module scope, such as `orders::Service`.
- `lesson`: one line.
- `source`: optional. The default is the agent and the execution id.

If an agent records the same area and lesson again, the tool updates the lesson.

`MemoryRecall` returns recorded lessons, the most relevant first. Inputs: `query`, an optional `area` that
limits the results to a module and its sub-scopes, and `limit` (default 8, at most 50).

The engine owns the database. Agents can use it only through these two tools, and no file tool can write it.
The build streak also uses it. After the second failed build in a row, Ostra adds the recalled lessons for that
failure to the tool result. The agent does not have to ask for them. [Project memory](project-memory.md) tells
how Ostra stores and ranks lessons.

### DocsSearch

`DocsSearch` searches the documentation books that the docs stage wrote into the workspace (Rule B8). It returns
the units that best match a question. Each unit has only the passages that matched. Inputs: `query`, an
optional `project` that limits the search to the part of one project, and `limit` (default 5, at most 15
units).

A page of a book can be long, and most of it is not related to one question. Thus the search does not rank full
pages. It cuts each page at its `##` headings into units. The page unit holds the summary, the text above the
first `##` heading, and the code references. Each `##` heading starts a unit with the title `Page > Heading`. The
search then cuts each unit into these passages:

- Each paragraph.
- Each `mermaid` diagram and each other code block.
- Each list and each table, in windows of five rows.
- The code references of the page, in windows of five.

A `###` or deeper heading labels the passages under it, for example `Retries: list`. The index uses the words in
the labels and messages of a diagram. It never uses the Mermaid keywords or the node IDs. Thus a question about
"participants" does not match each sequence diagram. The glossary gives one passage for each term. The system
architecture gives one passage for each component, each failure case, and each group of links or scaling rows.

BM25 ranks each passage over three fields:

- The label of the passage, with weight 2.
- The inline code of the passage and the paths and symbols of its code references, with weight 1.5.
- Its text, with weight 1.

The search changes words to lowercase and applies a light stemmer. An identifier also gives its parts. Thus
`SessionState` matches "session state". The stemmer removes plurals, `-ed`, and `-ing`. Thus "started" matches
"starts". A unit gets a rank from the sum of four parts:

- Its title, with weight 3, counted one time.
- Its best passage.
- 35% of its second-best passage.
- A BM25 score of the full text of the unit. This part finds a question whose words are in several passages.

The title counts one time for the unit, and it never decides which passages the search shows. If the title of a
unit matches the question, the hit shows the passages whose own text matches. If no passage matches, the hit
shows the first passage of the unit. A hit shows at most two passages. Each of them has at least half the score
of the best passage of the unit. Then the hit shows the path of the Markdown file of the page. The agent reads
that file when it needs the full page.

In a list or table passage, the hit shows only the lines that contain a word of the question. It also shows a
count of the lines that it does not show. Thus a window of five list items shows the one item that matched. The
hit shows prose, code blocks, and diagrams in full, because a sentence or a flow cut in half gives a wrong
meaning.

The search builds the index from `book.json` on each call. Thus the index always agrees with a book that the
docs stage just wrote again. A book from before free pages loads with its typed sections as pages (Rule B9), so
the search cuts it in the same way. If the workspace has a book and the agent has the capability, the repo brief
names the tool. The brief tells the agent to search before it reads code to learn an area.

The retrieval eval (`tests/evals/book_retrieval/`, run by `crates/ostra-core/tests/book_retrieval.rs`) measures
the ranking. It uses 221 questions about the source of Ostra against a book that an Opus docs run wrote about
the repository. Each question has labels for the units that state its answer. 127 questions have one such unit.
For those questions, the correct unit is first for 64%, and in the top five for 91%. A top-five result is about
3,300 characters. The search is weak on a question in plain words that share no word with the book, for example
"the code formatter" for a section about the format command. The eval fails if a ranking change goes below its
floors.

A Memory call expanded in the Activity tab, after a failed check and its fix:

![An expanded Memory call recording a build lesson](../images/console/memory-call.png)

### Code navigation

Eight tools answer questions from the code index of the project, not from file reads:

| Tool | Answers |
| --- | --- |
| `CodeOutline` | The imports of a file, each with the project file that it resolves to, and the definitions with line ranges. |
| `CodeFind` | Definitions by name: exact, then prefix, then substring, then initials (`pn` finds `parse_name`). |
| `CodeCallers` | Each use of a symbol, grouped by the function or type that contains it. |
| `CodeCallees` | What the body of one function or type uses. |
| `CodeImplementations` | What a type implements or extends, and what implements it. |
| `CodeNeighbors` | The files that one file uses and the files that use it, with the names that link them. |
| `CodeImpact` | What a change can break, hop by hop, for a symbol or a set of files. |
| `CodeMap` | Packages, their dependencies, the files with the most dependents, and import cycles. |

These tools exist for two reasons. An outline costs a fraction of a file read. And a caller list from the index
does not include the mentions that a text search cannot tell apart from a different definition with the same
name. On a harness, the tools are `code_outline`, `code_find`, and the other `code_*` names. The `ostra` MCP
server serves them. [The code index](code-index.md) describes how Ostra builds the index.

### Messaging tools

`ListAgents`, `SendMessage`, and `WaitForMessage` let an agent with the `coordinate` capability send messages to
the other subagents of its session. The definitions are in `crates/ostra-tools/src/defs.rs`. The inputs and
limits are in `crates/ostra-core/src/coord.rs`. On a harness, the tools are `list_agents`, `send_message`, and
`wait_for_message`. Claude Code shows them as `mcp__ostra__send_message` and the same for the others.

| Tool | Input | What it does |
| --- | --- | --- |
| `ListAgents` | none | Gives your subagent ID and each subagent of the session: its agent, label, project, status, what it waits for, and its report. Also gives the helper agents that you can start. |
| `SendMessage` | `message`, exactly one of `to` (a subagent ID) or `agent` (a helper), an optional `project` for a helper, an optional `wait` | Puts the message in the queue and returns immediately. With `wait: true`, the run pauses after the call. |
| `WaitForMessage` | none | Pauses the run until a message arrives for it. |

A message is plain text of at most 8,000 characters (`MAX_MESSAGE_CHARS`). Put longer material in a file, and
give its path in the message. Ostra never delivers a message during a request. The receiver reads it at its
next turn boundary. A pause ends a native run with status `waiting`. A harness or programmatic run waits with
its process alive. The next message for the run wakes it.

Ostra refuses `WaitForMessage` when the run must reply to a waiting sender, because that sender then waits on a
run that waits on it. The permission layer allows the three tools in each mode, because they change no file.
[Subagents that talk to each other](agents.md#subagents-that-talk-to-each-other) covers the full flow.

### The submit tool

Each agent has exactly one submit tool: `submit_<agent>`. Its schema is the struct of that agent in
`crates/ostra-core/src/submit.rs`. It is the last call of each run, and the engine reads only its payload. The
executors page describes it, because it ends the run and does no work:
[The submit call](executors.md#the-submit-call).

## Project management tools

An agent calls management tools to change Ostra itself, not the files that it works on. The first toolset
manages projects. It is for a request that needs a codebase that no project holds. An example is a new service
that otherwise has to be inside an existing repository.

The implementer holds the `manage_projects` capability by default, and each agent can request it (Rule CA6). The
guard limits the capability to one run. That run holds the capability and runs a phase that the approved plan
puts in a project that does not exist yet. Thus Ostra creates a project only after the user approved the plan
that needs it. If the user rejects a spec or plan, nothing stays on disk.

### ProjectList

`ProjectList` takes no input. It returns the workspace root, which is the base of the relative folder of a new
project. For each project, it also returns the key, the folder, the stack, the init status, and whether the
project is in the scope of this session. It is read-only, so each mode allows it.

### ProjectCreate

`ProjectCreate` takes these inputs:

| Input | Meaning |
| --- | --- |
| `key` | The project key, with the rule of the Add project dialog: lowercase letters, digits, and dashes, starting with a letter or digit. |
| `stack` | The language and the main framework in a few words, such as `rust`. One line, at most 64 characters. |
| `purpose` | What the codebase is for, at most 800 characters. The user approves the project from it. |
| `requirements` | 1 to 20 base requirements, one fact each, at most 400 characters each: toolchain version, libraries with versions, build tool, transport, and the systems that it connects to. |
| `folder` | Optional, relative to the workspace root, with no `..`. The default is the key. |
| `git_init` | Optional, default true: run `git init` in the new folder. |

The implementer, or the agent that holds the grant, gets these inputs from its phase file. The plan copied them
from the `Constraint` criteria of the spec. A call must pass three checks before Ostra changes anything on disk:

1. **The guard (rule O2).** The `manage-tools` guard allows the call only from an execution whose context has
   `creates_project` set. The runner sets it only for a run that holds `manage_projects` in a phase in a project
   that meets two conditions:
   - The approved plan lists the project in `new_projects`.
   - The session does not hold the project yet.

   The key must be the project key of that phase. The guard also refuses a malformed input: a bad key or stack,
   an empty or too long purpose, no requirements or too many, or a folder that is absolute or goes up with
   `..`. Because the guard checks here, Ostra never asks the user about a call that cannot run. Until the
   project exists, the same run can write only in its session dir and temp. The reason: its `Repo root:` is its
   session dir, and no project exists yet to write in.
2. **The permission (rule O1).** The call asks the user, in each permission mode, also in bypass mode, because
   a new project changes Ostra and not only files. The card text is similar to
   ``Create project `notes-mcp` (rust) in `notes-mcp/` of the workspace: ...``. Ostra cuts it at 250
   characters, because Grok clips ask reasons. The card offers no "always" rule. The answer "always in this
   workspace" adds no rule, because no allow rule can replace the answer. Only YOLO answers the ask. A `deny`
   rule on `ProjectCreate` still refuses it, and plan mode refuses it.
3. **The checks of the server.** After the answer, the server (`crates/ostra-server/src/manage.rs`) refuses
   these keys and folders:
   - A key that the workspace already has.
   - A folder that exists and is not empty.
   - A folder outside the workspace root. The server resolves a symlinked parent first.
   - A folder in `.ostra`, or a folder inside or around a different project.

   The server also refuses the call in these conditions: the session has ended, the session is not a pipeline
   session, the run that asked has ended, or the key is already in the session.

Then the server creates the folder and runs `git init` if the call asks for it. The `git init` uses the same
no-exec git settings as a clone. The server registers the project and its stack in `workspace.toml`. Then it
appends a `ProjectCreated` event to the session. If `git init` or the registration fails, the server removes
what it created. The event stops the implementer that asked. After the init of the new project runs, the phase
starts again inside the new project. [The pipeline](pipeline.md#a-new-codebase) tells what occurs next.

### How both executors reach them

The tools crate knows no workspace or engine. Thus it defines a `Manage` trait, and the server implements it.
When an execution opens, the server gives it a handle bound to its workspace, session, and execution. The
server gives the handle only when the agent has the `manage_projects` capability and runs in a session. The
native loop runs the call through that handle in the same process. A harness sees `project_list` and
`project_create` on the MCP server of Ostra. The server lists them only for an execution that holds the handle.
It checks, asks, and runs each call one time, the same as `report`.

## Workspace MCP tools

The MCP servers in the `mcp_servers` list of a workspace are available to each agent on each executor, because
Ostra is the only MCP client. A tool keeps the canonical name `mcp__<server>__<tool>` in the native loop, in
the Activity view, and in permission rules. A harness sees it as `mcp__ostra__<server>__<tool>`, through the MCP
server of Ostra.

The tool list is fixed when an execution opens. If Ostra cannot connect to a server, Ostra adds one Activity
line, and the run continues without that server. Results are text:

- Images and binary resources become a one-line note.
- Structured content becomes JSON.
- Ostra cuts results at 100 KiB.

[MCP servers](mcp.md) covers transports, sign-in, and names.

## Where to look in the code

| Concern | File |
| --- | --- |
| Dispatch, canonical calls, state for each execution | `crates/ostra-tools/src/lib.rs` |
| Tool descriptions and input schemas | `crates/ostra-tools/src/defs.rs` |
| Read, Write, Edit | `crates/ostra-tools/src/fs.rs` |
| Bash | `crates/ostra-tools/src/bash.rs` |
| Grep, Glob | `crates/ostra-tools/src/search.rs` |
| WebFetch | `crates/ostra-tools/src/web.rs` |
| Skill, Report, Memory, MemoryRecall, DocsSearch | `crates/ostra-tools/src/misc.rs` |
| Book passages and ranking for DocsSearch | `crates/ostra-core/src/book_search.rs` |
| Document | `crates/ostra-tools/src/doc.rs` |
| ProjectList, ProjectCreate: input rules, the handle, the server side | `crates/ostra-core/src/manage.rs`, `crates/ostra-tools/src/manage.rs`, `crates/ostra-server/src/manage.rs` |
| Capability to tool name for each executor | `assets/tool-mapping.toml` |
| Tool families for permissions | `crates/ostra-policy/src/perms.rs` |

Next: [The code index](code-index.md) tells what the code navigation tools read.
