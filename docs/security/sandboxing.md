# Sandboxing

Sandboxing is a very powerful feature of Ostra. It makes it safe to let agents run real commands on your machine:
builds, tests, package installs, and whole harness CLIs such as Claude Code and Codex. Every one of those processes
starts inside an OS sandbox (bubblewrap on Linux, Seatbelt on macOS). The kernel enforces it, not the agent or the
CLI, so these guarantees hold whatever a command turns out to do:

- **Your secrets stay out of reach.** SSH and GPG keys, cloud and registry credentials, browser profiles, the
  keychain, other CLIs' sign-ins, and Ostra's own data dir are hidden. A command that walks the disk with `find ~`
  does not see them.
- **Nothing is left behind to run later.** Shell startup files, login and autostart entries, and `.git/config` and
  hooks are read-only, so an agent cannot plant a program that runs the next time you open a terminal or run git.
- **Writes stay in the workspace.** The repo, the session, a private `/tmp`, and per-workspace tool caches are
  writable; the rest of the machine is read-only, your own `~/.cargo` and `~/.npm` included.
- **Your session stays closed.** The desktop bus, the SSH agent, the display, and Docker and Podman sockets are
  hidden or unset, and a command cannot signal a process outside its sandbox.
- **It is on by default and fails closed.** The default mode, `required`, refuses to start an execution the
  machine cannot sandbox, and a repository cannot turn its own sandbox off.
- **It needs no container or VM.** macOS ships Seatbelt, and Linux needs only the `bubblewrap` package. There is
  no image to maintain, and the toolchain you build with on the host builds inside it.

## Why a sandbox as well as the policy

The policy engine reads every tool call before it runs (see [agent containment](agent-containment.md)), but a
tool call is only what an agent asked for. A `Bash` call named `npm test` can run a postinstall script, a build
macro, or a test that opens `~/.ssh/id_ed25519`, and none of that appears in the call the policy approved. A
harness CLI can also act without reporting a hook for it. The sandbox is the layer that covers what the policy
cannot read: Ostra starts every agent command, every harness CLI, and every program it runs for a project inside
an OS sandbox profile, so the kernel refuses what the profile does not allow, whatever the program does.

This page explains which processes run sandboxed, what the profile lets them read and write, how one profile
renders for bubblewrap on Linux and Seatbelt on macOS, how the mode is chosen and why a folder cannot change it,
what happens when no sandbox is available, and what the sandbox does not cover.

The code is one module, [`crates/ostra-core/src/sandbox.rs`](../../crates/ostra-core/src/sandbox.rs), plus the
harness wrapping in [`crates/ostra-exec-harness/src/sandbox.rs`](../../crates/ostra-exec-harness/src/sandbox.rs).
It lives in `ostra-core` because every crate that starts a process (the tools, both executors, the code index,
the engine, and the server) depends on it.

## Policy and sandbox

The policy and the sandbox answer different questions:

| | Policy (`ostra-policy`) | Sandbox (`ostra-core::sandbox`) |
| --- | --- | --- |
| Sees | One canonical `ToolCall` before it runs | Every syscall of the process tree after it starts |
| Decides on | Paths and commands the call names | Paths, sockets, and processes the kernel is asked for |
| Can explain | Yes: a denial carries the correction | No: a program gets `EPERM`, `EROFS`, or an empty dir |
| Covers | Native tools and harness hooks | Native Bash, harness CLIs, project programs |

Both read the same lists. `paths::HOME_CREDENTIALS` is the one list of credential stores: the policy refuses a
tool call that names one, Grep and Glob skip them when they walk, and the sandbox hides them
([`crates/ostra-core/src/paths.rs`](../../crates/ostra-core/src/paths.rs)). A path the policy refuses by name is
therefore also a path a shell command cannot reach by walking the disk.

The sandbox does not replace the policy. Write scope, state ownership, the report path, and the other guards
depend on which agent is running and which stage it is in, and the sandbox profile knows neither. The profile's
job is narrower: keep every process Ostra starts inside the workspace, away from the user's secrets and session,
and unable to leave anything behind that runs later.

## What runs sandboxed

