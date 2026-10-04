# Tools

A tool is the only way an agent touches anything: a file, the shell, the web, the project's memory, or the
engine. This page describes every tool Ostra gives an agent, what each one does, and how an agent ends up with
the tools it has.

Two ideas shape all of it:

- **The names are Claude Code's.** Ostra's native tools are called `Read`, `Write`, `Edit`, `Bash`, `Grep`,
  `Glob`, `Skill`, `WebSearch`, and `WebFetch`, with the same input fields as Claude Code's tools of those
  names. The agent prompts were written and tuned against those tools, so reusing their shape keeps the prompts
  working on both executors. The policy also judges every call in that shape, whichever executor made it.
- **Tools do the work and the policy decides.** Nothing in `ostra-tools` checks permissions or guards. The
  executor runs `ExecutionPolicy::check` before a tool runs (see [Executors](executors.md)), and a tool is only
  ever called with a call the policy already allowed. That keeps each tool small and puts every rule in one
  place, [Agent containment](../security/agent-containment.md).

## How an agent gets its tools

Agents do not ask for tools by name. Each agent's `agent.toml` lists **capabilities**, and each executor maps a
capability to its own tool:

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

The table is `assets/tool-mapping.toml`. The prompt renderer reads it and writes each agent's prompt with the
tool names of the executor it runs on, so an implementer on Codex is told to edit with `apply_patch` and an
implementer on the native loop is told to use `Edit`. For the capabilities a CLI serves differently, the
renderer adds a short instruction, for example how to load a skill in a CLI that has no skill tool.

Two consequences follow from the table:

- **On a harness, the file and shell tools are the CLI's own.** Ostra does not replace Claude Code's `Read` or
  Codex's `apply_patch`. It checks each call through the hook bridge and lets the CLI run it. The adapters turn
  each CLI's call back into the canonical shape before the policy sees it, so `apply_patch` is judged as writes
  to the files the patch touches.
- **Ostra's own tools come from Ostra everywhere.** `Report`, `Document`, `Memory`, `MemoryRecall`, `DocsSearch`, the
  code navigation tools, the project management tools, the submit tool, and the workspace MCP tools are Ostra's code
  on every executor. The native
  loop calls them directly. A harness reaches them through the `ostra` MCP server.

Capabilities are the upper bound. A reviewer has no `write` capability, so it has no `Write` tool at all, on any
executor. The policy then narrows further what the tools an agent has may touch.

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
| documentation, system-architecture | read, shell, search_text, glob, code, docs_search |
| prompt-generation | read, edit, write, shell, search_text, glob, skill, report, code, test_files |
| initializer | read, write, edit, shell, search_text, glob, code, test_files |
| quick-answer | read, search_text, glob, web_search, web_fetch, memory_recall, code, docs_search |
| advisor | read, shell, search_text, glob, web_search, web_fetch, memory_recall, docs_search |

A custom agent lists its own capabilities in its definition; without a list it gets read, search_text, glob,
report, and coordinate. Any agent may request any capability, including the document grants, the ownership grants (`review_ledger`,
`security_block`, `progress_log`, `test_files`), and `manage_projects`, because none is reserved (Rule CA6);
approving the workspace file is what lets it hold them. The ownership grants add no tool: they only widen what
the guards let the agent write. The implementer holds `manage_projects` by default, and any agent that holds it
and runs a phase in a project the plan names as new may create that project. Every agent also gets its own
`submit_<agent>` tool and the workspace's MCP tools. [Agents](agents.md) covers what each agent is for.

## How the policy sees each tool

The permission layer sorts tools into families, and the family decides which rules apply to a call:

