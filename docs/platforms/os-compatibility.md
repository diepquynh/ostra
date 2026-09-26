# OS compatibility

Run Ostra on Linux or macOS. Windows is not supported, and WSL 2 works as a Linux machine. The rest of this page
explains why, what differs between Linux and macOS, and which parts of that claim someone has actually run.

Ostra depends on the operating system in more places than a web server usually does, because it starts other
programs on your behalf: a Bash shell for the native agents, a harness CLI in a terminal, MCP servers, language
servers, and git. It has to confine those programs, stop them cleanly, and keep your credentials out of their
reach. Each of those jobs uses a Unix facility, and the sandbox uses one that exists only on Linux and macOS.

## Support matrix

| | Linux | macOS | WSL 2 | Windows |
| --- | --- | --- | --- | --- |
| Builds | Yes | Yes | Yes | No |
| Agent sandbox | bubblewrap | Seatbelt (`sandbox-exec`) | bubblewrap, when user namespaces are allowed | None |
| Harness terminals (PTY) | Yes | Yes | Yes | No |
| Master key in the OS keychain | Secret Service over D-Bus, else a file | Keychain | Usually the file | Not reached |
| Background service from `install.sh` | systemd user unit | launchd agent | systemd user unit, when WSL runs systemd | No |
| Docker image | Yes (the image is Linux) | Through Docker Desktop's Linux VM | Through Docker | Through Docker Desktop's Linux VM |
| How we know | Tested | Sandbox measured on macOS 26 | Inferred from the code | Inferred from the code |

"Tested" means the full test suite runs and Ostra is used daily on that system. "Inferred" means the answer comes
from reading the code, and nobody has run it. The next section says what that covers in detail.

## What has been run, and what has not

Ostra has no CI yet. Every result below comes from someone running the tests or the server by hand.

- **Linux** is where Ostra is developed. The workspace tests (`cargo test --workspace`), the sandbox tests, the
  end-to-end server test, and the browser security suite run on Ubuntu with kernel 6.8 and bubblewrap 0.9. The
  Docker image builds on Ubuntu 26.04. Other distributions are expected to work when they ship bubblewrap and
  allow unprivileged user namespaces, but nobody has tried each one.
- **macOS** has a Seatbelt sandbox that was measured probe by probe on macOS 26, and it has tests of its own
  (`#[cfg(target_os = "macos")]` in [`sandbox.rs`](../../crates/ostra-core/src/sandbox.rs) and
  [`env_scrub.rs`](../../crates/ostra-core/tests/env_scrub.rs)). Older macOS releases have not been checked.
- **WSL 2** runs a real Linux kernel, so the Linux build and bubblewrap apply unchanged. That is a reading of the
  code, not a report from a run.
- **Windows** is inferred too: the code does not compile for it (see below).
- **CPU architecture** is not special-cased anywhere. x86-64 is tested. Arm64 (Apple silicon, Graviton, a
  Raspberry Pi 5) has no architecture-specific code in its way, but no test run backs that.

## Why Windows is not supported

Ostra relies on Unix process control, and several modules call it without a platform gate, so a Windows build
fails at compile time:

- The native Bash tool starts `bash` in its own process group (`process_group(0)`) and kills the whole group
  with `kill(-pid, SIGKILL)` when a command times out
  ([`bash.rs`](../../crates/ostra-tools/src/bash.rs)). Without this, a command that starts a background server
  would outlive its tool call.
- Harness terminals stop a CLI with `SIGTERM`, wait for a grace period, then send `SIGKILL` to its process group
  ([`pty.rs`](../../crates/ostra-exec-harness/src/pty.rs)).
- Harness state is prepared with Unix file modes and symlinks
  ([`launch.rs`](../../crates/ostra-exec-harness/src/launch.rs),
  [`term_log.rs`](../../crates/ostra-exec-harness/src/term_log.rs)).
- The sandbox itself exists only for Linux and macOS. `sandbox::probe` answers "The sandbox needs Linux or
  macOS." everywhere else, and the default sandbox mode is `required`, so even a working Windows build would
  refuse every execution until someone turned the sandbox off.