There are two profile builders, one for agents and one for the project's own programs.

**Agent executions** use `Profile::for_execution`:

- **Native Bash.** Each `Bash` tool call starts a fresh sandbox (a new `bwrap` or `sandbox-exec` process) around
  `bash -c`. The native executor builds the profile once per execution
  ([`sandbox_for`](../../crates/ostra-exec-native/src/lib.rs)) and the tool wraps each command in it
  ([`crates/ostra-tools/src/bash.rs`](../../crates/ostra-tools/src/bash.rs)).
- **Harness CLIs.** Claude Code, Codex, Grok, and Antigravity start inside the sandbox as a whole process, so the
  CLI's own shell tool and everything it starts inherit it
  ([`sandbox::wrap`](../../crates/ostra-exec-harness/src/sandbox.rs)). This is why the sandbox covers actions a
  harness never reports to a hook.

**Programs Ostra starts for a project** use `Profile::for_program` through `sandbox::host_command`:

| Program | Started by | Writable roots |
| --- | --- | --- |
| Format command | the runner, after the user approved it (Rule A1) | the project root |
| Language servers | the code index ([`lsp/mod.rs`](../../crates/ostra-code/src/lsp/mod.rs)) | the project root |
| Code providers | the code index ([`provider.rs`](../../crates/ostra-code/src/provider.rs)) | the project root |
| Stdio MCP servers | the workspace MCP pool ([`mcp.rs`](../../crates/ostra-server/src/mcp.rs)) | the workspace root |

These run sandboxed because each one executes code from the repository: a language server runs build scripts and
macros, a format command is whatever the project configured, and an MCP server is the workspace's own program.
An unapproved workspace file starts none of them in the first place (see [workspaces](../internals/workspaces.md)).

Ostra's own git operations (staging after a build, commits and pushes from the console) run outside the sandbox, because they are
Ostra's code with fixed arguments and they need `.git/config` and the data dir that the profile closes. HTTP MCP
servers are not local processes, so no sandbox applies to them; Ostra's own HTTP client reaches them.

## What an agent can reach

The profile is a list of path rules, each one of four kinds: writable, read-only, hidden, or (on bubblewrap only)
a link. Everything starts read-only: the base of the profile is the whole filesystem mounted read-only, so an
agent can read the toolchain, system headers, and the repo's dependencies without a rule for each one.

### Writable

