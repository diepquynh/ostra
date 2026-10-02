# Install

Ostra ships as one binary, `ostra`, that holds the server, the engine, every tool, and the browser console.
You build it from source. This page covers what the build needs, where Ostra keeps its files, the ways to run
it, and every command the binary accepts.

## What you need

| Tool | Why Ostra needs it |
| --- | --- |
| [rustup](https://rustup.rs) | `rust-toolchain.toml` pins Rust 1.99.0 with `rustfmt` and `clippy`. rustup reads that file and installs the version on the first `cargo` call, so you never pick a version yourself. |
| Node.js 24 with npm | Builds the console in `web/`. The binary embeds the built files at compile time, so the console must be built before the binary. |
| A C compiler | A few dependencies compile C code, SQLite among them. Use `build-essential` on Debian and Ubuntu, `gcc` and `make` on Fedora, or `xcode-select --install` on macOS. |
| `git` | Ostra clones, branches, stages, and commits in your projects. |
| bubblewrap (`bwrap`), Linux only | Every agent command runs in a sandbox. On Linux that sandbox is bubblewrap. macOS uses the built-in Seatbelt (`sandbox-exec`), so it needs nothing extra. |

Ostra supports Linux and macOS with the full sandbox. It also builds and runs on Windows, without a sandbox; see
[Windows](#windows). `install.sh` refuses any system but Linux and macOS; on Windows, `install.ps1` does its job.
For the sandbox on a Windows machine, run
Ostra in WSL 2 or in Docker. [OS compatibility](../platforms/os-compatibility.md) covers what differs between
systems.

### The sandbox check

On Linux and macOS the sandbox mode defaults to `required`: an execution that cannot be sandboxed is refused,
not run unsandboxed. On Linux two things can prevent a sandbox:

- **bubblewrap is missing.** Install it with `sudo apt install bubblewrap` or `sudo dnf install bubblewrap`.
- **User namespaces are blocked.** bubblewrap needs unprivileged user namespaces. Ubuntu 24.04 and later
  restrict them through AppArmor, and Docker's default seccomp profile blocks them inside containers. Ostra
  says which one it hit. On the host, allow them (for example
  `sudo sysctl kernel.apparmor_restrict_unprivileged_userns=0`); in a container, run with a profile that
  allows them.

To run without a sandbox on purpose, set the mode to `off`, either for every workspace in `config.toml` or
for one workspace in its settings:

```toml
[sandbox]
mode = "off"            # "required" (the default), "auto", or "off"
network = "allowlist"   # "none", "allowlist" (the default), "public", or "host"
```

`auto` sandboxes where it can and runs unsandboxed with a warning where it cannot. `off` runs agent commands
with the full rights of your user, so use it only on a machine you would let the agent use freely.

## Build

The one-step build is:

```bash
./build.sh
```

It runs `npm ci` when `node_modules` is missing, rebuilds the console only when a file under `web/` or
`design/` is newer than the last build, and then runs `cargo build --release -p ostra-server`. The first build
takes several minutes; later ones take seconds when only Rust code changed.

By hand, the same steps are:

```bash
npm ci                        # one npm workspace at the root: design/, web/, and site/
(cd web && npm run build)     # writes web/dist, which the binary embeds
cargo build --release -p ostra-server
```

The result is `target/release/ostra`. Build the console first every time `web/` or `design/` changes,
because a binary built against an old `web/dist` serves the old console.

### Versions and releases

Ostra follows Semantic Versioning, and `ostra --version` prints the version it was built at. Each commit
title says what kind of change it is in the Conventional Commits form, and that kind decides the bump:

| Title | Example | From 1.0.0 | Below 1.0.0 |
|---|---|---|---|
| `!` after the type, plus a `BREAKING CHANGE:` line in the body | `feat!: Rename the routes table` | major | minor |
| `feat:` | `feat: Add planning evals` | minor | patch |
| any other type (`fix:`, `docs:`, `refactor:`, ...) | `fix: Keep Esc from closing the app` | patch | patch |

Below 1.0.0 every place shifts one to the right, the way Cargo reads `0.y.z`, so a breaking change never
lands in a patch release. A commit sets no version itself. `./release.sh` reads the titles since the last
`vX.Y.Z` tag with [git-cliff](https://git-cliff.org) (configured in `cliff.toml`), takes the largest bump among
them, writes that version into `Cargo.toml`, `Cargo.lock`, and the npm manifests, rewrites `CHANGELOG.md`
grouped by type, then commits `chore: Release vX.Y.Z` and tags it. It refuses a dirty working tree, because
the release commit must hold only the bump, and it does not push. Pass a version, `./release.sh 1.0.0`, to
choose it yourself.

## Where Ostra keeps its files

Ostra splits its files into three places: one config file for the machine, one data folder for the server,
and a `.ostra/` folder inside each workspace and each project.

| What | Linux | macOS | Override |
| --- | --- | --- | --- |
| Global config | `~/.config/ostra/config.toml` | `~/Library/Application Support/ostra/config.toml` | `OSTRA_CONFIG` |
| Data folder | `~/.local/share/ostra/` | `~/Library/Application Support/ostra/` | `OSTRA_DATA_DIR` |

Run `ostra config` to print the config path on this machine. It writes the default config there first when
none exists.

The **global config** holds the machine's providers (which variables carry which keys), the tier-to-model
tables for each executor, harness CLI commands, global permission rules, the `[server]` defaults (port, bind
address, allowed hosts), and the `[sandbox]` profile. It holds no secrets: a key you save in the browser goes
to the registry, not here.

The **data folder** is owner-only (mode 0700), because it holds credentials:

| File | Holds |
| --- | --- |
| `registry.db` | The workspace list, saved provider keys and git credentials (sealed), sign-in records, and the Web Push signing key. Owner-only (0600), along with its `-wal` and `-shm` files. |
| `master.key` | The key that seals the registry's credentials, when the OS keychain is not reachable. Otherwise the key lives in the keychain under the service name `ostra`. `OSTRA_MASTER_KEY_FILE` names another file, such as a Docker secret. |
| `server.json` | The running server's port, URL host, and process id. Written at start, removed at a clean stop. `ostra url` and `ostra stop` read it. |
| `server.log` | Everything the server logs, appended across runs. Set `OSTRA_LOG=debug` for more detail. |
| `models-dev.json` | The model price catalog from models.dev, refreshed once a day, so a restart without network still prices every execution. |

No agent can read the data folder: the policy refuses any tool call that names it, whatever the permissions
say. [Storage](../architecture/storage.md) describes each database, and
[Secrets and data](../security/secrets-and-data.md) how credentials are sealed.

Each **workspace folder** gets `.ostra/workspace.toml` (its settings), `.ostra/workspace.db` (the event log
and the tables built from it), and `.ostra/sessions/<session id>/` (every file each session writes). Each
**project folder** gets `.ostra/INVENTORY.md`, `.ostra/project.toml`, and `.ostra/memory/knowledge.sqlite3`
from the init flow, plus its skills in `.agents/skills/`. The project files are meant to be committed with the
project.

## Run it

### In a terminal

```bash
./target/release/ostra        # the same as `ostra serve`
./run.sh                      # builds first, applies $OSTRA_ENV_FILE, then starts it
SKIP_BUILD=1 ./run.sh         # starts the existing binary
```

The server binds `127.0.0.1`, on port 7878 or, when that is taken, the next free port of the following 19.
Pass `--port` to require one port and fail if it is taken. It prints a sign-in URL and opens it in your
default browser.

| Flag | Effect |
| --- | --- |
| `--port 8080` | Listen on this port only. The default comes from `server.port` in the config, else 7878. |
| `--no-open` | Print the URL without opening a browser. |
| `--bind 0.0.0.0` | Listen on every interface (or `::`, or one interface's address). Read [Server and browser](../security/server-and-browser.md) first, because it serves plain HTTP. |
| `--allow-host ostra.example.com` | Accept this host name in the `Host` header, for a reverse proxy. Repeatable. |
| `--dev` | Also accept the Vite dev server's host (`localhost:5173`), for work on the console. |

### Credentials from a file

`run.sh` and the service read `OSTRA_ENV_FILE` when it is set. They apply only the lines that look like
`export NAME=value` and ignore everything else, so a file that also holds shell code does not run it:

```bash
cat > ~/ostra.env <<'EOF'
export ANTHROPIC_API_KEY=sk-ant-...
export OPENAI_API_KEY=sk-...
EOF
chmod 600 ~/ostra.env
OSTRA_ENV_FILE=~/ostra.env ./run.sh
```

Ostra removes every provider variable from the environment it gives agent processes, harness CLIs, and MCP
servers, so a key in the server's environment never reaches an agent's shell.

### As a login service

```bash
./install.sh                  # build, install to ~/.local/bin, register and start the service
./install.sh --port 8080      # extra flags pass through to `ostra serve`
./install.sh status
./install.sh url              # a fresh sign-in URL
./install.sh restart          # after changing a key in OSTRA_ENV_FILE
./install.sh uninstall        # removes the service and binary, keeps config and data
```

On Linux this is a systemd user unit (`~/.config/systemd/user/ostra.service`); on macOS a launchd agent
(`~/Library/LaunchAgents/dev.ostra.server.plist`). Both restart the server when it fails. The installer
writes a small launcher, `~/.local/bin/ostra-service`, that re-reads `OSTRA_ENV_FILE` at every start, so the
secrets never sit in the unit file or the plist.

The launcher keeps the `PATH` of the shell that ran `install.sh`, because service managers start with a
minimal `PATH` and would not find harness CLIs otherwise. Reinstall after you install a new harness CLI
somewhere new. On Linux, the unit runs while you are logged in; `loginctl enable-linger $USER` keeps it
running after you log out. Logs go to `journalctl --user -u ostra` on Linux and to `service.err.log` in the
data folder on macOS, in addition to `server.log`.

`OSTRA_PREFIX` moves the install from `~/.local`. `OSTRA_CONFIG` and `OSTRA_DATA_DIR`, when set, are
recorded into the service.

### In Docker

`docker-compose.yml` builds an image and publishes Ostra on `127.0.0.1:7878` only.

1. Create `./config` and give it to UID 1000, because Ostra runs as 1000:1000 inside the container:
   `mkdir -p config && sudo chown 1000:1000 config`.
2. Replace `/srv/ostra/projects` on both sides of the projects mount with one absolute host folder that holds
   your projects and workspaces. Both sides must match, because Ostra stores paths literally and the paths it
   records must mean the same folder inside and outside the container.
3. `docker compose up -d`, then `docker compose exec ostra ostra url`. Replace the address in the printed link
   with `localhost:7878`, because the container prints its own network address.

The data folder lives in the `ostra-data` volume and the container's home folder in `ostra-home`, so harness
CLIs you install from the setup check, and their logins, survive a rebuild. The sandbox inside a container
needs user namespaces, which Docker's default seccomp profile blocks (see [The sandbox check](#the-sandbox-check)).

## Windows

Ostra builds and runs natively on Windows 10 and 11, without a sandbox: agent commands run with the full rights of
your user, and the policy layer still checks every tool call. Read [what that leaves open](../security/sandboxing.md#windows)
before you point it at a repository you care about. For the sandbox, run Ostra in WSL 2 or in Docker instead.

You need:

| Tool | Why |
| --- | --- |
| [rustup](https://rustup.rs) | Installs the pinned Rust 1.99.0 for the `x86_64-pc-windows-msvc` target on the first `cargo` call. |
| Visual Studio Build Tools with the C++ workload and a Windows SDK | The MSVC linker and the C compiler for SQLite and the TLS library. |
| Node.js 24 with npm | Builds the console in `web/`. |
| [Git for Windows](https://git-scm.com/download/win) | Git itself, and Git Bash, which the Bash tool runs. Ostra refuses the WSL `bash.exe`. |

Build in PowerShell from the repository root. `build.sh` is a bash script for Linux and macOS, so run its steps by
hand:

```powershell
npm ci
npm run build --workspace web     # writes web\dist, which the binary embeds
cargo build --release -p ostra-server
.\target\release\ostra.exe
```

To run Ostra as a login service instead, use `install.ps1`, the Windows counterpart of `install.sh`. It needs no
administrator rights:

```powershell
.\install.ps1                  # build, install to ~\.local\bin, register and start the task
.\install.ps1 --port 8080      # extra flags pass through to `ostra serve`
.\install.ps1 status
.\install.ps1 url              # a fresh sign-in URL
.\install.ps1 restart
.\install.ps1 uninstall        # removes the task and binary, keeps config and data
```

It registers a Task Scheduler task, `dev.ostra.server`, that starts at logon under your own account. The task runs
`ostra.exe serve --no-open` under `conhost.exe --headless`, which gives the server a console with no window, so
nothing opens at logon. The task also repeats every 5 minutes and skips the run while a server is running, which
starts the server again within 5 minutes after it exits. `install.ps1` stops the server by ending its process,
because a windowless console has no Ctrl+C to send; recovery resumes the interrupted executions at the next start,
as it does after a crash.

A scheduled task starts with your saved environment, not the environment of the shell that ran the installer. That
changes three things compared with `install.sh`:

- Nothing records `PATH`. The task sees the user `PATH` that a new terminal sees, so a harness CLI whose installer
  added itself to `PATH` is found after `.\install.ps1 restart`.
- `OSTRA_ENV_FILE` is refused, because nothing reads a credentials file before `ostra.exe` starts. Save provider keys
  in the setup screen, or as user environment variables.
- `OSTRA_CONFIG` and `OSTRA_DATA_DIR` work only as saved user variables (`setx`). The installer refuses a value that
  exists only in the current shell, because the server would not see it.

Before it builds, the installer adds the saved user and machine `PATH` entries to its own, so a terminal opened
before rustup or Node.js was installed still finds `cargo` and `npm`. The installer also refuses to start while
another Ostra server uses the same data dir, such as an `ostra.exe`
started by hand, because two servers must not share one registry. If PowerShell refuses to run the script, allow
local scripts once with `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`.

Otherwise, start `ostra.exe` in a terminal. The data dir is `%LOCALAPPDATA%\ostra` and the config file
`%APPDATA%\ostra\config.toml`. The sandbox mode
defaults to `auto` there, so executions run and show that they are unsandboxed; set `mode = "off"` to silence the
warning on purpose, or `required` to refuse every execution. The setup screen shows the sandbox status and whether
Git Bash was found.

## Scratch setups

`OSTRA_CONFIG` and `OSTRA_DATA_DIR` let you run a second, throwaway Ostra next to your real one, with its
own config, registry, and sign-ins:

```bash
mkdir -p /tmp/ostra-try
OSTRA_CONFIG=/tmp/ostra-try/config.toml OSTRA_DATA_DIR=/tmp/ostra-try/data \
  ./target/release/ostra --port 7900
```

Set `OSTRA_CONFIG` and `OSTRA_DATA_DIR` the same way for every `ostra` command aimed at that server, because
`ostra url` and `ostra stop` find the server through the data folder. Pair a scratch setup with cheap tiers
and a small `session_budget_usd` when you try Ostra on a real repository, because a session starts many
executions.

## Commands

| Command | Does |
| --- | --- |
| `ostra` or `ostra serve` | Start the server with the flags above. |
| `ostra url` | Print a fresh one-time sign-in URL for the running server. Fails when no server is running. |
| `ostra config` | Print the global config path, writing the default config if there is none. |
| `ostra sessions` | List the browsers signed in: id, sign-in time, last seen, address, and browser. |
| `ostra sessions revoke <id>` | Sign one browser out. A running server refuses it within 5 seconds. `--all` signs out every browser. |
| `ostra signout` | Sign out every browser, the same as `ostra sessions revoke --all`. |
| `ostra stop <session id>` | End a session while the server is not running, so the next start does not resume it. |
| `ostra hook`, `ostra mcp-stdio` | Called by harness CLIs during an execution to reach the server. You do not run these. |

`ostra sessions` and `ostra stop` talk about different things: a *sign-in* is a browser holding a cookie; a
*session* is one task running through the pipeline. The commands live in
[`crates/ostra-server/src/main.rs`](../../crates/ostra-server/src/main.rs).