Some modules do carry `#[cfg(unix)]` gates with a fallback, such as file modes in the workspace database
([`workspace.rs`](../../crates/ostra-store/src/workspace.rs)) and the file writer in
[`files/mod.rs`](../../crates/ostra-server/src/files/mod.rs). Those gates are not a Windows port, and a Windows
port is not planned. On a Windows machine, use WSL 2 or Docker.

## The sandbox on each system

The sandbox is the reason the operating system matters most. The policy layer judges every tool call by the paths
it names, but a shell can hide a path inside a variable or a script, so the kernel enforces a second boundary
underneath. One `Profile` in [`sandbox.rs`](../../crates/ostra-core/src/sandbox.rs) describes that boundary:

- The host file system is read-only.
- Only the execution's own roots are writable: the worktree or project it works on, its scratch dir, and its tool
  caches.
- Ostra's data dir, credential stores under your home folder (`~/.ssh`, `~/.aws`, `~/.config/gh/hosts.yml`,
  browser profiles, and the rest of `HOME_CREDENTIALS` in [`paths.rs`](../../crates/ostra-core/src/paths.rs)),
  container engine sockets, and the desktop session bus are hidden.
- Files that run code in your later shells or builds (`.bashrc`, `.zshrc`, `.gitconfig`, `~/.cargo/bin`,
  `Library/LaunchAgents`, and similar) stay read-only even inside a writable folder, so an agent cannot leave a
  program behind for you.
- `SSH_AUTH_SOCK`, `DBUS_SESSION_BUS_ADDRESS`, `DISPLAY`, `DOCKER_HOST`, and similar variables are removed from
  the environment.

That profile renders for two backends, and they do not behave the same.

### Linux: bubblewrap

On Linux, Ostra finds `bwrap` on `PATH` and probes it once per process by starting `true` in a fresh sandbox. The
profile becomes bind mounts in a new mount and pid namespace:

- A hidden path looks empty, because an empty dir or file is mounted over it.
- `/tmp` is a private scratch dir.
- The agent sees only its own processes. A test asserts that `ls /proc` inside the sandbox lists fewer than ten
  pids.
- A harness CLI gets an overlaid home folder, so it can write its own state dirs, while its settings, hooks, and
  instruction files stay read-only ([`exec-harness/src/sandbox.rs`](../../crates/ostra-exec-harness/src/sandbox.rs)).
- When the sandboxed program exits, the pid namespace takes everything it started down with it.

The probe fails in two common situations, and the setup screen shows the reason:

1. **bubblewrap is not installed.** Install it with `sudo apt install bubblewrap` or `sudo dnf install
   bubblewrap`.
2. **Unprivileged user namespaces are blocked.** Recent Ubuntu releases restrict them through AppArmor. Allow them
   with `kernel.apparmor_restrict_unprivileged_userns=0`, or give `bwrap` an AppArmor profile that permits them.
   Inside a container, the default seccomp or AppArmor profile often blocks them, so run the container with a
   profile that allows them.

### macOS: Seatbelt

On macOS, Ostra renders the same profile as an SBPL policy for `/usr/bin/sandbox-exec`. It calls that binary by
absolute path and refuses it unless root owns it and no one else can write to it, because a Homebrew prefix on
`PATH` is writable by the user. Seatbelt has no mount namespace and no pid namespace, which changes a few things:

- A hidden path returns `EPERM` instead of looking empty.
- `/tmp` is denied instead of private, and `TMPDIR` points at the execution's scratch dir.
- A harness gets no disposable home. The home folder stays read-only apart from the CLI's own state dirs, so a
  CLI's writes at the top of it, such as Claude Code's `~/.claude.json`, fail. The CLIs keep running when that
  happens.
- A child that calls `setsid` or double-forks is not killed with its parent. Ostra handles this with a marker: each
  sandbox's policy allows lookup of two made-up mach service names, one for the invocation and one for the data
  dir. The kernel answers for any process whether its inherited policy allows a name (`sandbox_check`), so Ostra
  finds every descendant however it detached, and kills them. At server start it kills whatever an earlier server
  left behind the same way (`kill_leftovers`).
- The terminal device of a PTY is only known once the PTY is open, so the launcher passes it to the policy as
  `-D TTY=<path>`.
- macOS cannot start a sandbox inside another sandbox. When you start Ostra from a sandboxed terminal or from
  another agent, the probe reports `sandbox_apply: Operation not permitted` and tells you to start it outside.

