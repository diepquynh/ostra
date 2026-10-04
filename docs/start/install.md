# Install

Ostra is one binary, `ostra`. It holds the server, the engine, all the tools, and the browser console. You
compile it from source. This page tells what the build needs and where Ostra keeps its files. It also gives the
ways to run Ostra and all the commands of the binary.

## What you need

| Tool | Why Ostra needs it |
| --- | --- |
| [rustup](https://rustup.rs) | `rust-toolchain.toml` pins Rust 1.99.0 with `rustfmt` and `clippy`. rustup reads that file and installs the version on the first `cargo` call. Thus, you do not select a version yourself. |
| Node.js 24 with npm | Builds the console in `web/`. The binary embeds the built files at compile time. Thus, you must build the console before the binary. |
| A C compiler | Some dependencies compile C code, for example SQLite. Use `build-essential` on Debian and Ubuntu, `gcc` and `make` on Fedora, or `xcode-select --install` on macOS. |
| `git` | Ostra clones, branches, stages, and commits in your projects. |
| bubblewrap (`bwrap`), Linux only | Each agent command runs in a sandbox. On Linux, the sandbox is bubblewrap. macOS uses the built-in Seatbelt (`sandbox-exec`), so it needs no other tool. |

Ostra supports Linux and macOS with the full sandbox. It also compiles and runs on Windows, without a sandbox.
See [Windows](#windows). `install.sh` refuses all systems other than Linux and macOS. On Windows, `install.ps1`
does the same work. For the sandbox on a Windows machine, run Ostra in WSL 2 or in Docker.
[OS compatibility](../platforms/os-compatibility.md) gives the differences between the systems.

### The sandbox check

On Linux and macOS, the default sandbox mode is `required`. Ostra refuses an execution that it cannot put in a
sandbox, and does not run it unsandboxed. On Linux, two problems can prevent a sandbox:

- **bubblewrap is missing.** Install it with `sudo apt install bubblewrap` or `sudo dnf install bubblewrap`.
- **User namespaces are blocked.** bubblewrap needs unprivileged user namespaces. Ubuntu 24.04 and later
  versions restrict them through AppArmor. The default seccomp profile of Docker blocks them in containers.
  Ostra tells which of the two problems it found. On the host, allow them (for example
  `sudo sysctl kernel.apparmor_restrict_unprivileged_userns=0`). In a container, run with a profile that
  allows them.

To run without a sandbox intentionally, set the mode to `off`. Set it for all workspaces in `config.toml`, or
for one workspace in its settings:

```toml
[sandbox]
mode = "off"            # "required" (the default), "auto", or "off"
network = "allowlist"   # "none", "allowlist" (the default), "public", or "host"
```

`auto` uses the sandbox where it can. Where it cannot, it runs unsandboxed and shows a warning. Use `off` only
on a machine that the agent can use without limits. `off` runs agent commands with the full rights of your
user.

## Build

To compile in one step, run this script:

```bash
./build.sh
```

The script does these steps:

1. It runs `npm ci` when `node_modules` is missing.
2. It builds the console again only when a file in `web/` or `design/` is newer than the last build.
3. It runs `cargo build --release -p ostra-server`.

The first build takes some minutes. When only Rust code changed, a later build takes seconds.

To do the same steps by hand, run these commands:

```bash
npm ci                        # one npm workspace at the root: design/, web/, and site/
(cd web && npm run build)     # writes web/dist, which the binary embeds
cargo build --release -p ostra-server
```

The result is `target/release/ostra`. Build the console first each time that `web/` or `design/` changes. A
binary built with an old `web/dist` serves the old console.

### Versions and releases

Ostra uses Semantic Versioning. `ostra --version` prints the version of the build. Each commit title gives
the kind of change in the Conventional Commits form. The kind sets the bump:

| Title | Example | From 1.0.0 | Below 1.0.0 |
|---|---|---|---|
| `!` after the type, plus a `BREAKING CHANGE:` line in the body | `feat!: Rename the routes table` | major | minor |
| `feat:` | `feat: Add planning evals` | minor | patch |
| any other type (`fix:`, `docs:`, `refactor:`, ...) | `fix: Keep Esc from closing the app` | patch | patch |

Below 1.0.0, each place moves one position to the right, in the way that Cargo reads `0.y.z`. Thus, a breaking change is
never in a patch release. A commit does not set a version itself. `./release.sh` does these steps:

1. It reads the titles after the last `vX.Y.Z` tag with [git-cliff](https://git-cliff.org) (configured in
   `cliff.toml`).
2. It selects the largest bump of these titles.
3. It writes that version into `Cargo.toml`, `Cargo.lock`, and the npm manifests.
4. It writes `CHANGELOG.md` again, grouped by type.
5. It commits `chore: Release vX.Y.Z` and tags the commit.

The script refuses a dirty working tree, because the release commit must hold only the bump. It does not
push. To select the version yourself, give it to the script: `./release.sh 1.0.0`.

## Where Ostra keeps its files

Ostra keeps its files in three places:

- One config file for the machine.
- One data folder for the server.
- A `.ostra/` folder in each workspace and each project.

| What | Linux | macOS | Override |
| --- | --- | --- | --- |
| Global config | `~/.config/ostra/config.toml` | `~/Library/Application Support/ostra/config.toml` | `OSTRA_CONFIG` |
| Data folder | `~/.local/share/ostra/` | `~/Library/Application Support/ostra/` | `OSTRA_DATA_DIR` |

Run `ostra config` to print the config path on this machine. If no config exists, the command first writes the
default config there.

The **global config** holds these items:

- The providers of the machine: the variables that hold the keys.
- The tier-to-model tables for each executor.
- Harness CLI commands.
- Global permission rules.
- The `[server]` defaults: port, bind address, and allowed hosts.
- The `[sandbox]` profile.

It holds no secrets. A key that you save in the browser goes to the registry, not to this file.

The **data folder** is owner-only (mode 0700), because it holds credentials:

| File | Holds |
| --- | --- |
| `registry.db` | The workspace list, saved provider keys and git credentials (sealed), sign-in records, and the Web Push signing key. Owner-only (0600), along with its `-wal` and `-shm` files. |
| `master.key` | The key that seals the credentials of the registry, when Ostra cannot get to the OS keychain. If it can, the key is in the keychain under the service name `ostra`. `OSTRA_MASTER_KEY_FILE` names a different file, for example a Docker secret. |
| `server.json` | The port, URL host, and process id of the running server. Written at start, removed at a clean stop. `ostra url` and `ostra stop` read it. |
| `server.log` | All the log lines of the server, appended across runs. Set `OSTRA_LOG=debug` for more detail. |
| `models-dev.json` | The model price catalog from models.dev, refreshed one time each day. Thus, after a restart without network, Ostra can still calculate the price of each execution. |

No agent can read the data folder. The policy refuses each tool call that names it, for all permissions.
[Storage](../architecture/storage.md) describes each database. [Secrets and data](../security/secrets-and-data.md)
tells how Ostra seals credentials.

Each **workspace folder** gets these files:

- `.ostra/workspace.toml`: its settings.
- `.ostra/workspace.db`: the event log and the tables that Ostra makes from it.
- `.ostra/sessions/<session id>/`: all the files that each session writes.

Each **project folder** gets `.ostra/INVENTORY.md`, `.ostra/project.toml`, and
`.ostra/memory/knowledge.sqlite3` from the init flow. It also gets its skills in `.agents/skills/`. Commit the
project files with the project.

## Run it

### In a terminal

```bash
./target/release/ostra        # the same as `ostra serve`
./run.sh                      # builds first, applies $OSTRA_ENV_FILE, then starts it
SKIP_BUILD=1 ./run.sh         # starts the existing binary
```

The server binds `127.0.0.1` on port 7878. If that port is in use, it uses the next free port of the
next 19 ports. To require one port, pass `--port`. Then the server fails if the port is in use. The server
prints a sign-in URL and opens it in your default browser.

| Flag | Effect |
| --- | --- |
| `--port 8080` | Listen on this port only. The default comes from `server.port` in the config. If that key is not set, the default is 7878. |
| `--no-open` | Print the URL without opening a browser. |
| `--bind 0.0.0.0` | Listen on all interfaces (or `::`, or the address of one interface). Read [Server and browser](../security/server-and-browser.md) first, because the server uses plain HTTP. |
| `--allow-host ostra.example.com` | Accept this host name in the `Host` header, for a reverse proxy. You can use this flag more than one time. |
| `--dev` | Also accept the host of the Vite dev server (`localhost:5173`), for work on the console. |

### Credentials from a file

When `OSTRA_ENV_FILE` is set, `run.sh` and the service read it. They apply only the lines in the form
`export NAME=value` and ignore all other lines. Thus, they do not run shell code in the file:

```bash
cat > ~/ostra.env <<'EOF'
export ANTHROPIC_API_KEY=sk-ant-...
export OPENAI_API_KEY=sk-...
EOF
chmod 600 ~/ostra.env
OSTRA_ENV_FILE=~/ostra.env ./run.sh
```

Ostra removes all provider variables from the environment that it gives to agent processes, harness CLIs,
and MCP servers. Thus, a key in the environment of the server never gets to the shell of an agent.

### As a login service

```bash
./install.sh                  # build, install to ~/.local/bin, register and start the service
./install.sh --port 8080      # extra flags pass through to `ostra serve`
./install.sh status
./install.sh url              # a fresh sign-in URL
./install.sh restart          # after changing a key in OSTRA_ENV_FILE
./install.sh uninstall        # removes the service and binary, keeps config and data
```

On Linux, the service is a systemd user unit (`~/.config/systemd/user/ostra.service`). On macOS, it is a
launchd agent (`~/Library/LaunchAgents/dev.ostra.server.plist`). Both start the server again when it fails. The
installer writes a small launcher, `~/.local/bin/ostra-service`. The launcher reads `OSTRA_ENV_FILE` again at
each start. Thus, the secrets are never in the unit file or the plist.

The launcher keeps the `PATH` of the shell that ran `install.sh`. Service managers start with a minimal
`PATH`, and without the saved `PATH` they cannot find harness CLIs. If you install a new harness CLI in a new
location, install the service again. On Linux, the unit runs when you are logged in. To keep it on after you
log out, run `loginctl enable-linger $USER`. Logs go to `server.log`. On Linux, they also go to
`journalctl --user -u ostra`. On macOS, they also go to `service.err.log` in the data folder.

`OSTRA_PREFIX` changes the install location from `~/.local`. When `OSTRA_CONFIG` and `OSTRA_DATA_DIR` are
set, the installer records them in the service.

### In Docker

`docker-compose.yml` builds an image and publishes Ostra on `127.0.0.1:7878` only. Do these steps:

1. Create `./config` and give it to UID 1000, because Ostra runs as 1000:1000 in the container:
   `mkdir -p config && sudo chown 1000:1000 config`.
2. On the two sides of the projects mount, replace `/srv/ostra/projects` with one absolute host folder. This
   folder holds your projects and workspaces. The two sides must be equal. Ostra stores paths literally, and a
   recorded path must point to the same folder in and out of the container.
3. Run `docker compose up -d`, then `docker compose exec ostra ostra url`. In the printed link, replace the
   address with `localhost:7878`, because the container prints its own network address.

The data folder is in the `ostra-data` volume. The home folder of the container is in `ostra-home`. Thus,
harness CLIs that you install from the setup check, and their logins, stay after a rebuild. The sandbox in a
container needs user namespaces, and the default seccomp profile of Docker blocks them (see
[The sandbox check](#the-sandbox-check)).

## Windows

Ostra compiles and runs natively on Windows 10 and 11, without a sandbox. Agent commands run with the full
rights of your user. The policy layer still checks each tool call. Before you use Ostra on an important
repository, read [what that leaves open](../security/sandboxing.md#windows). For the sandbox, run Ostra in
WSL 2 or in Docker.

You need:

| Tool | Why |
| --- | --- |
| [rustup](https://rustup.rs) | Installs the pinned Rust 1.99.0 for the `x86_64-pc-windows-msvc` target on the first `cargo` call. |
| Visual Studio Build Tools with the C++ workload and a Windows SDK | The MSVC linker, and the C compiler for SQLite and the TLS library. |
| Node.js 24 with npm | Builds the console in `web/`. |
| [Git for Windows](https://git-scm.com/download/win) | Git, and Git Bash, which the Bash tool runs. Ostra refuses the WSL `bash.exe`. |

Compile in PowerShell from the repository root. Run the steps of `build.sh` by hand, because it is a bash
script for Linux and macOS:

```powershell
npm ci
npm run build --workspace web     # writes web\dist, which the binary embeds
cargo build --release -p ostra-server
.\target\release\ostra.exe
```

To run Ostra as a login service, use `install.ps1`. It is the Windows version of `install.sh`, and it needs no
administrator rights:

```powershell
.\install.ps1                  # build, install to ~\.local\bin, register and start the task
.\install.ps1 --port 8080      # extra flags pass through to `ostra serve`
.\install.ps1 status
.\install.ps1 url              # a fresh sign-in URL
.\install.ps1 restart
.\install.ps1 uninstall        # removes the task and binary, keeps config and data
```

The script registers a Task Scheduler task, `dev.ostra.server`. The task starts at logon under your own
account. It runs `ostra.exe serve --no-open` under `conhost.exe --headless`. This gives the server a console
with no window, so no window opens at logon. The task also runs again each 5 minutes. When a server runs, the
task skips the run. Thus, the server starts again in 5 minutes or less after it exits. To stop the server,
`install.ps1` ends the server process, because it cannot send Ctrl+C to a console with no window. At the next
start, recovery resumes the interrupted executions, the same as after a crash.

A scheduled task starts with your saved environment, not the environment of the shell that ran the installer.
This causes three differences from `install.sh`:

- Nothing records `PATH`. The task sees the user `PATH` that a new terminal sees. Some harness CLI installers
  add the CLI to `PATH`. The task finds such a CLI after `.\install.ps1 restart`.
- The installer refuses `OSTRA_ENV_FILE`, because nothing reads a credentials file before `ostra.exe` starts.
  Save provider keys in the setup screen, or as user environment variables.
- `OSTRA_CONFIG` and `OSTRA_DATA_DIR` work only as saved user variables (`setx`). The installer refuses a value
  that is only in the current shell, because the server cannot see it.

Before the build, the installer adds the saved user and machine `PATH` entries to its own `PATH`. Thus, a
terminal that you opened before you installed rustup or Node.js still finds `cargo` and `npm`. If a different
Ostra server uses the same data dir, the installer refuses to start. An example is an `ostra.exe` that you
started by hand. Two servers must not use one registry. If PowerShell refuses to run the script, allow local
scripts one time with `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`.

If you do not use the service, start `ostra.exe` in a terminal. The data dir is `%LOCALAPPDATA%\ostra`. The
config file is `%APPDATA%\ostra\config.toml`. On Windows, the default sandbox mode is `auto`. Thus,
executions run and show that they are unsandboxed. To stop the warning intentionally, set `mode = "off"`. To
refuse all executions, set `required`. The setup screen shows the sandbox status and if Ostra found Git Bash.

## Scratch setups

With `OSTRA_CONFIG` and `OSTRA_DATA_DIR`, you can run a second, temporary Ostra next to your usual one. It has
its own config, registry, and sign-ins:

```bash
mkdir -p /tmp/ostra-try
OSTRA_CONFIG=/tmp/ostra-try/config.toml OSTRA_DATA_DIR=/tmp/ostra-try/data \
  ./target/release/ostra --port 7900
```

Set `OSTRA_CONFIG` and `OSTRA_DATA_DIR` the same way for each `ostra` command for that server. `ostra url`
and `ostra stop` find the server through the data folder. Use cheap tiers and a small `session_budget_usd`
with a scratch setup when you try Ostra on a real repository. A session starts many executions, and each
execution costs money.

## Commands

| Command | Does |
| --- | --- |
| `ostra` or `ostra serve` | Start the server with the flags above. |
| `ostra url` | Print a new one-time sign-in URL for the running server. Fails when no server runs. |
| `ostra config` | Print the global config path. If there is no config, first write the default config. |
| `ostra sessions` | List the signed-in browsers: id, sign-in time, last seen, address, and browser. |
| `ostra sessions revoke <id>` | Sign one browser out. A running server refuses it in 5 seconds or less. `--all` signs out all browsers. |
| `ostra signout` | Sign out all browsers, the same as `ostra sessions revoke --all`. |
| `ostra stop <session id>` | End a session when the server does not run, so the next start does not resume it. |
| `ostra hook`, `ostra mcp-stdio` | Harness CLIs call these during an execution to connect to the server. Do not run these yourself. |

`ostra sessions` and `ostra stop` are about different things. A *sign-in* is a browser that holds a cookie. A
*session* is one task that runs through the pipeline. The commands are in
[`crates/ostra-server/src/main.rs`](../../crates/ostra-server/src/main.rs).
