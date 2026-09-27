# The server

`ostra` is a single binary. It is the web server, the pipeline engine, the terminal host for harness CLIs,
and the command-line tool you use to manage it. This page follows a request through it, explains how live
updates reach the browser, and covers how the server starts, stops, and finds itself again.

## One binary, several roles

The `ostra` command does different jobs depending on its first argument (`crates/ostra-server/src/main.rs`):

| Command | What it does |
| --- | --- |
| `ostra` or `ostra serve` | Starts the server and opens the browser at a sign-in link |
| `ostra url` | Prints a fresh one-time sign-in link for the server that is running now |
| `ostra sessions` | Lists signed-in browsers, with first and last seen times, address, and browser |
| `ostra sessions revoke <id>` | Signs one browser out; `--all` signs out every browser |
| `ostra signout` | The same as `ostra sessions revoke --all` |
| `ostra stop <session-id>` | Stops a session while the server is down, so the next start does not re-run it |
| `ostra config` | Prints the config path, writing a default config if none exists |
| `ostra hook ...` | The hook bridge that harness CLIs call. You never run it yourself |
| `ostra mcp-stdio ...` | The MCP stdio shim that harness CLIs start. You never run it yourself |

`serve` takes a few flags:

- `--port <n>`: the port. Without it, Ostra uses `server.port` from the config, and otherwise tries 7878 and
  the next 19 ports before letting the OS pick one.
- `--no-open`: do not open a browser.
- `--bind <addr>`: listen on another address, such as `0.0.0.0` to reach Ostra from another device. The
  default is `127.0.0.1`.
- `--allow-host <name>`: accept an extra host name, such as a reverse proxy's domain. Repeatable.
- `--dev`: also accept the Vite dev server's host, for frontend work.

The `hook` and `mcp-stdio` roles exist because a harness CLI such as Claude Code can only talk to the
outside world by running a command. Ostra writes a hook configuration that runs `ostra hook` before each
tool call and an MCP registration that starts `ostra mcp-stdio`. Both relay to the running server over
`/internal/policy` and `/internal/mcp`, so a tool call inside Claude Code passes the same policy as a tool
call in the native loop.

## Startup

`app::run` in `crates/ostra-server/src/app.rs` starts the server in this order:

1. **Prepare the environment.** Before any thread starts, `scrub_startup_env` moves the process environment
   off the stack and zeroes the original strings. On macOS any unsandboxed process of the same user, an agent
   command in mode `off` included, can read another process's startup environment, and that is where provider
   keys would sit. Ostra's Seatbelt policy refuses it to sandboxed commands.
   Then `extend_path` adds the usual install folders to `PATH`, so a harness CLI installed while the server
   runs is found without a restart.
2. **Read the global config** and bind the listener.
3. **Stop leftovers.** On macOS, processes that an earlier server started inside a sandbox are not ended with
   their parent. If no other server is alive, Ostra ends them before recovery starts executions again. Then,
   on a macOS admin account, it starts reading the sandbox's decoy reports from the system log, so the first
   execution's decoys count from its first command.
4. **Open the data dir.** It loads the cached model prices (refreshed from models.dev once a day in the
   background), opens the registry, and unlocks it with the master key, sealing any plaintext credential it
   finds. It writes the embedded prompts and skills to `assets/` in the data dir.
5. **Build the shared pieces.** Providers from the config and saved credentials, the code index, the MCP
   gateway, the native executor, the Web Push notifier, and a check of which harness CLIs are installed.
