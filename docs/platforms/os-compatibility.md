# OS compatibility

Run Ostra on Linux or macOS to get the full sandbox. Ostra also compiles and runs on Windows, but without a
sandbox. WSL 2 works as a Linux machine. This page explains the differences between the systems. It also tells
which of these claims a person has run and checked.

Ostra depends on the operating system in more places than a usual web server does. The reason is that Ostra
starts other programs for you:

- A Bash shell for the native agents.
- A harness CLI in a terminal.
- MCP servers.
- Language servers.
- Git.

Ostra must confine those programs, stop them cleanly, and keep your credentials away from them. On Linux and
macOS, each of these jobs uses a Unix facility. On Windows, Job Objects stop process trees and ConPTY runs
terminals. But on Windows, no sandbox confines the commands of an agent yet.

## Support matrix

| | Linux | macOS | WSL 2 | Windows |
| --- | --- | --- | --- | --- |
| Builds | Yes | Yes | Yes | Yes (`x86_64-pc-windows-msvc`) |
| Agent sandbox | bubblewrap | Seatbelt (`sandbox-exec`) | bubblewrap, when the system allows user namespaces | None yet. The default mode is `auto` |
| Shell for the Bash tool | `bash` | `bash` | `bash` | Git Bash (Git for Windows), required |
| Extra shell tools | None | None | None | PowerShell and Cmd |
| Harness terminals (PTY) | Yes | Yes | Yes | Yes, through ConPTY |
| Stopping a process tree | Process group | Process group, and the sandbox marker | Process group | Job Object |
| Master key in the OS keychain | Secret Service over D-Bus, else a file | Keychain | Usually the file | Credential Manager, else a file |
| Background service | systemd user unit (`install.sh`) | launchd agent (`install.sh`) | systemd user unit, when WSL runs systemd | Task Scheduler task at logon (`install.ps1`) |
| Docker image | Yes (the image is Linux) | Through the Linux VM of Docker Desktop | Through Docker | Through the Linux VM of Docker Desktop |
| How we know | Tested | Sandbox measured on macOS 26 | Inferred from the code | Test suite run on Windows 11 |

"Tested" means that the full test suite runs on that system, and that a person uses Ostra on it each day.
"Inferred" means that the answer comes from a reading of the code, and that nobody ran it. The next section
gives the details.

## What has been run, and what has not

CI (`.github/workflows/ci.yml`) runs on Ubuntu 24.04 only: clippy, the Rust tests without the evals, the checks and tests of
`design/`, `web/`, and `site/`, and the browser suite. Each result below for another system comes from a person who ran
the tests or the server by hand.

- **Linux** is the system where the developers work on Ostra. These tests run on Ubuntu with kernel 6.8 and
  bubblewrap 0.9:
  - The workspace tests (`cargo test --workspace`).
  - The sandbox tests.
  - The end-to-end server test.
  - The browser security suite.

  The Docker image compiles on Ubuntu 26.04. Other distributions can work when they ship bubblewrap and allow
  unprivileged user namespaces. But nobody tried each one.
- **macOS** has a Seatbelt sandbox. A person measured it probe by probe on macOS 26. It has its own tests: the
  Seatbelt tests in [`tests.rs`](../../crates/ostra-sandbox/src/tests.rs), and
  [`env_scrub.rs`](../../crates/ostra-sandbox/tests/env_scrub.rs). The Seatbelt tests skip a system where Seatbelt
  does not work. Nobody checked older macOS releases.
- **WSL 2** runs a real Linux kernel. Thus the Linux build and bubblewrap apply without a change. This statement
  comes from a reading of the code, not from a run.