- The workspace root, the repo root, the session root, and the execution's session dir.
- `.git` of each repo, bound writable onto itself. Commits, branches, and the index work; the bind also means
  `.git` cannot be renamed away (see [git](#git-stays-usable-and-closed)).
- A private `/tmp` (see [temporary files](#temporary-files)).
- The per-workspace tool caches (see [tool caches](#tool-caches)).
- Each entry of `[sandbox] extra_writable`.

### Read-only

- Every path the profile does not name, including the rest of the home folder.
- **Shell startup and autostart files** (`PERSISTENCE_IN_HOME`): `.bashrc`, `.profile`, `.zshrc`, `.zshenv` and the
  other shell rc files, `.config/fish/`, `.gitconfig` and `.config/git/`, `.config/autostart/`,
  `.config/systemd/`, `.config/environment.d/`, `.pam_environment`, `.xprofile`, `.npmrc`, `.local/bin/`,
  `.cargo/bin/`, `.cargo/config(.toml)`, and on macOS `Library/LaunchAgents/`, `Library/Preferences/`,
  `Library/Scripts/`, `Library/Services/`, and iTerm2 scripts. These are named even inside a writable dir,
  because each one runs a program the next time the user opens a shell or logs in, so an agent that could write
  one could leave a program behind for the user.
- **Git's executable config**: `.git/config`, `.git/hooks/`, `.git/info/`, and the same three in every submodule
  gitdir under `.git/modules/`.
- **Ostra's own state**: `.ostra/workspace.toml` and the workspace db, the project memory db (with its `-wal`,
  `-shm`, and `-journal` files), the session's `.state` dir, and every path the execution lists as protected.
- **Workspace artifacts**: `.ostra/artifacts/` is read-only, because agents read workspace artifacts and never
  write them (Rule W1).
- **The user's own caches**: `~/.cargo`, `~/.npm`, `~/.cache`, and `~/Library/Caches` stay read-only, because
  the user's builds on the host run what those caches hold.
- `<data dir>/assets`, revealed read-only inside the hidden data dir, because agents read skills and references
  from it.

A protected file that does not exist yet still needs to stay uncreatable: otherwise an agent could create
`.git/hooks/pre-commit` or `~/.zshenv` where there was none. On bubblewrap, Ostra creates a placeholder on the
host (an empty file, or `{}` for a `.json` file, mode 0600, or an empty dir) and binds it read-only, but only where
the parent is a writable dir the sandbox already exposes; a missing path under a read-only parent needs nothing.
On Seatbelt a deny rule (`literal` for a file, `subpath` for a dir) refuses creation of the path directly, in any
letter case, so no placeholder is written.

### Hidden

Hidden paths are not readable at all. On bubblewrap a hidden dir is an empty tmpfs and a hidden file is
`/dev/null`; on Seatbelt both return `EPERM`.

- **Ostra's data dir**: the registry, the secret store, the server log, session databases, and hidden workspace
  artifacts (Rule W2). This is also where Ostra puts the temporary SSH key for pushes, for the same reason.
- **Ostra's config dir**, or only the file `OSTRA_CONFIG` points at when it is set, and the master key file
  (`OSTRA_MASTER_KEY_FILE`).
- **Credential stores** (`HOME_CREDENTIALS`): `.ssh`, `.gnupg`, `.aws`, `.azure`, `.config/gcloud`, `.kube`,
  `.docker`, `.netrc`, `.git-credentials`, the `gh` and `hub` tokens, cargo and PyPI credentials, `.vault-token`,
  Terraform credentials, keyrings, `.password-store`, `.Xauthority`, browser profiles, and on macOS
  `Library/Keychains`, cookies, Safari, Mail, Messages, the TCC database, and app containers. Harness sign-in
  files (`.claude/.credentials.json`, `.codex/auth.json`, `.grok/auth.json`, `.gemini/oauth_creds.json`) are on
  the list too; a harness execution keeps only its own sign-in file visible, so Claude Code cannot read the
  Codex login.
- **The user session**: `$XDG_RUNTIME_DIR` (or `/run/user/<uid>`), which holds the D-Bus session bus, the Wayland
  socket, and agent sockets.
- **Container sockets**: `/run/docker.sock`, `/var/run/docker.sock`, the Podman socket, and the OrbStack, Colima,
  Rancher Desktop, and Podman machine sockets. They are hidden, not read-only, because a read-only file does not
  stop `connect`, and access to a Docker socket is root on the host.
- **Other sessions**: `.ostra/sessions/` is hidden, and the running session is bound back below it, so an agent
  cannot read or edit another session's reports.
- **Other executions' harness dirs**: `.state/harness/` is hidden, because each harness dir holds that
  execution's bridge token.
- Each entry of `[sandbox] extra_hidden`.

The environment is cleaned the same way. `SSH_AUTH_SOCK`, `DBUS_SESSION_BUS_ADDRESS`, `DISPLAY`,
`WAYLAND_DISPLAY`, `XAUTHORITY`, `DOCKER_HOST`, and `GPG_AGENT_INFO` are unset, because each one points at a
service that acts with the user's rights. Native Bash commands also lose provider keys, bridge tokens, MCP OAuth
client secrets, and every `OSTRA_*` variable, and project programs lose provider keys, bridge tokens, and every
`OSTRA_*` variable. A harness CLI inherits Ostra's environment minus every credential variable except its own
sign-in key, and gets fresh bridge variables for its own execution (see [secrets and data](secrets-and-data.md)).

### A deeper rule overrides a shallower one

Rules overlap: the repo is writable, `.git/hooks` inside it is read-only, and `.ostra/sessions/<this session>`
is writable inside a hidden dir. `Profile::ordered` sorts every rule by path depth, so a deeper rule is applied
after its parent and wins. When two rules name the same path, only the most restrictive one is kept, in the order
writable, read-only, hidden. Bubblewrap applies the list as mounts in that order, and Seatbelt emits it as SBPL
rules in that order, where a later rule overrides an earlier one. One ordering serves both backends.

One case is removed on purpose. A read-only bind below a hidden dir would expose that path again, so the plan drops
it unless the rule is marked as a reveal, which only `<data dir>/assets` is.

Paths are canonicalized first, because bubblewrap mounts real paths and Seatbelt matches real paths. A rule for
`~/work/repo` where `~/work` is a symlink applies to the target.

The test `deeper_rules_win_and_git_stays_usable` in `sandbox.rs` runs this ordering in a live sandbox: the
execution's own harness dir is writable, another session is hidden, a commit succeeds, and `git config` and hook
writes are refused.

### Git stays usable and closed

An agent needs git to inspect history and commit, and the pipeline stages changes with `git add -A` after every
build. Git also reads
programs to run from its config: `core.fsmonitor`, `core.sshCommand`, hook scripts, and filters. The profile keeps
both properties:

- `.git` is writable, so objects, refs, and the index can change.
- `.git/config`, `.git/hooks/`, and `.git/info/` are read-only in each repo and each submodule gitdir, so an agent
  cannot plant a hook or an fsmonitor command that runs later outside the sandbox, when the user or Ostra runs git.
- `.git` is bound onto itself. A bind-mount point cannot be renamed, so `mv .git .g && git init` cannot swap in a
  new config. On Seatbelt, which has no mounts, a `file-write-unlink` deny on every ancestor of a protected path
  inside a writable dir does the same job.
- Every git process a sandboxed command starts gets `core.fsmonitor=false`, `safe.bareRepository=explicit`, and
  `protocol.ext.allow=never` through `GIT_CONFIG_COUNT`, so a malicious repo config the agent did not write still
  cannot start a program ([`crates/ostra-core/src/git.rs`](../../crates/ostra-core/src/git.rs)).

The repos covered are the workspace root, the repo root, and their direct children that contain a `.git` dir. A
checkout whose `.git` is a file, such as a worktree or a submodule checkout, is not covered.

### Temporary files

The shared `/tmp` is where other programs leave sockets, lock files, and build output, so it is not shared.

- **Native Bash.** Each execution gets its own scratch dir, `$TMPDIR/ostra-scratch-<uuid>` (mode 0700), mounted at
  `/tmp` on bubblewrap. It persists across the execution's Bash calls and is removed when the execution ends.
  Read and Write map a `/tmp/...` path to the scratch dir on the host (`Profile::to_host`), so a file the shell
  wrote in `/tmp` is the file the Read tool opens.
- **Harness CLIs.** `/tmp` is `<harness dir>/tmp`, removed with the execution.
- **Project programs.** An empty tmpfs on bubblewrap, and the workspace's cache `tmp/` on Seatbelt.

On Seatbelt, which cannot remount `/tmp`, the policy denies `/private/tmp`, `/private/var/tmp`, and the user's
Darwin temp dir, and sets `TMPDIR` (and `TMPPREFIX` for zsh heredocs) to the scratch dir instead. Programs that
honor `TMPDIR` work; a program that hard-codes `/tmp` fails with `EPERM`.

### Tool caches

A build inside the sandbox needs to download crates and packages, but it must not write the user's own caches,
because the user's host builds would then run what an agent put there. The sandbox gives each workspace its own
caches under `~/.cache/ostra/sandbox/<hash>/` (or `$OSTRA_SANDBOX_CACHE/<hash>`), created 0700, and points the
tools at them. Agent executions hash the workspace root; project programs hash their first root, the project
root, so they keep a cache apart from the agents'.

| Variable | Cache dir |
| --- | --- |
| `CARGO_HOME` | `cargo` |
| `npm_config_cache` | `npm` |
| `npm_config_store_dir` | `pnpm-store` |
| `XDG_CACHE_HOME` | `xdg` |
| `GOMODCACHE`, `GOCACHE` | `go-mod`, `go-build` |
| `PIP_CACHE_DIR` | `pip` |
| `YARN_CACHE_FOLDER` | `yarn` |
| `UV_CACHE_DIR` | `uv` |
| `CLANG_MODULE_CACHE_PATH` | `clang-modules` |

The user's `~/.cargo/config.toml` and `~/.cargo/config` are linked into the sandbox `CARGO_HOME`, so registry mirrors and build settings
carry over, and cargo's credentials file is not.

Two consequences follow. The first build in a workspace downloads its dependencies again, and an agent's build
and the user's build do not share compiled dependencies. `~/.rustup` stays read-only, so a toolchain the project
pins but the user has not installed fails inside the sandbox; install it on the host once.

### Network

`[sandbox] network` (default `true`) sets network access for native Bash. With `false`, bubblewrap unshares the
network namespace, so a command has no network at all, loopback included. On Seatbelt, `false` drops the rules that allow IP
traffic and DNS. With `true`, bubblewrap keeps the host network, and Seatbelt allows outbound IP, localhost binds
and inbound local connections, and DNS through mDNSResponder. On Seatbelt, Unix sockets are allowed only inside
writable dirs, whatever the setting, and denied everywhere else.

Harness CLIs and project programs always get the network, whatever the setting says. A harness needs its model
API and Ostra's hook bridge on loopback, and language servers and MCP servers fetch dependencies and call their
own services. The network is the boundary the sandbox leaves open: a sandboxed command with network can send anything
it can read. What it can read is the workspace and the rest of the read-only filesystem, and the hidden list is
what keeps secrets out of that.

## Harness CLIs in the sandbox

A harness CLI is a larger program than a shell command, with its own state dirs, its own sign-in, and in two
cases its own sandbox. [`sandbox::wrap`](../../crates/ostra-exec-harness/src/sandbox.rs) adds what each one needs
to `Profile::for_execution`:

- **Its state dir is writable, and its instruction files are not.** Each CLI writes session history and caches
  under its home dir, so that dir is writable. The files in it that run in the user's own later sessions of that
  CLI stay read-only: a hook or instruction written there would reach outside Ostra.

  | Harness | Writable | Read-only inside it |
  | --- | --- | --- |
  | Claude Code | `$CLAUDE_CONFIG_DIR` or `~/.claude`, and `~/.local/state/claude` | `settings.json`, `settings.local.json`, `CLAUDE.md`, `commands/`, `agents/`, `skills/`, `plugins/`, `hooks/`, `output-styles/` |
  | Codex | `$CODEX_HOME` or `~/.codex` | `config.toml`, `AGENTS.md`, `hooks.json`, `rules/` |
  | Grok | `$GROK_HOME` or `~/.grok` | `config.toml`, `trusted_folders.toml`, `hooks/` |
  | Antigravity | `~/.gemini` | `settings.json`, `GEMINI.md`, `config/plugins/` |

- **The home folder is disposable on bubblewrap.** The CLI's home is a tmpfs with each existing entry bound back
  read-only, so a new file at the top of home lands in the tmpfs and is gone when the execution ends. Claude Code
  rewrites `~/.claude.json` through a temp file and a rename, which a read-only home refuses, so Ostra copies the
  real file into the execution's dir (mode 0600) and links it in; the real file is never changed. Seatbelt has
  no mounts and no disposable home, so the rest of home is read-only there and a top-level write fails; the CLIs
  continue without it.
- **The PTY stays the controlling terminal.** Native Bash runs with `--new-session`, which blocks `TIOCSTI` input
  injection into the terminal Ostra was started from. A harness runs in a PTY that must be its controlling
  terminal, so that flag is off, and on Seatbelt the policy allows exactly that PTY's device (`-D TTY=<path>`)
  and every other terminal stays denied.
- **The CLI's own sandbox is off inside Ostra's.** When Ostra runs Claude Code sandboxed, it writes
  `"sandbox": {"enabled": false}` into the execution's settings, because macOS cannot nest Seatbelt sandboxes and
  Ostra's profile already covers the CLI. Codex always runs with `--dangerously-bypass-approvals-and-sandbox`,
  because Ostra's hooks and sandbox take the place of its approvals; this flag is passed whether or not Ostra's
  sandbox is active, so a Codex execution in mode `off` has no sandbox at all.
- **Only its own sign-in is visible.** The CLI keeps its own sign-in file and its own API key variables (for
  example `ANTHROPIC_API_KEY` for Claude Code, `OPENAI_API_KEY` for Codex). Every other harness's sign-in and
  every other provider key is hidden or removed. The agent's own shell commands inside that CLI can read the
  CLI's key, because they run in the same process tree.

When the execution ends, Ostra stops the process group (SIGTERM, then SIGKILL after 3 seconds), releases the
sandbox's process marker, and removes the `~/.claude.json` copy, the Grok config that holds the bridge token, and
the scratch `tmp/`. The per-harness details are in [executors](../internals/executors.md).

## One profile, two backends

A `Profile` describes rules, not mounts. `Profile::command` renders it for whichever backend the machine has.

### Bubblewrap on Linux

Bubblewrap builds a new mount namespace from the rules:

1. `--ro-bind / /`, `--dev /dev`, `--proc /proc`: the read-only base, a fresh `/dev`, and a `/proc` for the new
   pid namespace.
2. `/tmp` bound to the scratch dir, or a tmpfs.
3. The ordered rules: `--bind` for writable, `--ro-bind` for read-only, `--tmpfs` for a hidden dir,
   `--ro-bind /dev/null` for a hidden file, `--symlink` for links.
4. `--unsetenv` for the session variables and `--setenv` for the cache and temp variables.
5. `--unshare-pid --unshare-ipc`, and `--unshare-net` when the network is off.
6. `--die-with-parent`, and `--new-session` outside a PTY.

The pid namespace means a sandboxed command sees only its own processes, so it cannot read another process's
environment or command line, or send it a signal. `--die-with-parent` means every process in the sandbox ends
when Ostra stops the command, including one that detached.

Ostra calls `bwrap` from `PATH` and test-runs it at startup
(`bwrap --ro-bind / / --dev /dev --proc /proc --unshare-pid true`). The run fails where unprivileged user
namespaces are blocked, which is the case in many containers and on Ubuntu 24.04 with its AppArmor default; the
message names the fix. Ostra reports no known gaps for bubblewrap (`known_gaps`); the limits under
[what the sandbox does not cover](#what-the-sandbox-does-not-cover) still apply.

### Seatbelt on macOS

Seatbelt applies a policy to a process and its children with no mount namespace. Ostra writes an SBPL policy and
runs the program through `/usr/bin/sandbox-exec -p <policy>`. It calls that absolute path, because a Homebrew
prefix on `PATH` is writable by the user, and it refuses a binary that is not owned by root or that group or
others can write.

The policy starts from `(deny default)` and allows process exec and fork, reading files, `sysctl` reads, POSIX
semaphores and shared memory, preference reads, a short list of system mach services (directory lookup, the
notification center, logging, `trustd`, `configd`, `cfprefsd`, and `dirhelper`), and signals and process info only
for processes in the same sandbox. Everything else stays denied, including the keychain, the pasteboard, Apple
Events (so `osascript` cannot drive another app), LaunchServices (`open -a`), `launchctl submit`, the window
server, TCC, and preference writes (`defaults write`). The ordered rules follow: `file-write*` allowed for
writable paths, denied for read-only paths, and `file-read*` and `file-write*` both denied for hidden paths.

Seatbelt has two differences from bubblewrap that change what a program sees:

- **A hidden path returns `EPERM`,** not an empty dir. A program that walks home reports the error and continues.
- **`/tmp` is denied, not private,** and `TMPDIR` points at the scratch dir.

Seatbelt matches paths as strings, so the policy covers the ways to name a path twice: rules match in any letter
case (APFS is case-insensitive, so `.SSH` is `.ssh`), and the tests cover hard links and the
`/System/Volumes/Data` firmlink. Ostra quotes every path into the policy and refuses a path with a control
character or non-UTF-8 bytes, so a folder name cannot rewrite the policy.

**Process lifetime.** macOS has no pid namespace or `--die-with-parent`, so Ostra marks each invocation instead.
Each policy allows the lookup of two mach names that no service registers: one unique to the invocation and one
shared by every sandbox of this data dir. When the invocation ends, Ostra lists the user's processes and asks the
kernel (`sandbox_check`) which ones are sandboxed with a policy that allows its name, then kills them, including
processes that called `setsid` or double-forked. At startup, when no other server is running, Ostra kills every
process that still carries the data dir's shared name, which cleans up after a server that crashed
([`kill_leftovers`](../../crates/ostra-server/src/app.rs)).

**Known gap.** A Seatbelt sandbox can read the arguments and startup environment of any process the same user
runs (`sysctl KERN_PROCARGS2`). Ostra clears its own startup environment first thing in `main`
(`scrub_startup_env`), but other programs the user runs are exposed, so keep tokens out of shell-profile exports.
Ostra reports this gap in the sandbox status, at startup, and on the setup screen.

Seatbelt cannot start inside another Seatbelt sandbox. If Ostra itself runs sandboxed (for example, from inside a
sandboxed app's terminal), the probe detects `sandbox_apply: Operation not permitted` and says to start Ostra
outside it.

Other operating systems have no backend; the probe answers "The sandbox needs Linux or macOS." Platform
specifics, container setups, and WSL are in [OS compatibility](../platforms/os-compatibility.md).

## Modes

`[sandbox] mode` in `config.toml` sets the default, and each workspace can set its own mode in its settings
(Settings, Permissions tab, Sandbox panel).

| Mode | With a backend | Without one |
| --- | --- | --- |
| `required` (default) | Sandboxed | The execution does not start, and the error says why and how to fix it |
| `auto` | Sandboxed | Runs unsandboxed, with a warning |
| `off` | Unsandboxed | Unsandboxed |

`required` is the default because a machine that silently loses its sandbox (a kernel update that blocks user
namespaces, a container that drops a capability) would otherwise keep running agents with the user's full
rights. Choose `off` only on purpose, for example in a disposable VM, or to test a harness the sandbox blocks.

The mode is decided per execution (`sandbox::decide`), from the global config read fresh and the workspace's
mode, so a change applies to the next execution with no restart. When the workspace's own mode changes, a running
language server is replaced on its next request, and a stdio MCP server reconnects, because the mode is part of
its connection fingerprint. A change to the global mode reaches those servers when they next start. The backend itself is probed once per server process.

### The mode is stored in the registry

The workspace's mode is kept in the registry, never in `.ostra/workspace.toml` (Rule A2), because a file in the
folder travels with the repository and a repository could otherwise turn its own sandbox off. A saved
`workspace.toml` never carries the field, and adopting a folder whose file sets it ignores it. The one exception
is a workspace registered before approvals existed: its old file values move into the registry once. See
[settings and routing](../internals/settings-and-routing.md) for the rest of Rule A2.

Only the mode can be set per workspace. `network`, `extra_writable`, and `extra_hidden` come from the global
config for every workspace, so a workspace cannot widen the paths an agent can write.

### Settings

```toml
[sandbox]
mode = "required"                    # "required", "auto", or "off"
network = true                       # false: native Bash commands get no network at all
extra_writable = ["~/.config/my-lsp"] # absolute, or starting with ~/
extra_hidden = ["~/work/other-client"]
```

Each path in `extra_writable` and `extra_hidden` must be absolute or start with `~/`, because a relative path
would depend on where an execution starts. An invalid path stops the server at startup; when the config changes
while the server runs, an invalid re-read is logged and the last valid config stays in effect.
`extra_writable` is the fix for a language server or MCP server that keeps its login or cache in a dir under home,
which the sandbox otherwise leaves read-only. `extra_hidden` closes other paths the default list does not know,
such as another client's checkout.

`OSTRA_SANDBOX_CACHE` moves the tool-cache root; the test suites set it.

## When there is no sandbox

Ostra reports it in these places:

- **At startup,** for the global mode, the log names the backend (`agent commands run in the bubblewrap sandbox`)
  and any known gaps. When a sandbox is wanted and missing, stderr carries `WARNING: agent commands run WITHOUT a
  sandbox, with your user's full rights.` With mode `off`, the log says the sandbox is off.
- **On the setup screen,** the "Agent command sandbox" check shows the backend and mode, or the reason and the fix
  (`GET /api/environment`).
- **In each execution,** in mode `auto` without a backend, a native execution shows the status line
  `Bash runs without a sandbox.` with the reason, and a harness execution shows `<harness> runs without a
  sandbox.` Mode `off` shows no status line, and mode `required` fails the execution with the reason.
- **In the workspace settings,** the Sandbox panel lists what is lost while the chosen mode runs commands
  unsandboxed, from the form's unsaved value, so the list appears before the save.
- **In the Artifacts tab,** a warning says shell commands can read hidden artifacts, with a button to the setting.

The status all of these read is `SandboxStatus` ([`api.rs`](../../crates/ostra-core/src/api.rs)): `mode`,
`available`, `backend`, `active`, `message`, and `gaps`. The workspace detail carries the workspace's own status,
the global config with the workspace's mode in its place.

The screenshot shows the Sandbox panel on the Permissions tab:

![The workspace Permissions tab with the Sandbox panel](../images/console/settings-permissions.png)

Without the sandbox, the policy still checks every tool call, and the file tools still refuse the data dir, the
credential stores, and hidden artifacts. What is lost is everything a command does that its tool call does not
name: `find ~` reads the credential stores, a build script can write anywhere the user can, including `~/.bashrc`
and `.git/hooks`, and `[sandbox] network = false` does not apply. Programs Ostra starts for a project run
unsandboxed in that case too, without a status line of their own.

## What the sandbox does not cover

These limits follow from the design, and the sections above give the reasons:

- **Network exfiltration.** A sandboxed command with network can send what it can read. Hiding secrets is the
  defense, not closing the network, except for native Bash with `network = false`.
- **Abstract Unix sockets on Linux.** They live in the network namespace, so with the network on, a command can
  connect to one the host exposes. `network = false` closes them for native Bash.
- **Nested repositories below the first level.** Git's config is protected in the workspace root, the repo root,
  and their direct children. A repo nested deeper has a writable `.git/config`.
- **Paths that are not on the hidden list.** A secret in an unusual place, such as a token file in the repo or a
  key in `~/Documents`, is readable unless `extra_hidden` names it.
- **What the agent is allowed to change.** The sandbox lets an agent write the whole workspace. Stage-level limits
  (a reviewer that must not edit code, a builder confined to its paths) are the policy's write scope.
- **Seatbelt's process arguments gap**, described under [Seatbelt](#seatbelt-on-macos).

## Where to look in the code

| Topic | File |
| --- | --- |
| `Profile`, both backends, the lists, `decide`, `host_command`, the probe | [`crates/ostra-core/src/sandbox.rs`](../../crates/ostra-core/src/sandbox.rs) |
| `HOME_CREDENTIALS` and the data dir | [`crates/ostra-core/src/paths.rs`](../../crates/ostra-core/src/paths.rs) |
| `SandboxConfig`, `SandboxMode`, `validate_sandbox` | [`crates/ostra-core/src/config.rs`](../../crates/ostra-core/src/config.rs) |
| `SandboxStatus` | [`crates/ostra-core/src/api.rs`](../../crates/ostra-core/src/api.rs) |
| Native Bash wrapping, scratch `/tmp`, env scrub | [`crates/ostra-tools/src/bash.rs`](../../crates/ostra-tools/src/bash.rs) |
| The profile per native execution | [`crates/ostra-exec-native/src/lib.rs`](../../crates/ostra-exec-native/src/lib.rs) |
| Harness wrapping and state dirs | [`crates/ostra-exec-harness/src/sandbox.rs`](../../crates/ostra-exec-harness/src/sandbox.rs) |
| CLI flags that turn a harness's own sandbox off | [`crates/ostra-exec-harness/src/launch.rs`](../../crates/ostra-exec-harness/src/launch.rs) |
| The mode in the registry (Rule A2) | [`crates/ostra-server/src/trust.rs`](../../crates/ostra-server/src/trust.rs) |
| Startup status and leftover cleanup | [`crates/ostra-server/src/app.rs`](../../crates/ostra-server/src/app.rs) |