| Family | Tools | How a call is judged |
| --- | --- | --- |
| Read | `Read`, `Grep`, `Glob` | Allowed unless a `deny` or `ask` rule names the path. `Read(~/.ssh/**)` style rules apply. |
| Edit | `Write`, `Edit`, and harness equivalents (`MultiEdit`, `NotebookEdit`, `ApplyPatch`) | Each target path is judged. The session dir and the OS temp dir are allowed. Other paths follow the rules, then the permission mode: `acceptEdits` allows edits inside the project. A path that runs code later is judged by the mode even inside the project. |
| Bash | `Bash` | The command is parsed with tree-sitter-bash and each simple command is matched separately, so `npm test && curl evil` does not pass on `Bash(npm test *)`. A command Ostra cannot parse, or one that uses shell syntax it does not model, asks, and plan mode refuses it. |
| Other (opaque shells) | `PowerShell`, `Cmd` (Windows only) | Not parsed. Refused when the text names Ostra's own files, engine state, or a credential store; otherwise matched against `PowerShell(...)` and `Cmd(...)` rules, then the mode, which never allows one unasked. |
| WebFetch | `WebFetch` | Matched against `WebFetch(domain:...)` rules. With no rule, it asks, except in bypass mode. |
| Other (changes Ostra) | `ProjectCreate` | Always asks, in every mode including bypass, and no allow rule stands in for the answer (rule O1). A `deny` rule refuses it, plan mode refuses it, and only YOLO answers the ask. |
| Other | `Skill`, `WebSearch`, `Report`, `Document`, `Memory`, `MemoryRecall`, `DocsSearch`, `ProjectList`, the code tools, `submit_*`, workspace MCP tools | Ostra's own tools are allowed. `Skill` with a `path` is judged as a Read of that file. Workspace MCP tools are allowed unless a rule says otherwise, and plan mode allows only those their server marks read-only (rule M1). |

The guards (write scope, state ownership, the report path, the lesson gate, the build streak, self-protection,
and the management tools guard) run before this, on every family. Write scope, the report path, and most of
self-protection run only with tool enforcement enabled. They are on [Agent containment](../security/agent-containment.md).

The Permissions tab of Settings sets the mode, the sandbox (its mode, network choice, allowed hosts, and decoy
files), and the allow, ask, and deny rules, and shows the global rules read-only:

![The Permissions settings tab with mode, sandbox, network, decoy files, rules, and global rules](../images/console/settings-permissions.png)

## One call, start to finish

Before the policy sees a call, `ToolEnv::canonical_call` rewrites it into the form the tool will run:

- A relative `file_path` or `path` becomes absolute against the shell's current directory.
- `Bash` gets a `cwd` field set to that directory, replacing any `cwd` the model sent.
- A `WebFetch` URL is replaced with its parsed form.

The policy checks that rewritten call, and the tool runs that same call. Without this step a relative path could
be checked against one directory and written in another after a `cd`, or a URL such as
`https://evil.example\@docs.rs/x` could look like `docs.rs` to a rule and reach `evil.example`.

`execute` then coerces any field the model sent as a JSON string back into the type its schema says, runs the
tool, and times it. Every tool except `Bash` stops at once on cancel. `Bash` handles cancel itself, because it has
a process group to kill.

Expanding a call in the Activity tab shows the policy decision and the change. This Edit was allowed by the
`acceptEdits` permission mode:

![An expanded Edit call showing the permission rule that allowed it and its diff](../images/console/tool-edit.png)

## File tools

### Read

Reads a file and returns it numbered like `cat -n`.

| Input | Meaning |
| --- | --- |
| `file_path` | The file. Relative paths resolve against the shell's directory. |
| `offset`, `limit` | The 1-based first line and the number of lines. The default is the first 2000 lines. |

Lines over 2000 characters are cut. Files over 50 MB are refused with a pointer to `head`, `tail`, or `sed -n`.
A binary file returns its size instead of its bytes. A directory is an error that points to `Glob`.

Each successful Read marks the file as read in this execution. That mark is what `Write` and `Edit` check.

### Write

Writes a whole file. Inputs: `file_path`, `content`.

If the file already exists, it must have been read in this execution, or the call fails with "Read it first,
then write it, so you do not overwrite content you have not seen." Parent directories are created. The result
carries a unified diff for the Activity view (capped at 50,000 bytes), so you see what changed without opening
the file.

A Write the policy refused shows the guard rule and what to do instead:

![An implementer run with a denied Write that names its guard rule](../images/console/execution.png)

### Edit