- **Windows**: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` pass on
  Windows 11 Pro 24H2 (build 26100), x86-64. The setup has the MSVC toolchain, Rust 1.98.1, and Git for Windows
  2.54. The `web/` checks, type check, and unit tests also pass there. The Windows-only tests cover these items:
  - The Job Object kill.
  - The ConPTY terminal.
  - Git Bash discovery.
  - The PowerShell and Cmd tools.
  - The path guards.

  The server starts and serves the console. It stores its master key in the Credential Manager and reads it
  back. Nobody ran the harness CLIs or a live session with a model on Windows.
- **CPU architecture** has no special case in the code. The tests run on x86-64. Arm64 (Apple silicon, Graviton,
  a Raspberry Pi 5) has no architecture-specific code that stops it. But no test run supports this statement.

## Windows

Ostra compiles and runs on Windows, and agents run. But the sandbox does not confine the commands of an agent.
This list tells what works and what does not:

- **No sandbox yet.** No Windows facility gives an unprivileged process its own view of the file system, as
  bubblewrap does. Thus Windows has no backend. The default mode on Windows is `auto`. This mode runs agent
  commands unsandboxed, and it shows a warning at startup, on the setup screen, and in each execution. The mode
  `required` refuses each execution, with a reason that names WSL 2 or Docker. The policy still checks each tool
  call. [Sandboxing](../security/sandboxing.md#windows) lists what stays open, for example registry `Run` keys
  and scheduled tasks. To get the full sandbox on a Windows machine, run Ostra in WSL 2 or Docker.
- **Git for Windows is required.** The Bash tool runs `bin\bash.exe` of Git Bash. Ostra finds it from
  `git --exec-path` or the default install dirs. Ostra never uses the `bash.exe` in `System32`, because that file
  is the WSL launcher. The setup screen shows if Ostra found Git Bash.
- **PowerShell and Cmd tools.** On Windows, agents with the shell capability also get the `PowerShell` and `Cmd`
  tools. The policy cannot parse those commands. Thus it never allows one without a question to you
  ([Agent containment](../security/agent-containment.md#powershell-and-cmd)).
- **Ostra compares paths in the same way that NTFS names files.** The guards do these things
  ([Windows paths](../security/agent-containment.md#windows-paths)):
  - They follow junctions.
  - They ignore letter case.
  - They resolve 8.3 names.
  - They remove trailing dots and spaces.
  - They translate the `/c/...` paths of Git Bash.
  - They refuse shares, device paths, and alternate data streams.
- **Process trees are Job Objects.** See [Processes and signals](#processes-and-signals).
- **Harness CLIs run in ConPTY**, the Windows pseudoconsole. They use the same `portable-pty` crate.
- **Harness commands resolve through `PATHEXT`.** Ostra finds a harness command itself, and it accepts only a file
  with a `PATHEXT` extension. The reason is that npm installs a CLI as an extensionless `sh` script for Git Bash,
  next to its `.cmd` file. Windows cannot start that script. For an npm `.cmd` shim, Ostra runs its JavaScript
  entry point directly with `node`. The reason is that `cmd.exe` cannot pass the multi-line arguments that a
  harness gets. Ostra refuses each other batch file, with a message that tells you to set the harness command to
  an `.exe` ([`command.rs`](../../crates/ostra-exec-harness/src/command.rs)).
- **Owner-only files use the ACL of the profile.** Windows has no Unix file modes. The data dir is under
  `%LOCALAPPDATA%`. The inherited ACL of that folder gives access only to you, SYSTEM, and administrators. The
  files that Ostra creates there inherit this ACL. Ostra sets no explicit ACL of its own.
- **An empty dir turns off Git hooks in the git calls of Ostra.** Git for Windows reads
  `core.hooksPath=/dev/null` as `\dev\null` on the current drive. Any local user can create folders there. Thus
  on Windows, the setting names an empty `no-hooks` dir in the data dir of Ostra
  ([`git.rs`](../../crates/ostra-core/src/git.rs)).
- **Git never waits for a sign-in window.** The git commands of Ostra set `GCM_INTERACTIVE=never`. Thus Git
  Credential Manager answers from stored credentials. It never opens a dialog that no person can see.
- **The hook bridge uses loopback HTTP.** On Linux, sandboxed harnesses reach the bridge through a Unix socket.
  Ostra does not create this socket on Windows. Unsandboxed harness hooks call `/internal/*` on `127.0.0.1`. They
  do the same on Linux in mode `off`.

`install.sh`, `build.sh`, and `run.sh` are bash scripts for Linux and macOS. On Windows, `install.ps1` compiles
Ostra, installs it, and registers the service. To compile by hand, follow these steps
([Install](../start/install.md#windows)):

1. Run `npm ci` and `npm run build` in `web/`.
2. Run `cargo build --release`.
3. Start `target\release\ostra.exe`.

## The sandbox on each system

The sandbox is the main reason that the operating system is important. The policy layer examines each tool call
by the paths that it names. But a shell can hide a path in a variable or a script. Thus the kernel enforces a
second boundary below the policy. One `Profile` in [`profile.rs`](../../crates/ostra-sandbox/src/profile.rs)
describes that boundary:

- The host file system is read-only.
- Only the roots of the execution are writable: the worktree or project that it works on, its scratch dir, and
  its tool caches.
- The sandbox hides these items:
  - The data dir of Ostra.
  - The credential stores under your home folder: `~/.ssh`, `~/.aws`, `~/.config/gh/hosts.yml`, browser
    profiles, and the other entries of `HOME_CREDENTIALS` in [`paths.rs`](../../crates/ostra-core/src/paths.rs).
  - Container engine sockets.
  - The desktop session bus.
- Some files run code in your later shells or builds: `.bashrc`, `.zshrc`, `.gitconfig`, `~/.cargo/bin`,
  `Library/LaunchAgents`, and similar files. These files stay read-only, also in a writable folder. Thus an agent
  cannot leave a program for you.
- The sandbox removes `SSH_AUTH_SOCK`, `DBUS_SESSION_BUS_ADDRESS`, `DISPLAY`, `DOCKER_HOST`, and similar variables
  from the environment.

Ostra renders that profile for two backends, and the two backends do not behave the same.

### Linux: bubblewrap

On Linux, Ostra finds `bwrap` on `PATH`. It probes `bwrap` one time per process: it starts `true` in a new
sandbox. The profile becomes bind mounts in a new mount namespace and a new pid namespace:

- A hidden path looks empty, because Ostra mounts an empty dir or file on it.
- `/tmp` is a private scratch dir.
- The agent sees only its own processes. A test checks that `ls /proc` in the sandbox lists fewer than ten pids.
- A harness CLI gets an overlaid home folder. Thus it can write its own state dirs. Its settings, hooks, and
  instruction files stay read-only
  ([`exec-harness/src/sandbox.rs`](../../crates/ostra-exec-harness/src/sandbox.rs)).
- When the sandboxed program exits, the pid namespace stops each process that the program started.

The probe fails in two frequent cases. The setup screen shows the reason:

1. **bubblewrap is not installed.** Install it with `sudo apt install bubblewrap` or `sudo dnf install
   bubblewrap`.
2. **The system blocks unprivileged user namespaces.** Recent Ubuntu releases restrict them through AppArmor. To
   allow them, set `kernel.apparmor_restrict_unprivileged_userns=0`. You can also give `bwrap` an AppArmor
   profile that permits them. In a container, the default seccomp or AppArmor profile often blocks them. Then run
   the container with a profile that allows them.

### macOS: Seatbelt

On macOS, Ostra renders the same profile as an SBPL policy for `/usr/bin/sandbox-exec`. Ostra calls that binary
by its absolute path. Ostra refuses the binary if root does not own it or if another user can write to it. The
reason is that the user can write to a Homebrew prefix on `PATH`. Seatbelt has no mount namespace and no pid
namespace. This causes these differences:

- A hidden path returns `EPERM`. It does not look empty.
- Seatbelt denies `/tmp`. It does not make it private. `TMPDIR` points to the scratch dir of the execution.
- A harness gets no disposable home. The home folder stays read-only, but the state dirs of the CLI are writable.
  Thus a CLI cannot write at the top of the home folder. For example, writes to the `~/.claude.json` of Claude
  Code fail. The CLIs continue to run when this occurs.
- A child that calls `setsid` or forks two times does not stop with its parent. Ostra uses a marker for this case:
  - The policy of each sandbox allows the lookup of two made-up mach service names. One name is for the
    invocation, and one is for the data dir.
  - For each process, the kernel tells if its inherited policy allows a name (`sandbox_check`).
  - Thus Ostra finds each descendant, independent of how it detached, and kills it.
  - At server start, Ostra uses the same method to kill the processes that an earlier server left
    (`kill_leftovers`).
- Ostra knows the terminal device of a PTY only after the PTY opens. Thus the launcher gives it to the policy as
  `-D TTY=<path>`.
- macOS cannot start a sandbox in another sandbox. If you start Ostra from a sandboxed terminal or from another
  agent, the probe reports `sandbox_apply: Operation not permitted`. The probe then tells you to start Ostra
  outside the sandbox.

- macOS has no network namespaces. Thus each sandbox shares the loopback of the host. The egress proxy and the
  hook bridge of each execution get their own ports on `127.0.0.1`. By default, a command can also connect to
  each other service on the Mac, but not to blocked ports or to the ports of Ostra. A workspace can block ports,
  or allow only listed ports. In that case, Ostra refuses a test that connects to a server on a random port.
- Each unsandboxed program of your user can read the arguments and the startup environment of another process
  through `sysctl(KERN_PROCARGS2)`. The Seatbelt policy refuses this to sandboxed commands. For commands that run
  in mode `off`, Ostra also clears its own startup environment at the start of `main` (`scrub_startup_env`).
- Decoy credential files work only on an admin account, because macOS shows the reports of the sandbox in the
  system log only to admins. They also work only for decoy paths where a file exists.

[Sandboxing](../security/sandboxing.md#how-a-command-gets-out-on-macos) has the details and the measurements.

### When no sandbox is available

The `[sandbox] mode` setting in `config.toml`, or the sandbox setting of the workspace, sets what occurs:

| Mode | With a working backend | Without one |
| --- | --- | --- |
| `required` (default on Linux and macOS) | Sandboxed | Ostra refuses each execution, with the reason |
| `auto` (default on Windows) | Sandboxed | Runs unsandboxed, with a warning at startup and in each execution |
| `off` | Unsandboxed | Unsandboxed |

Unsandboxed agent commands run with all the rights of your user. Choose `off` only when you intend it, for
example in a disposable VM. In each mode, the policy layer still checks each tool call.
[Sandboxing](../security/sandboxing.md) explains the profile that each backend renders.

## Processes and signals

Ostra can stop each program that it starts, and each process that the program started:

- **Native Bash commands** run in their own process group. A timeout kills the group. Under Seatbelt, the sandbox
  marker also finds children that left the group.
- **Harness CLIs** run in a PTY, where the child is the leader of its own session. To stop one, Ostra sends
  `SIGTERM` to the group, waits for a grace period, and then sends `SIGKILL`.
- **MCP servers over stdio** get their own process group on Unix. The reason is that servers that start through
  `npx` or `uvx` run as grandchildren. When Ostra closes the transport, it sends `SIGTERM` to the group
  ([`stdio.rs`](../../crates/ostra-mcp/src/stdio.rs)).

On Windows, each of these programs runs in a Job Object
([`proctree.rs`](../../crates/ostra-core/src/proctree.rs)). Ostra creates the job with
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and no breakaway. Thus the whole tree stops when Ostra closes the handle of
the job, or when Ostra exits:

- **Bash, PowerShell, Cmd, and stdio MCP servers** start suspended. They join their job, and only then do they
  run. Thus their first child is already in the job. A timeout or a stop ends the job. Ostra resumes the
  suspended thread through `NtResumeProcess`, because std does not return its handle. A thread snapshot that Ostra
  takes directly after creation can miss the new thread. The other method puts the child in its job at creation
  (`PROC_THREAD_ATTRIBUTE_JOB_LIST`). But this method is unstable in std on Rust 1.98.
- **Harness CLIs** start through ConPTY, which chooses its own creation flags. Thus a CLI joins its job directly
  after it starts. A program that the CLI starts in that first moment is not in the job. To stop a CLI, Ostra
  closes the pseudoconsole. This sends `CTRL_CLOSE_EVENT` to the attached programs. Ostra then waits for the grace
  period and ends the job.
- The tools wait at most five seconds until the system reaps a killed command. Thus a process that does not end
  cannot block a tool call.

Antivirus behavior monitors watch this type of process control. With the suspended start above, the test suite
runs without a detection on a machine with Kaspersky. A probe tested the planned behavior of the Windows sandbox.
It created a local user, gave the user a restricted token, and started the command as that user. Kaspersky
killed this probe as `PDM:Trojan.Win32.Generic` only because of its behavior. Windows Defender did not react.
Release builds do not have a code signature yet, and this also counts against an unknown binary. Thus the
Windows sandbox backend will need a signature to run reliably. Some products will also need an antivirus
trusted-zone entry ([Sandboxing](../security/sandboxing.md#windows)).

Harness CLIs inherit the environment of Ostra, but without some variables. These variables connect a CLI to a
session that Ostra itself runs in: `CLAUDECODE`, `CODEX_THREAD_ID`, and the other entries of
`PARENT_SESSION_ENV` in `pty.rs`. Without this step, if you start Ostra from Claude Code, each Claude Code
execution becomes a nested child that Ostra cannot resume.

## Shells

Ostra needs `bash` and `sh` on the machine:

- The native Bash tool always runs `bash -c` with a wrapper script, independent of your login shell. The wrapper
  records the working directory on exit. Thus a `cd` stays in effect for the next call, as in Claude Code.
- Harness installers and setup terminals run through `sh -c`.
- An agent can nest one command in another. For this case, the bash parser in `ostra-policy` knows `sh`, `bash`,
  `zsh`, `dash`, `ksh`, `fish`, `ash`, and `mksh` as shells. Thus the policy sees the inner command.

macOS ships an old bash 3.2 at `/bin/bash`. Ostra uses the first `bash` on `PATH`. Thus Ostra uses a Homebrew
bash when it comes first.

On Windows, the Bash tool runs Git Bash by its absolute path. The `PowerShell` and `Cmd` tools run
`powershell.exe` and `cmd.exe` from the system dir. Harness installers run in PowerShell on Windows.

## Files, permissions, and credentials

| | Linux | macOS | Windows | Override |
| --- | --- | --- | --- | --- |
| Data dir | `~/.local/share/ostra` | `~/Library/Application Support/ostra` | `%LOCALAPPDATA%\ostra` | `OSTRA_DATA_DIR` |
| Config file | `~/.config/ostra/config.toml` | `~/Library/Application Support/ostra/config.toml` | `%APPDATA%\ostra\config.toml` | `OSTRA_CONFIG` |
| Sandbox tool caches, one set for each session and one for each project or workspace program | `~/.cache/ostra/sandbox/` | `~/.cache/ostra/sandbox/` | Not used | `OSTRA_SANDBOX_CACHE` |

Both dirs come from the `dirs` crate. Thus they follow `XDG_DATA_HOME` and `XDG_CONFIG_HOME` on Linux. On
Windows, they are in two different trees, `Local` and `Roaming`. The home folder on Windows is the profile
folder, because Windows does not set `HOME`.

Ostra makes file modes stricter on Unix, because SQLite creates its `-wal` and `-shm` files world-readable:

- The data dir is `0700`.
- The registry database and its side files are `0600`. The registry holds provider keys, git credentials, and the
  push signing key.
- Ostra creates the master key file, the SSH key file for git of each execution, and other private files with
  mode `0600`.

The master key seals each credential in the registry. Ostra stores the key in the OS keychain when a keychain
answers in ten seconds. If not, Ostra stores it in an owner-only file in the data dir
([`master_key.rs`](../../crates/ostra-server/src/master_key.rs)):

- **macOS** uses the login Keychain.
- **Linux** uses the Secret Service over D-Bus (GNOME Keyring or KWallet). A headless server, a container, or an
  SSH session without a session bus has no Secret Service. In these cases, Ostra uses the file.
- **Windows** uses the Credential Manager. The key is a generic credential with the name of the data dir. When
  the server restarts, it reads the key back from the Credential Manager.
- `OSTRA_MASTER_KEY_FILE` names a different key file to use, for example a Docker secret. Ostra does not change a
  read-only mount, because its mode is the decision of the operator.

The sandbox hides the data dir and the key file from each agent, independent of the backend that stores the key.

## Running as a service

`install.sh` installs the binary into `~/.local/bin`. It also registers a per-user service that starts at login
and restarts when the server fails:

- **Linux**: a systemd user unit. `journalctl --user -u ostra` shows its log. The unit runs only when you are
  logged in. To change this, enable lingering with `loginctl enable-linger`.
- **macOS**: a launchd agent with the label `dev.ostra.server`. Its errors go to `service.err.log` in the data
  dir.
- Other systems: the script stops with "Unsupported system" and tells you to use `run.sh`.
- **Windows**: `install.ps1` registers a Task Scheduler task at logon, `dev.ostra.server`. The task needs no
  administrator rights. It runs `ostra.exe` under a windowless console and repeats every 5 minutes. Thus it starts
  the server again after the server exits ([Install](../start/install.md#windows)).

Service managers start programs with a minimal `PATH`. Thus the launcher records the `PATH` of the shell that ran
`install.sh`. With this `PATH`, the service finds harness CLIs in `~/.local/bin` or in a Homebrew prefix. A
Windows task starts with the saved environment of the user. Thus `install.ps1` records nothing. It refuses
`OSTRA_ENV_FILE`, and it refuses `OSTRA_CONFIG` or `OSTRA_DATA_DIR` when only the shell sets them, because the
task cannot see them.

## Docker

The [Dockerfile](../../Dockerfile) compiles on Ubuntu 26.04. It runs Ostra as UID 1000, with bubblewrap, git,
and an SSH client installed. [docker-compose.yml](../../docker-compose.yml) does these things:

- It publishes the port only on loopback.
- It keeps the home folder in a volume. Thus harness logins stay after a restart.
- It mounts the projects dir at the same absolute path on the two sides, because Ostra stores workspace and
  project paths literally.

Check the sandbox status on the setup screen after the first start. The default seccomp and AppArmor profiles of
Docker can block the user namespaces that bubblewrap needs. Then, in the default `required` mode, Ostra refuses
each execution. To fix this, do one of these two things:

- Run the container with a security profile that allows user namespaces.
- Use the container as the boundary, and set `[sandbox] mode = "off"` intentionally.

On macOS and Windows, Docker Desktop runs the image in a Linux VM. Thus this section also applies there.

## WSL 2

WSL 2 is a Linux VM. Thus follow the Linux instructions in the distribution:

- Install bubblewrap. WSL 2 kernels allow unprivileged user namespaces. Thus the probe usually passes. Run the
  setup check to make sure.
- WSL 1 has no user namespaces. Thus the sandbox cannot start there.
- Keep projects on the Linux file system (`~/src`, not `/mnt/c/...`). Files on the Windows drive go through a
  translation layer. This layer is slow for git and the code index, and it does not keep Unix file modes.
- Your WSL setup controls whether WSL can open the browser. Start with `--no-open`, and open the printed URL in a
  Windows browser. `localhost` forwarding reaches the server.
- The systemd user unit from `install.sh` needs systemd enabled in `/etc/wsl.conf`.

## Harness CLIs

A harness execution starts the CLI of the vendor without a change in a PTY. The CLIs are Claude Code, Codex,
Grok Build, and Antigravity. Ostra finds each CLI on `PATH` by its command name, or by the command that
`config.toml` sets for it. Ostra then reads its version and its login state
([`setup.rs`](../../crates/ostra-exec-harness/src/setup.rs)).

The setup screen can install a missing CLI. It runs the official installer of the vendor (`curl -fsSL <vendor
URL>/install.sh | sh` or `| bash`) in a terminal that you watch. It can run the login flow of the CLI in the same
way. Those installers need `curl` and a Unix shell. On Windows, the terminal runs PowerShell:

- For Claude Code: `irm https://claude.ai/install.ps1 | iex`.
- For Codex: `npm install -g @openai/codex`, which needs npm.
- For Grok Build and Antigravity: Ostra has no Windows installer command. Their terminal tells you to follow the
  Windows instructions of the vendor.

The vendor of each CLI decides which systems the CLI supports. If a CLI does not install, check the page of the
vendor.

On Windows, the hook commands of a CLI name `ostra.exe` with forward slashes. When the path has a space, the
commands put it in double quotes. Git Bash (the hook shell of Claude Code) and cmd read this form in the same way.
A Codex shell call reaches the policy as `PowerShell` if it does not start `bash` or `sh`. The reason is that
Codex runs its commands in PowerShell on Windows.

In the sandbox, a CLI can read and write its own state dir (`~/.claude`, `~/.codex`, `~/.grok`, `~/.gemini`) and
its own credential file. It cannot write other files under your home. Its settings, hooks, and instruction files
stay read-only, because a hook there will run in your own later sessions of that CLI.

## Browsers

The console is a single-page React app that the server embeds. For live updates and the terminal (xterm.js), it
needs a current browser with WebSocket support. The file views use the Monaco editor.

- **Tested**: Chromium, through the Playwright browser security suite in [`tests/browser`](../../tests/browser).
- **Expected to work**: current Firefox and Safari, because the console uses standard web APIs. Nobody runs the
  suite on them.
- **Push notifications** need a secure context: `localhost`, or HTTPS through a reverse proxy. Over plain HTTP on
  a LAN address, the browser does not offer Web Push. The other parts of the console still work. On iOS, Safari
  sends Web Push only to a site that you added to the home screen.
- **Clipboard managers** can use up the one-time sign-in link when they show a preview of it. Run `ostra url` to
  get a new link.

The server opens the sign-in URL with the default opener of the system: `open` on macOS, `xdg-open` on Linux,
and the default browser on Windows. On a headless machine, give `--no-open` and copy the printed URL.