Seatbelt also has a gap that Ostra cannot close. On macOS, any program running as your user, a sandboxed one
included, can read the arguments and startup environment of your other programs through
`sysctl(KERN_PROCARGS2)`. Ostra protects its own copy: the first thing `main` does is move the environment off
the stack and zero the original strings (`scrub_startup_env`). It cannot protect your other programs, so the setup
check names this gap. Keep tokens in the keychain or in a file the sandbox hides, not in exports in your shell
profile.

### When no sandbox is available

The `[sandbox] mode` setting in `config.toml`, or the workspace's own sandbox setting, decides what happens:

| Mode | With a working backend | Without one |
| --- | --- | --- |
| `required` (default) | Sandboxed | Every execution is refused, with the reason |
| `auto` | Sandboxed | Runs unsandboxed and logs a warning once |
| `off` | Unsandboxed | Unsandboxed |

Unsandboxed agent commands run with the full rights of your user. Choose `off` only on purpose, for example in a
throwaway VM. The policy layer still checks every tool call either way.

## Processes and signals

Every program Ostra starts gets a way to stop it and everything it spawned:

- **Native Bash commands** run in their own process group. A timeout kills the group, and under Seatbelt the
  sandbox marker also finds children that left the group.
- **Harness CLIs** run in a PTY, where the child leads its own session. Stopping one sends `SIGTERM` to the group,
  waits for a grace period, then sends `SIGKILL`.
- **MCP servers over stdio** get their own process group on Unix, because servers started through `npx` or `uvx`
  run as grandchildren. Closing the transport sends `SIGTERM` to the group
  ([`stdio.rs`](../../crates/ostra-mcp/src/stdio.rs)).

Harness CLIs inherit Ostra's environment minus the variables that would tie them to a session Ostra itself runs
inside (`CLAUDECODE`, `CODEX_THREAD_ID`, and the rest of `PARENT_SESSION_ENV` in `pty.rs`). Without that, starting
Ostra from inside Claude Code would make every Claude Code execution a nested child that cannot be resumed.

## Shells

Ostra needs `bash` and `sh` on the machine:

- The native Bash tool always runs `bash -c` with a wrapper script, whatever your login shell is. The wrapper
  records the working directory on exit, so a `cd` carries over to the next call, as it does in Claude Code.
- Harness installers and setup terminals run through `sh -c`.
- The bash parser in `ostra-policy` understands `sh`, `bash`, `zsh`, `dash`, `ksh`, `fish`, `ash`, and `mksh` as
  shells when an agent nests one command inside another, so the policy sees the inner command.

macOS ships an old bash 3.2 at `/bin/bash`. Ostra uses the first `bash` on `PATH`, so a Homebrew bash is used when
it comes first.

## Files, permissions, and credentials

| | Linux | macOS | Override |
| --- | --- | --- | --- |
| Data dir | `~/.local/share/ostra` | `~/Library/Application Support/ostra` | `OSTRA_DATA_DIR` |
| Config file | `~/.config/ostra/config.toml` | `~/Library/Application Support/ostra/config.toml` | `OSTRA_CONFIG` |
| Sandbox tool caches, one set per workspace | `~/.cache/ostra/sandbox/` | `~/.cache/ostra/sandbox/` | `OSTRA_SANDBOX_CACHE` |

Both dirs come from the `dirs` crate, so they follow `XDG_DATA_HOME` and `XDG_CONFIG_HOME` on Linux.

Ostra tightens file modes on Unix, because SQLite creates its `-wal` and `-shm` files world-readable:

- The data dir is `0700`.
- The registry database and its side files are `0600`. The registry holds provider keys, git credentials, and the
  push signing key.
- The master key file, the per-execution SSH key file for git, and other private files are created `0600`.

The master key seals every credential in the registry. Ostra stores it in the OS keychain when one answers within
ten seconds, and otherwise in an owner-only file in the data dir
([`master_key.rs`](../../crates/ostra-server/src/master_key.rs)):

- **macOS** uses the login Keychain.
- **Linux** uses the Secret Service over D-Bus (GNOME Keyring or KWallet). A headless server, a container, or an
  SSH session without a session bus has no Secret Service, so Ostra falls back to the file.
- `OSTRA_MASTER_KEY_FILE` names a key file to use instead, for example a Docker secret. A read-only mount is left
  as it is, because its mode is the operator's choice.

