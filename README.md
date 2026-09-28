# Ostra

Ostra is a local development workspace that runs the Ultracode engineering pipeline: research, spec,
fact-check, plan, build, review, test, and docs. You start one binary and work in a browser tab. A Rust server
executes every tool call, code drives the pipeline from stage to stage and holds every gate, and models do the
work inside each stage. `HANDOVER.md` is the design brief this implementation follows.

Ostra does not compete with Claude Code, Codex, or any other agent harness. It runs them as executors, next to its
own native agent loop. It exists to help developers get used to working with AI agents in a structured software
development lifecycle, where each stage has a defined output and a gate you answer before the next stage starts.

> [!WARNING]
> **Ostra is not malware, but on Windows your antivirus may report it as malware.** Kaspersky's behavior monitor
> killed a Windows sandbox test probe as `PDM:Trojan.Win32.Generic`. Windows Defender did not react to the same
> behavior. The verdict comes from what the program does, not from a known signature, because
> running agents under control takes the same system calls that malware uses:
>
> - Every shell an agent runs starts suspended and joins a Job Object, so a stop or a timeout ends every process
>   it started.
> - Harness CLIs such as Claude Code and Codex run in a pseudoconsole (ConPTY) that the browser streams.
> - `install.ps1` registers a Task Scheduler task that starts the server at logon.
> - The planned Windows sandbox backend will create low-privilege local users, start each command as one of them
>   with a restricted token, and add a per-user network filter. Ostra does none of this yet, but a test probe of
>   that design (`tests/windows-probes/`) was killed by the same monitor.
>
> Build Ostra from source, so you know what runs. Release builds are not code-signed yet, and a signature is what
> these products need before they trust it. If your antivirus blocks Ostra, add only the `ostra.exe` you built to
> its trusted list instead of turning protection off, because agent commands on Windows run unsandboxed today,
> with your user's full rights.
>
> [Sandboxing](docs/security/sandboxing.md) explains how Ostra confines agents, and
> [its Windows section](docs/security/sandboxing.md#windows) covers what the monitor reacts to and what stays open
> until Windows has a sandbox.

## When should you use Ostra

Use Ostra when a change costs more in coordination than in typing:

- **Your system has many moving components.** One feature touches several services, libraries, or
  repositories, and you spend most of the time iterating across them. Ostra works over every project in a
  workspace, so research, plan, build, and review see all of them at once.
- **A feature needs a thorough read of the codebase first.** The research and spec stages map the code and
  check the idea against it before any code is written, and the fact-check stage verifies what the spec claims.
- **You want to try a change and correct it before it is finished.** Most changes take the light track:
  research, then reviewed phases with no spec or plan. When the build is done the session waits for your
  feedback, builds each round as a reviewed revision, and writes tests and docs only after you accept.
- **You are close to a deadline and still need to sleep.** A session keeps working through its stages while
  you are away, and it stops at a gate or a budget limit instead of spending without bound.
- **You do not want to watch a terminal.** Ostra runs in a browser tab, sends a notification when a gate
  needs your answer, and keeps the whole history of each session on its board.

For a one-line fix or a quick question, a single agent in a terminal is faster, because the pipeline's
stages and gates add time that a small change does not need.

## Build and run

### Quick start

With Rust (rustup), Node.js, and on Linux bubblewrap installed:

```bash
git clone https://github.com/diepquynh/ostra && cd ostra
export ANTHROPIC_API_KEY=sk-ant-...   # or OPENAI_API_KEY, or add a key later in the setup screen
./run.sh                              # builds the UI and the binary, then starts Ostra
```

Ostra prints a sign-in URL and opens it in your browser. The setup screen that follows creates the first
workspace. To keep Ostra running as a login service instead, run `./install.sh`.

### Step by step

**1. Install the prerequisites.**

| Tool | Why |
| --- | --- |
| [rustup](https://rustup.rs) | `rust-toolchain.toml` pins the toolchain, and rustup installs it on the first build. |
| Node.js 24 with npm | Builds the browser UI, which the binary embeds at compile time. |
| A C compiler (`build-essential` on Debian and Ubuntu, Xcode command line tools on macOS) | Some crates compile C code. |
| `git` | Ostra clones, branches, and commits in your projects. |
| bubblewrap (`bwrap`), Linux only | Sandboxes agent commands. macOS uses the built-in Seatbelt. |

The sandbox mode defaults to `required`, so on Linux without bubblewrap every execution is refused. Install
it (`sudo apt install bubblewrap`, `sudo dnf install bubblewrap`), or set `[sandbox] mode = "off"` in
`config.toml` to run agent commands with your full user rights on purpose.

**2. Give Ostra model access.** Use one or more of these:

- Export `ANTHROPIC_API_KEY` or `OPENAI_API_KEY` in the shell that starts Ostra. [Model access](#model-access)
  lists the other variables.
- Put `export NAME=value` lines in a file and point `OSTRA_ENV_FILE` at it. `run.sh` and the service apply
  only those lines, so nothing else in the file runs.
- Start without a key and save one in the setup screen. Ostra seals it in its registry database, never in
  `config.toml`.
- Sign in to a harness CLI (Claude Code, Codex, Grok Build, Antigravity) with its own login.

**3. Build.**

```bash
./build.sh
```

It runs `npm ci` the first time, rebuilds the UI only when `web/` or `design/` changed, and then runs
`cargo build --release -p ostra-server`. The first build takes several minutes. By hand, the same steps are:

```bash
npm ci && (cd web && npm run build)  # build the UI first, because the binary embeds web/dist
cargo build --release -p ostra-server
```

The binary is `target/release/ostra`.

**4. Start the server.**

```bash
./target/release/ostra                # or ./run.sh, which also applies $OSTRA_ENV_FILE
```

It binds to `127.0.0.1` on port 7878, or the next free port, prints a sign-in URL, and opens it. Useful flags:

| Flag | Effect |
| --- | --- |
| `--port 8080` | Listen on another port. |
| `--no-open` | Print the URL without opening a browser. |
| `--bind 0.0.0.0` | Listen on every interface. Read [Remote access](#remote-access) first. |
| `--allow-host ostra.example.com` | Accept a reverse proxy's domain as `Host`. Repeatable. |

`SKIP_BUILD=1 ./run.sh` starts the existing binary without building.

**5. Sign in and create a workspace.** The URL carries a one-time token in its fragment, so it works once. If
you lose the tab, run `ostra url` for a fresh one. The setup screen then checks this machine for keys and
harnesses, asks for a workspace name and folder, adds the projects to work on, and sets the default
permissions and model routing.

**6. Stop and restart.** Press Ctrl-C in the terminal that runs the server. Before starting it again, stop any
session you do not want resumed with `ostra stop <session-id>`, because the next start re-runs every
interrupted execution. The log is `server.log` in the data directory (`~/.local/share/ostra` on Linux,
`~/Library/Application Support/ostra` on macOS).

Other commands:

| Command | Does |
| --- | --- |
| `ostra url` | Print a fresh sign-in URL for the running server. |
| `ostra config` | Print the config path, writing the default config if none exists. |
| `ostra sessions` | List signed-in browsers; `ostra sessions revoke <id>` signs one out. |
| `ostra signout` | Sign out every browser. |
| `ostra stop <session-id>` | Stop a session while the server is not running. |

### Run as a service

`./install.sh` builds Ostra, installs `ostra` into `~/.local/bin` (or `$OSTRA_PREFIX/bin`), and registers a
launchd agent on macOS or a systemd user unit on Linux, so it starts at login and restarts when it fails.
Flags pass through to `ostra`, for example `./install.sh --port 8080`. The service listens on `127.0.0.1`
unless you pass `--bind`.

```bash
OSTRA_ENV_FILE=~/ostra.env ./install.sh   # install and start, applying the file's export lines
./install.sh restart                      # restart, for example after changing a key
./install.sh status
./install.sh url                          # print a fresh sign-in URL
./install.sh uninstall                    # remove the service; config and data are kept
```

The service applies the `export` lines of the credentials file at every start, and keeps the `PATH` of the
shell that installed it so it finds harness CLIs. On Linux it runs while you are logged in; run
`loginctl enable-linger $USER` to keep it running after logout. Service logs are in
`journalctl --user -u ostra` on Linux and `service.err.log` in the data directory on macOS.

On Windows, `.\install.ps1` takes the same commands and registers a Task Scheduler task at logon, with no
administrator rights. It reads no credentials file, so save keys in the setup screen or as user environment
variables ([Install](docs/start/install.md#windows)).

### Run in Docker

`docker-compose.yml` builds the image and publishes Ostra on `127.0.0.1:7878`. Before the first start:

1. Create the config folder and give it to UID 1000, because Ostra runs as 1000:1000 inside the container:
   `mkdir -p config && sudo chown 1000:1000 config`.
2. Replace `/srv/ostra/projects` on both sides of the projects mount with one absolute host folder that holds
   your projects and workspaces, writable by UID 1000. Both sides must match, because Ostra stores project
   paths literally.
3. Run `docker compose up -d`, then `docker compose exec ostra ostra url`. Open the printed link with its
   address replaced by `localhost:7878`, because the container prints its own network address.

Pass keys with an `environment:` or `env_file:` entry on the service. Harness CLIs and their logins persist in
the `ostra-home` volume.

### Remote access

To use Ostra from another machine, listen on every interface:

```bash
ostra --bind 0.0.0.0                    # or set `bind = "0.0.0.0"` under [server] in config.toml
```

The sign-in URL then uses this machine's LAN address. Every interface address and the host name are accepted
as `Host`; any other `Host` is refused, which blocks DNS rebinding. Behind a TLS reverse proxy, add its domain
with `--allow-host ostra.example.com` (or `allowed_hosts` under `[server]`). The hook bridge endpoints accept
local connections only.

Traffic over plain HTTP is not encrypted, and anyone who reaches the port with a sign-in link can run commands
as you. Prefer an SSH tunnel (`ssh -L 7878:127.0.0.1:7878 host`, keeping the default bind) or a TLS reverse
proxy. Browser push notifications need HTTPS or localhost.

## Model access

Ostra reads provider credentials from the environment, then the OS keychain. Keys never reach the browser.

| Provider | Variables |
| --- | --- |
| Anthropic | `ANTHROPIC_API_KEY`, or `ANTHROPIC_AUTH_TOKEN` (sent as a bearer token) with `ANTHROPIC_BASE_URL` for a gateway |
| OpenAI | `OPENAI_API_KEY`, optional `OPENAI_BASE_URL` |

Harness CLIs (Claude Code, Codex, Grok Build, Antigravity) run with their own logins. Settings show which are
installed and logged in.

Ostra never uses a consumer subscription through a proxy. [docs/providers](docs/providers/README.md) lists
the credentials each provider allows on each path and the terms that apply.

## Configuration

| File | Holds |
| --- | --- |
| `~/.config/ostra/config.toml` | Providers, tier-to-model tables per executor, harness commands, global permissions. `ostra config` writes the default and prints its path. |
| `<workspace>/.ostra/workspace.toml` | Projects, executor and model routing per agent, custom instructions, YOLO, permissions, notifications. Edited from the Settings screen, validated on save. |
| `<project>/.ostra/` | `INVENTORY.md`, `project.toml`, `skills/`, and `memory/knowledge.sqlite3`. Written by the init flow, committable. |

`OSTRA_CONFIG` and `OSTRA_DATA_DIR` override the config file and the data directory, which is useful for a
scratch setup.

## Repository layout

| Path | Contents |
| --- | --- |
| `crates/ostra-core` | Ids, settings and route resolution, pipeline enums, submit schemas, the event log, API types (exported to TypeScript) |
| `crates/ostra-engine` | Event-sourced session state, the planner, judge calls, the runner, the spawn factory |
| `crates/ostra-agents` | Embedded prompts, minijinja rendering per executor, spawn structs, the repo brief |
| `crates/ostra-exec-native` | The native agent loop |
| `crates/ostra-exec-harness` | Harness executors: PTY, per-harness adapters, hook bridge, MCP stdio shim |
| `crates/ostra-tools` | Read, Write, Edit, Bash, Grep, Glob, Skill, WebFetch, Report, Memory |
| `crates/ostra-policy` | Guards, permissions, bash parsing |
| `crates/ostra-providers` | Anthropic and OpenAI streaming clients, a scripted mock |
| `crates/ostra-store` | SQLite: workspace database, registry, project memory |
| `crates/ostra-notify` | Web Push without OpenSSL |
| `crates/ostra-code` | Code navigation for the Files view: tokenizer, per-project code index, LSP client, code providers |
| `crates/ostra-server` | The `ostra` binary: axum, auth, REST, WebSocket, embedded web build, CLI |
| `assets/` | Agent prompts and definitions, judge prompts, stack references, the meta-author skill |
| `web/` | The console: React, Vite, TypeScript |
| `design/` | The design system `web/` and `site/` share: tokens and React components |
| `site/` | The homepage and docs. Docs pages render this repository's Markdown |
| `.design-sync/` | Notes and config for syncing UI changes to the Claude Design project |
| `tests/conformance/` | Engine fixtures, one per rule ID of HANDOVER section 8.2 |

`cd site && npm run build` writes the homepage and docs to `site/dist`, a folder any static host can serve. It
builds the console with mock data first, for the homepage's product shot. Each page carries its own
Content-Security-Policy, because a static host sends none. To add a docs page, put the Markdown in `docs/` and
list it in `site/src/docs/pages.ts`.

## Tests

```bash
cargo test --workspace                 # unit, conformance, and the server end-to-end test
cargo clippy --workspace --all-targets -- -D warnings
(cd web && npm run typecheck && npx vitest run)
(cd design && npm run typecheck && npx vitest run)
(cd site && npm run typecheck && npx vitest run)
(cd tests/browser && npm test)         # the browser security suite, for the console and the site
```

The server end-to-end test (`crates/ostra-server/tests/e2e.rs`) drives a whole YOLO IMPLEMENT session through
the real server, engine, native loop, policy, and tools, with a scripted model. Live provider tests are
ignored by default: `cargo test -p ostra-providers -- --ignored` runs them when credentials are set.

`cargo test -p ostra-core` regenerates the TypeScript bindings in `web/src/api/gen/`.
