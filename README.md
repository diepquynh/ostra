# Ostra

Ostra is a local development workspace that runs the Ultracode engineering pipeline: research, spec,
fact-check, plan, build, review, test, and docs. You start one binary and work in a browser tab. A Rust server
executes every tool call, code drives the pipeline from stage to stage and holds every gate, and models do the
work inside each stage. `HANDOVER.md` is the design brief this implementation follows.

## Build and run

`./run.sh` does all of this in one step: it builds the UI only when it changed, builds the release binary,
applies the `export` lines of `$OSTRA_ENV_FILE` when it is set, and starts `ostra` with any
flags you pass, for example `./run.sh --bind 0.0.0.0`. `SKIP_BUILD=1 ./run.sh` starts the existing binary.

To run Ostra as a service that starts at login, run `./install.sh` (flags pass through, for example
`./install.sh --port 8080`). The service listens on every interface; pass `--bind 127.0.0.1` to keep it
local, and read the remote access notes below. It builds, installs `ostra` into `~/.local/bin` (or `$OSTRA_PREFIX/bin`),
and registers a launchd agent on macOS or a systemd user unit on Linux. The service applies the `export`
lines of the credentials file at every start, so after changing a key run `./install.sh restart`. The
other commands are `status`, `url`, and `uninstall`, which keeps your config and data.

By hand:

```bash
(cd web && npm ci && npm run build)   # the browser UI, embedded into the binary
cargo build --release
./target/release/ostra                # prints a sign-in URL and opens it
```

The server binds to `127.0.0.1` by default, on port 7878 or the next free one. The printed URL carries a
one-time token in its fragment. If you lose the tab, run `ostra url` for a fresh one.

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
| `web/` | React, Vite, TypeScript |
| `.design-sync/` | Notes and config for syncing UI changes to the Claude Design project |
| `tests/conformance/` | Engine fixtures, one per rule ID of HANDOVER section 8.2 |

## Tests

```bash
cargo test --workspace                 # unit, conformance, and the server end-to-end test
cargo clippy --workspace --all-targets -- -D warnings
(cd web && npm run typecheck && npx vitest run)
```

The server end-to-end test (`crates/ostra-server/tests/e2e.rs`) drives a whole YOLO IMPLEMENT session through
the real server, engine, native loop, policy, and tools, with a scripted model. Live provider tests are
ignored by default: `cargo test -p ostra-providers -- --ignored` runs them when credentials are set.

`cargo test -p ostra-core` regenerates the TypeScript bindings in `web/src/api/gen/`.