The sandbox hides the data dir and the key file from every agent, whichever backend stores the key.

## Running as a service

`install.sh` installs the binary into `~/.local/bin` and registers a per-user service that starts at login and
restarts when the server fails:

- **Linux**: a systemd user unit (`journalctl --user -u ostra` shows its log). It runs while you are logged in
  unless you enable lingering with `loginctl enable-linger`.
- **macOS**: a launchd agent labeled `dev.ostra.server`, with its errors in `service.err.log` in the data dir.
- Anything else: the script stops with "Unsupported system" and points you at `run.sh`.

Service managers start programs with a minimal `PATH`, so the launcher records the `PATH` of the shell that ran
`install.sh`. That is how the service finds harness CLIs installed in `~/.local/bin` or a Homebrew prefix.

## Docker

The [Dockerfile](../../Dockerfile) builds on Ubuntu 26.04 and runs Ostra as UID 1000 with bubblewrap, git, and an
SSH client installed. [docker-compose.yml](../../docker-compose.yml) publishes the port on loopback only, keeps
the home folder in a volume so harness logins survive, and mounts the projects dir at the same absolute path on
both sides, because Ostra stores workspace and project paths literally.

Check the sandbox status on the setup screen after the first start. Docker's default seccomp and AppArmor profiles
can block the user namespaces bubblewrap needs, and with the default `required` mode every execution is then
refused. Either run the container with a security profile that allows user namespaces, or treat the container as
the boundary and set `[sandbox] mode = "off"` on purpose. On macOS and Windows, Docker Desktop runs the image in a
Linux VM, so this section applies there too.

## WSL 2

WSL 2 is a Linux VM, so follow the Linux instructions inside the distribution:

- Install bubblewrap. WSL 2 kernels allow unprivileged user namespaces, so the probe usually passes. Run the setup
  check to confirm.
- WSL 1 has no user namespaces, so the sandbox cannot start there.
- Keep projects on the Linux file system (`~/src`, not `/mnt/c/...`). Files on the Windows drive go through a
  translation layer that is slow for git and the code index, and it does not keep Unix file modes.
- Opening the browser from WSL depends on your WSL setup. Start with `--no-open` and open the printed URL in a
  Windows browser; `localhost` forwarding reaches the server.
- The systemd user unit from `install.sh` needs systemd enabled in `/etc/wsl.conf`.

## Harness CLIs

Harness executions start the vendor's own CLI (Claude Code, Codex, Grok Build, or Antigravity) unmodified in a PTY.
Ostra looks each one up on `PATH` by its command name, or by the command set for it in `config.toml`, and reads
its version and login state ([`setup.rs`](../../crates/ostra-exec-harness/src/setup.rs)).

The setup screen can install a missing CLI by running the vendor's official installer (`curl -fsSL <vendor
URL>/install.sh | sh` or `| bash`) in a terminal you watch, and it can run the CLI's own login flow the same way.
Those installers need `curl` and a Unix shell. Which systems each CLI supports is up to its vendor, so check the
vendor's page when a CLI will not install.

Inside the sandbox, a CLI can read and write its own state dir (`~/.claude`, `~/.codex`, `~/.grok`, `~/.gemini`)
and its own credential file, and nothing else under your home. Its settings, hooks, and instruction files stay
read-only, because a hook written there would run in your own later sessions of that CLI.

## Browsers

The console is a single-page React app that the server embeds. It needs a current browser with WebSocket support
for live updates and the terminal (xterm.js), and the file views use the Monaco editor.

- **Tested**: Chromium, through the Playwright browser security suite in [`tests/browser`](../../tests/browser).
- **Expected to work**: current Firefox and Safari, since the console uses standard web APIs. Nobody runs the suite
  against them.
- **Push notifications** need a secure context: `localhost`, or HTTPS through a reverse proxy. Over plain HTTP on
  a LAN address, the browser does not offer Web Push, and the rest of the console still works. On iOS, Safari
  delivers Web Push only to a site added to the home screen.
- **Clipboard managers** can spend the one-time sign-in link when they preview it. Run `ostra url` for a new one.

The server opens the sign-in URL with the system's default opener (`open` on macOS, `xdg-open` on Linux). On a
headless machine, pass `--no-open` and copy the printed URL.