6. **Attach every workspace.** For each registered workspace, it opens `workspace.db`, starts an engine, and
   runs recovery. Recovery marks any execution that was running as `Interrupted` (keeping what it already
   spent), denies any open permission question because the execution that asked it has ended, and restarts
   the planner for every session that has not finished. A
   workspace whose `.ostra/workspace.toml` is missing is skipped with a warning, not deleted
   ([Workspaces](../internals/workspaces.md#opening-a-workspace)).
7. **Write `server.json`** and print the sign-in link.
8. **Serve** until Ctrl-C, then remove `server.json`.

If you bind to anything other than loopback, the server prints a warning before it serves: anyone who
reaches the port with a sign-in link can run commands as you, and plain HTTP exposes the cookie to the
network path. It recommends an SSH tunnel or a TLS reverse proxy.

## `server.json` and finding the running server

`server.json` in the data dir records the running server:

```json
{"pid": 2832910, "port": 7878, "url_host": "127.0.0.1:7878"}
```

The CLI commands read it. `ostra url` uses the port and host to build a new link. `ostra stop` refuses when
the pid in it is alive, because the running server holds the session in memory and would not see an
offline stop. The check is `kill(pid, 0)`, which asks whether the process exists without sending it a
signal. A stale file left by a crash is harmless: its pid is dead, so the next start overwrites it.

This is also why `ostra stop` exists. If the server dies in the middle of a session, the next start
recovers the session and the planner starts the interrupted executions again. When you do not want that,
run `ostra stop <session-id>` first. It opens the workspace database directly, appends the stop, and cancels
the session's running executions.

## Routes

Everything is served from one axum router (`router` in `crates/ostra-server/src/api.rs`):

- `/api/...` is the REST API: workspaces, projects, git, files, code navigation, sessions, gates, decisions,
  executions, artifacts, uploads, MCP, cost, search, and push subscriptions. The full list is in the
  [API reference](../../HANDOVER.md#13-api).
- `/ws` is the WebSocket.
- `/internal/policy` and `/internal/mcp` are the harness callbacks. They accept only connections from this
  machine and only a token that belongs to a running execution, which expires when the execution ends.
- `/mcp/oauth/callback` receives the redirect of an MCP server's OAuth sign-in.
- Everything else is the embedded console.

One middleware, `guard`, runs in front of all of it. It checks the `Host` header, the `Origin`, and the
sign-in cookie, and adds the security headers to every response, including refusals. How each check works is
in [Server and browser security](../security/server-and-browser.md).

Errors have one shape. A validation failure is `422` with every problem at once, each naming its field:

```json
{"error": "Fix the commands and save again.",
 "issues": [{"path": "commands.test", "message": "Write the command on one line; chain steps with `&&`."}]}
```

Other errors carry the same `error` field with an empty `issues` list. Settings use a separate `validate`
endpoint with the same body, which reports every issue and writes nothing, so the browser can show problems
as you type.

## Live updates over one WebSocket

Each browser tab opens one WebSocket to `/ws` and subscribes to channels on it:

| Channel | What arrives |
| --- | --- |
| `session:<id>` | Every event appended to the session, and its updated summary |
| `execution:<id>` | Execution deltas (text, thinking, tool calls and results, policy decisions, usage) and status changes |
| `term:<execution>` | Raw terminal bytes of a harness run, as binary frames |
| `workspace:<ws>` | Session summaries, Sessions tree patches, running work and spend, files an execution wrote, clone progress |
| `home` | Session summaries for every workspace, and harness install status |

The path from an event to your screen is short. `Inner::append` in the engine stores the event and sends an
`EngineNotice` on a broadcast channel. The server forwards every engine's notices into one hub, and each
socket routes a notice to the channels it names (`route` in `crates/ostra-server/src/ws.rs`). A socket that
subscribed to the channel sends it on.

A few choices keep this fast under load:

- **Terminals have their own path.** PTY bytes never travel through engine notices. Each harness execution's
  terminal is streamed straight from the PTY registry to the sockets watching it, so a busy terminal cannot
  delay session events.
- **A slow viewer is resynced, not buffered forever.** A socket queues at most 64 terminal frames. A viewer
  that falls behind gets a fresh snapshot of the screen (the scrollback, the formatted screen, and the
  input modes) instead of a gap. Subscribing to a terminal starts with the same snapshot, and a finished run
  replays its stored transcript.
- **Busy channels are throttled.** Tree patches go out at most four times a second per session, activity at
  most twice a second, and file-change notices are grouped over a short window.
- **Binary frames are small.** A terminal frame is one length byte, the execution id, and the bytes, with no
  JSON around them.

The socket also has limits: 1 MiB per message, 64 KiB per terminal input, and 256 channels. It re-checks its
sign-in every few seconds, so revoking a browser from the CLI closes its sockets within 5 seconds; revoking it
from the console closes them at once.

## The embedded console

The React console in `web/` is compiled into the binary with `rust-embed` (`crates/ostra-server/src/assets.rs`).
There is nothing to install next to `ostra`, and the console always matches the server that serves it.

Files under `assets/` have hashed names, so they are served with a one-year immutable cache. Everything else,
including `index.html`, is `no-cache`, so an upgrade takes effect on the next load. Any path that is not a
file loads `index.html`, so a deep link such as `/w/ws_1/f/app/src/main.rs` opens the right screen. Paths
that look like assets or API calls return 404 instead, so a missing script is an error and not a page of
HTML.

Because the build is embedded at compile time, run `npm run build` in `web/` before `cargo build --release`
when the console changed. A binary built without it serves a one-line page that says so.

## Web Push without OpenSSL

Ostra can notify your phone or desktop when a gate needs your answer or a session finishes. The browser's
service worker subscribes, and the server sends the notification through the browser vendor's push service.

`crates/ostra-notify` implements the protocol from the RFCs with pure-Rust cryptography crates (`p256`,
`hkdf`, `aes-gcm`, `sha2`), and the HTTP client uses `rustls`:

- **RFC 8291** message encryption: an ephemeral P-256 key agreement with the subscription's key, HKDF to
  derive the content key and nonce, and AES-128-GCM.
- **RFC 8188** `aes128gcm` content coding, with the whole message in one 4096-byte record.
- **RFC 8292** VAPID: an ES256-signed JWT, valid for 12 hours, that proves the push came from this server.

The VAPID key pair is generated once and the private half is sealed in the registry. Messages live for 24
hours at the push service and are capped at 3993 bytes of plaintext, which leaves room for the header and
the tag inside the push services' 4096-byte limit.

Dropping OpenSSL means `ostra` builds with no system crypto libraries and no C toolchain beyond what SQLite
needs, which keeps the binary easy to build on Linux, macOS, and inside minimal containers.

Push needs a secure origin, so it works on `localhost` and over HTTPS, but not on a plain-HTTP LAN address.

## Logs

The server logs to standard output and to `server.log` in the data dir, so a problem seen in a browser on
another device can be read back later. `OSTRA_LOG` sets the filter (for example `OSTRA_LOG=debug`). Logs never
contain API keys, tokens, or file contents.

## Where to go next

- [Quick start](../start/quick-start.md) and [install](../start/install.md): building and starting `ostra`.
- [Troubleshooting](../start/troubleshooting.md): a server that will not start, a spent sign-in link, a
  session that re-runs after a restart.
- [OS compatibility](../platforms/os-compatibility.md): what differs on Linux and macOS.
- [Executors](../internals/executors.md) and [MCP](../internals/mcp.md): what the hook bridge and the MCP shim
  connect to.
- [Server and browser security](../security/server-and-browser.md): the checks `guard` runs.
- [Storage](storage.md): the files the server reads and writes.