Replaces an exact string. Inputs: `file_path`, `old_string`, `new_string`, and `replace_all`.

- The file must have been read in this execution.
- `old_string` must match exactly once. Zero matches fail with advice to re-read the file. Several matches fail
  with the count and advice to add surrounding lines, or to set `replace_all`.
- An empty `old_string` on a file that does not exist creates it.
- `old_string` equal to `new_string` fails, because nothing would change.

The result shows the lines around the change. Requiring an exact, unique match is what makes an edit safe to
apply without a human reading it: the model cannot change a place it did not name.

## Bash

Runs a command in `bash` and returns stdout and stderr together. On Windows that is Git for Windows'
`bin\bash.exe`, found by absolute path (`shells::bash` in [`shells.rs`](../../crates/ostra-core/src/shells.rs)),
never the first `bash` on `PATH`, which is often `C:\Windows\System32\bash.exe`, the WSL launcher, running the
command in a Linux VM with another view of the files. Without Git for Windows the tool fails with a message that
says to install it, and the setup screen shows the same check ("Shell for the Bash tool").

| Input | Meaning |
| --- | --- |
| `command` | The command line. |
| `timeout` | Milliseconds. The default is 120,000 (2 minutes), the maximum 600,000 (10 minutes). |
| `description` | A short label for the Activity view. |

**The working directory persists.** The shell starts at the project root. The command runs through `eval` in a
wrapper script whose exit trap writes the final directory to a file, so a `cd` carries into the next call. Each
call is still a new process: shell variables do not carry over. Under Git Bash the trap writes `pwd -W`, the
Windows form of the directory (`C:/Users/me/repo` rather than `/c/Users/me/repo`), because the other tools resolve
paths against it.

**Output streams and is bounded.** Output reaches the Activity view as the command prints it. In memory, Ostra
keeps the head and tail of very long output, and the model gets at most 30,000 characters, keeping the start and
the end, because a build error is usually at one end or the other.

**Nothing survives the call.** The command runs in its own process group. When it returns, times out, or is
cancelled, the whole group is killed. A server or a watcher started with `&` is stopped when the command
returns, which is why the prompt tells agents not to start one. On Windows the shell starts suspended, joins a
Job Object that kills its processes when its handle closes, and only then runs, so its first child is already in
the job ([`proctree.rs`](../../crates/ostra-core/src/proctree.rs)). The job is closed when the call ends.

**The environment is scrubbed.** Every credential variable that Ostra's config names, every workspace MCP secret,
and every `OSTRA_*` variable is removed before the command starts. `GIT_PAGER` and `PAGER` are set to `cat`,
because a pager would wait for input that never comes. `GIT_EXTERNAL_DIFF` is removed. Stdin is closed.
[Secrets and data](../security/secrets-and-data.md) explains which variables count as credentials.

**It may run in a sandbox.** With the sandbox on, the command runs under bubblewrap on Linux or Seatbelt on
macOS, with the execution's own scratch dir as `/tmp`. The other file tools map `/tmp` the same way, so a path the
shell printed means the same file to `Read`. [OS compatibility](../platforms/os-compatibility.md) covers which
backend each platform uses.

Commands that match the project's configured build and test commands also add their wall time to the
execution's `build_ms`, and their failures count toward the build-streak guard.

A Bash command that no rule allows waits for the user. The same run shows a Write refused by the `write-scope`
guard:

![A code reviewer run with a Bash call asking for permission and a denied Write](../images/console/reviewer-ask.png)

## PowerShell and Cmd

On Windows, the `shell` capability also gives an agent a `PowerShell` tool (Windows PowerShell 5.1,
`powershell.exe` under the system dir) and a `Cmd` tool (`cmd.exe` under the system dir). Linux and macOS builds
do not have them. They take the same inputs as Bash (`command`, `timeout`, `description`) and share its limits: the
same timeouts, the same 30,000-character output bound, and the same scrubbed environment
([`winshell.rs`](../../crates/ostra-tools/src/winshell.rs)).

- **The working directory persists.** PowerShell runs `Set-Location` first and writes `(Get-Location).Path` in a
  `finally` block; cmd runs `cd /d` first and `cd > <file>` last, which leaves the command's `ERRORLEVEL` as the
  exit code.
- **cmd's line is passed raw.** Rust quotes an inner `"` as `\"`, which cmd does not read, so the whole
  `/d /s /c "..."` line reaches cmd unescaped. The command is not wrapped in parentheses, because a `)` in it would
  end the group.
- **Nothing survives the call.** Both start suspended in a Job Object, as Bash does on Windows.
- **Your execution policy applies.** PowerShell runs with `-NoProfile -NonInteractive` and no
  `-ExecutionPolicy` override, so running a `.ps1` script follows the policy set on the machine or by your
  organization. An override was also what made Kaspersky's behavior monitor flag the build as a trojan.

Ostra cannot parse either shell, so the policy never allows one unasked and refuses a command whose text names its
own files or a credential store ([PowerShell and Cmd](../security/agent-containment.md#powershell-and-cmd)). Their
descriptions tell agents to prefer Bash for that reason.

## Search tools

Both use the ripgrep crates (`grep-searcher`, `grep-regex`, `ignore`, `globset`), so they behave like `rg` without
needing it installed.

### Grep

| Input | Meaning |
| --- | --- |
| `pattern` | A Rust regex. |
| `path` | A file or directory. The default is the shell's directory. |
| `glob`, `type` | File filters: `*.ts`, `**/*.{ts,tsx}`, or a type such as `rust` or `py`. |
| `output_mode` | `files_with_matches` (default, newest first), `content`, or `count`. |
| `-i`, `-n`, `-A`, `-B`, `-C` | Case, line numbers, context. |
| `multiline` | Lets `.` match newlines and a pattern span lines. |
| `head_limit` | Caps the lines or files returned. |

Output is capped at 30,000 characters.

### Glob

Finds files by name pattern, such as `src/**/*.ts`. Inputs: `pattern`, `path`. Returns at most 100 paths, newest
first.

Both tools search hidden files, so `.ostra/` and `.agents/` are visible. Which ignore files they honor depends on
the sandbox. In a sandboxed execution they honor every `.*ignore` file (`.gitignore`, `.dockerignore`,
`.npmignore`, and any other name of that shape), skipping each hidden folder without walking into it, because a
sandboxed agent never searches what an ignore file hides (Rule G2,
[Ignored paths](../security/agent-containment.md#ignored-paths)). Without a sandbox they honor the set ripgrep
reads: `.gitignore`, `.ignore`, `.git/info/exclude`, and the global excludes file. Both skip credential stores and
Ostra's own data dir (except its agent assets) even when a search starts from a parent folder, so a `Grep` from
your home folder does not read `~/.ssh`.

## Web tools

### WebSearch

`WebSearch` has no local implementation. When an agent has the `web_search` capability and the provider offers a
server-side search tool (Anthropic and OpenAI both do), the native loop enables it on the request and the provider
runs the search. Each search shows as a status line in the Activity view. On a harness, the CLI's own search tool
is used.

### WebFetch

Fetches a URL and returns it as markdown. Inputs: `url`, and an optional `prompt` saying what to look for.

On Anthropic the provider's own fetch tool is used, and the local tool is not offered. On other providers Ostra
fetches the page itself:

- Only `http` and `https`.
- **Public addresses only.** The resolver refuses names that resolve to loopback, private, link-local, CGNAT,
  unique-local, multicast, or reserved ranges, and it checks the address it connects to, so a name cannot point
  the fetch at your machine or your network by changing its DNS answer between the check and the connect. A
  private IP literal is refused before any lookup. A host named exactly by a `WebFetch(domain:<host>)` allow
  rule is exempt, which is how you let an agent read an internal docs server.
- **Redirects stay on one host.** A redirect to another host is not followed. The model gets the new URL and has
  to fetch it with a new call, which the policy checks. The policy judged only the first URL, so following the
  redirect would skip that check.
- HTML becomes markdown, text types come back as they are, and binary content is refused. The body is capped at
  5 MB and the text at 100,000 characters.

![A running fact-check with Read, Grep, and WebFetch calls](../images/console/factcheck-run.png)

## Skill

Loads a `SKILL.md` and returns its text with the instruction to follow it. Pass `name` or `path`.

With `name`, it looks in the project's `.agents/skills/<name>/SKILL.md`, then the older
`.ostra/skills/<name>/SKILL.md`, then the skills Ostra ships (such as `meta-author`). A name with `/` or `..` is
refused, because a name is not a path. With `path`, it reads that file, and the policy judges it as a `Read` of that
path. The skill file is marked read, so a later `Edit` of it passes the read-first check.

## Ostra's own tools

These have no counterpart in Claude Code. They are how an agent hands work to the engine and to later runs.

### Report

Writes the agent's report to its declared `Report file:` path. Input: `content`, plus an optional `reason`.

The agent never chooses the path. The engine names every report path when it builds the spawn, and later stages
read the report from there. An agent with no declared report file gets an error that says so.

The call is also where the lesson gate applies. If the agent went from a verified failure to a recovery and
recorded no lesson, the report is refused until it records one with `Memory`, or passes a `reason` that says why no
lesson applies. That is a guard, so it lives on the containment page.

### Document

Writes a typed document: a research document, a spec, or a plan. The schemas are the structs in
`crates/ostra-core/src/doc`. An agent gets the tool when it holds a document grant (Rule CA6):
`document_research`, `document_spec`, or `document_plan`, whatever the agent's name. Its schema covers every kind
the run is granted (`document_tool_definition` in `crates/ostra-tools/src/defs.rs`): one kind's schema directly,
or a choice of them when the agent holds several. The tool takes the kind from the file name's prefix
(`ostra-research-`, `ostra-spec-`, `ostra-plan-`) and refuses a name that matches no granted kind. Even an agent granted a
single kind must start the name with that kind's prefix, because later stages find the file by it
(`crates/ostra-tools/src/doc.rs`). The
standard agents `explore`, `generate-spec`, and `plan` each hold one grant.

| Input | Meaning |
| --- | --- |
| `path` | The document's `.md` path in the session dir: `ostra-research-*`, `ostra-spec-*`, or the master `ostra-plan-*`. |
| `document` | The whole document. It replaces what is stored. |
| `update` | A partial revision. Each top-level field it names replaces the stored one, except lists whose items carry an `id`, which merge by id. An item the update names takes the fields it sends and keeps the rest, and the keyed lists inside it merge the same way, so one step's `action` can be sent alone. |
| `remove` | Ids to drop at any depth: a requirement, a phase by number, a step. An id that sits in more than one item is named with its parent, such as `2.3/E1`. |

The JSON is stored beside the path, and the markdown is rendered from it with the section names the downstream
prompts and the fact-check read. A plan also gets one `-phase-{N}.md` file per phase. Ostra computes the parts code
can compute (the spec's delivery order, traceability tables, and counts; the plan's indexes and step counts), and the
model never writes them.

Every write runs the document checks and returns their results. Errors are things code can decide: a dangling
requirement or criterion id, an uncovered criterion, a cycle between phases, broken `AC{n}.{m}` numbering. Warnings
are judgment calls, such as more than one `SHALL` in a statement. The submit call is refused while an error remains,
so an agent cannot hand the next stage a broken spec.

Writes and submits also check the code the document names (Hard rule 4): a research document's paths, a spec's
criterion groundings and consumed contract sources, and a plan's step files and `read_first` paths must exist, and a
`path:Symbol` must name a word the file contains. A plan step may name a file an earlier step creates. A research
document's write also records a hash of each file it names, which later stages compare against (Rule D2a). The
result names the nested items an `update` kept without sending them, so the agent sees what a partial update left
in place.

These files can only be written through `Document`. A `Write`, `Edit`, or shell write to them is refused with the
correction to call `Document`, because the next render would overwrite the change and the browser would not show it.
An agent without the matching grant may not write them at all.

The other grants, `review_ledger`, `security_block`, `progress_log`, and `test_files`, add no tool: they only
widen what the write guards let the agent's file tools touch (see [agent containment](../security/agent-containment.md)).

### Memory and MemoryRecall

`Memory` records one durable lesson in the project's memory database: `area` (a module scope such as
`orders::Service`), `lesson` (one line), and an optional `source`, which defaults to the agent and execution id.
Recording the same area and lesson again updates it.

`MemoryRecall` returns recorded lessons, most relevant first. Inputs: `query`, an optional `area` that narrows to a
module and its sub-scopes, and `limit` (default 8, at most 50).

The database is engine-owned: agents reach it only through these two tools, and no file tool may write it. The build
streak also uses it: after the second failed build in a row, recalled lessons for that failure are appended to the
tool result without the agent asking. [Project memory](project-memory.md) covers how lessons are stored and ranked.

### DocsSearch

`DocsSearch` searches the documentation books the docs stage wrote into the workspace (Rule B8) and returns the
sections that best match a question, each with only the passages that matched. Inputs: `query`, an optional
`project` that narrows the search to one project's part, and `limit` (default 5, at most 15 sections).

A section of a book can be long, and most of it does not bear on any one question, so the search does not rank
whole sections. It cuts each section and sub-section into passages: the purpose, the boundaries, the assumptions,
the business flow, each diagram, each table, the separation of concerns, and the code references, with lists and
tables over five rows split into windows of five. A diagram is indexed by its title and the words in its labels
and messages, never by its Mermaid keywords or node IDs, so a question about "participants" does not match every
sequence diagram. The glossary gives one passage per term, and the system architecture one per component, failure
case, and group of links or scaling rows.

BM25 ranks each passage over three fields: its label (weight 2), the paths and symbols of its code references
(weight 1.5), and its text (weight 1). Words are lowercased and lightly stemmed, and an identifier also yields
its parts, so `SessionState` matches "session state", and plurals, `-ed`, and `-ing` are stripped, so "started"
meets "starts". A section ranks by its title (weight 3, counted once), plus its best passage, 35% of its second
best, and a BM25 score of the section's whole text, which catches a question whose words are spread over several
passages. The title counts once for the section and never decides which passages are
shown: a section whose title matches the question shows the passages whose own text matches, or its purpose when
none does. A hit shows at most two passages, each at least half as strong as the section's best, then the path of
the section's Markdown file, which the agent reads when it needs the whole section. Within a list or table passage,
only the lines that name a word of the question are shown, with a count of the lines left out, so a window of five
assumptions shows the one that matched. Prose and diagrams are shown whole, because a sentence or a flow cut in
half misleads.

The index is built from `book.json` on every call, so it never falls behind a book the docs stage just rewrote.
When the workspace has a book and the agent has the capability, the repo brief names the tool and tells the agent
to search before it reads code to learn an area.

The retrieval eval (`tests/evals/book_retrieval/`, run by `crates/ostra-core/tests/book_retrieval.rs`) measures
the ranking with 221 questions about Ostra's own source against a book an Opus docs run wrote about the repository.
Each question was labeled with the sections that state its answer; 127 have one. On those, the right section is
first for 63% of questions and in the top five for 90%, and a top-five result is about 3,200 characters. The weak
spot is a question in plain words that share none with the book ("the code formatter" for a section about the
format command): 82% of those reach the top five. The eval fails when a ranking change drops below its floors.

A Memory call expanded in the Activity tab, after a failed check and its fix:

![An expanded Memory call recording a build lesson](../images/console/memory-call.png)

### Code navigation

Eight tools answer questions from the project's code index instead of from file reads:

| Tool | Answers |
| --- | --- |
| `CodeOutline` | A file's imports (with the project file each resolves to) and definitions with line ranges. |
| `CodeFind` | Definitions by name: exact, then prefix, then substring, then initials (`pn` finds `parse_name`). |
| `CodeCallers` | Every use of a symbol, grouped by the enclosing function or type. |
| `CodeCallees` | What one function or type body uses. |
| `CodeImplementations` | What a type implements or extends, and what implements it. |
| `CodeNeighbors` | The files one file uses and the files that use it, with the linking names. |
| `CodeImpact` | What a change can break, hop by hop, for a symbol or a set of files. |
| `CodeMap` | Packages, their dependencies, the most depended-on files, and import cycles. |

They exist because an outline costs a fraction of a file read, and a caller list from the index drops the mentions a
text search cannot tell apart from another definition with the same name. On a harness they are `code_outline`,
`code_find`, and so on, served by the `ostra` MCP server. [The code index](code-index.md) describes how the index is
built.

### Messaging tools

`ListAgents`, `SendMessage`, and `WaitForMessage` let an agent with the `coordinate` capability message the other
subagents of its session (definitions in `crates/ostra-tools/src/defs.rs`, inputs and limits in
`crates/ostra-core/src/coord.rs`). On a harness they are `list_agents`, `send_message`, and `wait_for_message`, which
Claude Code shows as `mcp__ostra__send_message` and so on.

| Tool | Input | What it does |
| --- | --- | --- |
| `ListAgents` | none | Your subagent ID, every subagent of the session with its agent, label, project, status, what it waits on, and its report, and the helper agents you can start. |
| `SendMessage` | `message`, exactly one of `to` (a subagent ID) or `agent` (a helper), optional `project` for a helper, optional `wait` | Queues the message and returns at once. With `wait: true` the run pauses after it. |
| `WaitForMessage` | none | Pauses the run until a message arrives for it. |

A message is plain text of at most 8,000 characters (`MAX_MESSAGE_CHARS`); longer material belongs in a file whose
path the message names. A message is never delivered in the middle of a request: the receiver reads it at its next
turn boundary. Pausing ends a native run with status `waiting` and makes a harness or programmatic run wait with its
process alive, and the next message for the run wakes it. `WaitForMessage` is refused while the run owes a waiting
sender a reply, because that sender would then wait on a run that waits on it. The permission layer allows the three
tools in every mode, because they change no file. [Subagents that talk to each
other](agents.md#subagents-that-talk-to-each-other) covers the whole flow.

### The submit tool

Each agent has exactly one: `submit_<agent>`, whose schema is that agent's struct in `crates/ostra-core/src/submit.rs`.
It is the last call of every run, and the engine reads only its payload. It is described with the executors, because
it ends the run rather than doing work: [The submit call](executors.md#the-submit-call).

## Project management tools

Management tools are calls an agent makes to Ostra itself rather than to the files it works on. The first
toolset manages projects, for a request that needs a codebase no project holds, such as a new service that
would otherwise have to live inside an existing repository. The implementer holds the `manage_projects`
capability by default, and any agent may request it (Rule CA6). The guard narrows it to one run: a run holding
the capability in a phase the approved plan puts in a project that does not exist yet. A project is therefore created only after the user approved the plan that
needs it, so a spec or plan the user rejects leaves nothing on disk.

### ProjectList

`ProjectList` takes no input. It returns the workspace root, which a new project's folder is relative to, and
each project's key, folder, stack, init status, and whether it is in this session's scope. It is read-only, so
every mode allows it.

### ProjectCreate

`ProjectCreate` takes:

| Input | Meaning |
| --- | --- |
| `key` | The project key, with the Add project dialog's rule: lowercase letters, digits, and dashes, starting with a letter or digit. |
| `stack` | The language and main framework in a few words, such as `rust`. One line, at most 64 characters. |
| `purpose` | What the codebase is for, at most 800 characters. The user approves the project from it. |
| `requirements` | 1 to 20 base requirements, one fact each and at most 400 characters: toolchain version, libraries with versions, build tool, transport, the systems it connects to. |
| `folder` | Optional, relative to the workspace root with no `..`. Defaults to the key. |
| `git_init` | Optional, default true: run `git init` in the new folder. |

The implementer (or whichever agent holds the grant) takes these from its phase file, where the plan copied them from the spec's `Constraint`
criteria. A call passes three checks before anything changes on disk:

1. **The guard (rule O2).** The `manage-tools` guard allows the call only from an execution whose context has
   `creates_project` set, which the runner sets only for a run holding `manage_projects` in a phase in a project the approved
   plan lists in `new_projects` and the session does not hold yet. The key must be that phase's project key.
   The guard also refuses a malformed input: a bad key or stack, an empty or oversized purpose, no requirements
   or too many, or a folder that is absolute or climbs with `..`. Checking here means the user is never asked
   about a call that cannot run. Until the project exists, the same run may write nothing outside its session
   dir and temp, because its `Repo root:` is its session dir and there is no project yet to write in.
2. **The permission (rule O1).** The call asks the user, in every permission mode including bypass, because
   creating a project changes Ostra, not only files. The card reads like
   ``Create project `notes-mcp` (rust) in `notes-mcp/` of the workspace: ...``, cut at 250 characters because
   Grok clips ask reasons. It offers no "always" rule, and answering "always in this workspace" adds none,
   because no allow rule may stand in for the answer. Only YOLO answers the ask; a `deny` rule on
   `ProjectCreate` still refuses it, and plan mode refuses it.
3. **The server's checks.** After the answer, the server (`crates/ostra-server/src/manage.rs`) refuses a key
   the workspace already has, and a folder that exists and is not empty, lies outside the workspace root (a
   symlinked parent is resolved first), lies in `.ostra`, or is inside or around another project. It also
   refuses when the session has ended, when it is not a pipeline session, when the run that asked has ended,
   or when the key is already in the session.

Then the server creates the folder, runs `git init` when asked (with the same no-exec git settings as a clone),
registers the project in `workspace.toml` with its stack, and appends a `ProjectCreated` event to the session.
A failed `git init` or registration removes what it created. The event stops the implementer that asked, and
the phase starts over inside the new project once its init has run. What happens next is in
[The pipeline](pipeline.md#a-new-codebase).

### How both executors reach them

The tools crate knows no workspace or engine, so it defines a `Manage` trait and the server implements it. When
an execution opens, the server hands it a handle bound to its workspace, session, and execution, and only when
its agent has the `manage_projects` capability and runs in a session. The native loop runs the call through that
handle in process. A harness sees `project_list` and `project_create` on Ostra's MCP server, which lists them only
for an execution that holds the handle, and checks, asks, and runs each call once, like `report`.

## Workspace MCP tools

The MCP servers listed in a workspace's `mcp_servers` reach every agent on every executor, because Ostra is the only MCP
client. A tool keeps the canonical name `mcp__<server>__<tool>` in the native loop, in the Activity view, and in
permission rules. A harness sees it as `mcp__ostra__<server>__<tool>`, through Ostra's own MCP server.

The tool list is fixed when an execution opens. A server that cannot be reached adds one Activity line, and the run
continues without it. Results are text: images and binary resources become a one-line note, structured content
becomes JSON, and results are cut at 100 KiB. [MCP servers](mcp.md) covers transports, sign-in, and naming.

## Where to look in the code

| Concern | File |
| --- | --- |
| Dispatch, canonical calls, per-execution state | `crates/ostra-tools/src/lib.rs` |
| Tool descriptions and input schemas | `crates/ostra-tools/src/defs.rs` |
| Read, Write, Edit | `crates/ostra-tools/src/fs.rs` |
| Bash | `crates/ostra-tools/src/bash.rs` |
| Grep, Glob | `crates/ostra-tools/src/search.rs` |
| WebFetch | `crates/ostra-tools/src/web.rs` |
| Skill, Report, Memory, MemoryRecall, DocsSearch | `crates/ostra-tools/src/misc.rs` |
| Book passages and ranking for DocsSearch | `crates/ostra-core/src/book_search.rs` |
| Document | `crates/ostra-tools/src/doc.rs` |
| ProjectList, ProjectCreate: input rules, the handle, the server side | `crates/ostra-core/src/manage.rs`, `crates/ostra-tools/src/manage.rs`, `crates/ostra-server/src/manage.rs` |
| Capability to tool name per executor | `assets/tool-mapping.toml` |
| Tool families for permissions | `crates/ostra-policy/src/perms.rs` |

Next: [The code index](code-index.md) explains what the code navigation tools read from.
