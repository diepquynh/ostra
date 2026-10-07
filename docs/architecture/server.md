# The server

`ostra` is one binary. It is the web server, the pipeline engine, and the terminal host for harness CLIs. It is
also the command-line tool that you use to manage the server. This page follows a request through the binary.
It explains how live updates go to the browser. It also tells how the server starts, stops, and finds the
running server again.

## One binary, several roles

The first argument of the `ostra` command selects its job (`crates/ostra-server/src/cli.rs`):

| Command | What it does |
| --- | --- |
| `ostra` or `ostra serve` | Starts the server and opens the browser at a sign-in link |
| `ostra url` | Prints a new one-time sign-in link for the server that runs now |
| `ostra sessions` | Lists the signed-in browsers, with the first and last seen times, the address, and the browser |
| `ostra sessions revoke <id>` | Signs one browser out. `--all` signs out all browsers |
| `ostra signout` | The same as `ostra sessions revoke --all` |
| `ostra stop <session-id>` | Stops a session when the server is down. Then the next start does not run the session again |
| `ostra config` | Prints the config path. If no config exists, it writes a default config |
| `ostra plugin add <name> -- <command>...` | Registers a plugin program in the `[[plugins]]` of the workspace that holds the current folder. `--workspace` selects a different workspace |
| `ostra hook ...` | The hook bridge that harness CLIs call. Do not run it yourself |
| `ostra mcp-stdio ...` | The MCP stdio shim that harness CLIs start. Do not run it yourself |

`serve` takes these flags:

- `--port <n>`: the port. Without this flag, Ostra uses `server.port` from the config. If that key is not set,
  Ostra tries 7878 and the next 19 ports. If all of them are in use, the OS selects a port.
- `--no-open`: do not open a browser.
- `--bind <addr>`: listen on a different address. For example, use `0.0.0.0` to connect to Ostra from a different
  device. The default is `127.0.0.1`.
- `--allow-host <name>`: accept one more host name, for example the domain of a reverse proxy. You can use this
  flag more than one time.
- `--dev`: also accept the host of the Vite dev server, for frontend work.

The `hook` and `mcp-stdio` roles exist because a harness CLI such as Claude Code can only send data out through
a command. Ostra writes a hook configuration that runs `ostra hook` before each tool call. It also writes an MCP
registration that starts `ostra mcp-stdio`. The two commands send their data to the running server through
`/internal/policy` and `/internal/mcp`. Thus, a tool call in Claude Code passes the same policy as a tool call in
the native loop.

## Startup

`app::run` in `crates/ostra-server/src/app.rs` starts the server in this order:

1. **Prepare the environment.** Before a thread starts, `scrub_startup_env` moves the process environment off
   the stack and writes zeros over the original strings. On macOS, each process of the same user without a
   sandbox can read the startup environment of a different process. This includes an agent command in mode
   `off`. The provider keys are in that environment. The Seatbelt policy of Ostra denies this read to
   sandboxed commands. Then `extend_path` adds the usual install folders to `PATH`. Thus, the server finds a
   harness CLI that you install during its run, without a restart.
2. **Read the global config** and bind the listener.
3. **Stop leftovers.** On macOS, a process that an earlier server started in a sandbox does not end with its
   parent. If no other server is alive, Ostra ends these processes before recovery starts executions again.
   Then, on a macOS admin account, Ostra starts to read the decoy reports of the sandbox from the system log.
   Thus, the decoys of the first execution count from its first command.
4. **Open the data dir.** Ostra loads the cached model prices. A background job refreshes them from models.dev
   one time each day. Ostra opens the registry and unlocks it with the master key. It seals each plaintext
   credential that it finds. It writes the embedded prompts and skills to `assets/` in the data dir.
5. **Build the shared pieces.** These pieces are:
   - The providers from the config and the saved credentials.
   - The code index.
   - The MCP gateway.
   - The native executor.
   - The Web Push notifier.
   - A check of which harness CLIs are installed.
6. **Attach every workspace.** For each registered workspace, Ostra opens `workspace.db`, starts an engine, and
   runs recovery. Recovery does these actions:
   - It marks each execution that was running as `Interrupted`, and keeps the spend of that execution.
   - It denies each open permission question, because the execution that asked it ended.
   - It starts the planner again for each session that did not finish.

   If the `.ostra/workspace.toml` of a workspace is missing, Ostra skips the workspace with a warning. It does
   not delete the workspace ([Workspaces](../internals/workspaces.md#opening-a-workspace)).
7. **Write `server.json`** and print the sign-in link.
8. **Serve** until Ctrl-C, then remove `server.json`.

If you bind to an address that is not loopback, the server prints a warning before it serves. The warning
gives two risks. A person who connects to the port with a sign-in link can run commands as you. Plain HTTP shows
the cookie to the network path. The warning recommends an SSH tunnel or a TLS reverse proxy.

## `server.json` and finding the running server

`server.json` in the data dir records the running server:

```json
{"pid": 2832910, "port": 7878, "url_host": "127.0.0.1:7878"}
```

The CLI commands read this file. `ostra url` uses the port and the host to build a new link. If the pid in the
file is alive, `ostra stop` refuses to run. The reason is that the running server holds the session in memory,
and it does not see an offline stop. The check is `kill(pid, 0)`. This call finds whether the process exists,
and it sends no signal to the process. An old file from a crash causes no problem. Its pid is dead, so the next
start writes over it.

This behavior is also the reason for `ostra stop`. If the server stops during a session, the next start
recovers the session. Then the planner starts the interrupted executions again. To prevent this, run
`ostra stop <session-id>` before the next start. The command opens the workspace database directly and appends
the stop. It also cancels the running executions of the session.

## Routes

One axum router serves all paths (`router` in `crates/ostra-server/src/api.rs`):

- `/api/...` is the REST API. It covers workspaces, projects, git, files, code navigation, sessions, gates,
  decisions, executions, artifacts, uploads, MCP, cost, search, and push subscriptions. The
  [API reference](../../HANDOVER.md#13-api) gives the full list.
- `/ws` is the WebSocket.
- `/internal/policy` and `/internal/mcp` are the harness callbacks. They accept only connections from this
  computer. They accept only a token that belongs to a running execution. The token expires when the execution
  ends.
- `/mcp/oauth/callback` receives the redirect of the OAuth sign-in of an MCP server.
- All other paths are the embedded console.

One middleware, `guard`, runs before all routes. It checks the `Host` header, the `Origin`, and the sign-in
cookie. It adds the security headers to each response, refusals included.
[Server and browser security](../security/server-and-browser.md) explains each check.

All errors have one shape. A validation failure is `422` with all problems together. Each problem names its
field:

```json
{"error": "Fix the commands and save again.",
 "issues": [{"path": "commands.test", "message": "Write the command on one line; chain steps with `&&`."}]}
```

Other errors have the same `error` field with an empty `issues` list. Settings use a separate `validate`
endpoint with the same body. This endpoint reports each issue and writes nothing. Thus, the browser can show
problems when you type.

## Live updates over one WebSocket

Each browser tab opens one WebSocket to `/ws`, and subscribes to channels on it:

| Channel | What arrives |
| --- | --- |
| `session:<id>` | Each event that the engine appends to the session, and the updated summary of the session |
| `execution:<id>` | Execution deltas (text, thinking, tool calls and results, policy decisions, usage) and status changes |
| `term:<execution>` | The raw terminal bytes of a harness run, as binary frames |
| `workspace:<ws>` | Session summaries, Sessions tree patches, running work and spend, the files that an execution wrote, clone progress |
| `home` | Session summaries for each workspace, and the install status of each harness |

An event goes to your screen in few steps. `Inner::append` in the engine stores the event and sends an
`EngineNotice` on a broadcast channel. The server sends the notices of all engines into one hub. Each socket
routes a notice to the channels that the notice names (`route` in `crates/ostra-server/src/ws.rs`). A socket that
subscribed to the channel sends the notice to the browser.

These design decisions keep the path fast under load:

- **Terminals have their own path.** PTY bytes never go through engine notices. The PTY registry streams the
  terminal of each harness execution directly to the sockets that watch it. Thus, a busy terminal cannot delay
  session events.
- **A slow viewer gets a new snapshot, not an unlimited buffer.** A socket queues at most 64 terminal frames. If
  a viewer falls behind, it gets a new snapshot of the screen and not a gap. The snapshot holds the scrollback,
  the formatted screen, and the input modes. A new subscription to a terminal starts with the same snapshot. A
  finished run replays its stored transcript.
- **Busy channels have a rate limit.** Tree patches go out at most four times each second for each session.
  Activity goes out at most two times each second. The server puts file-change notices into groups over a short
  time window.
- **Binary frames are small.** A terminal frame is one length byte, the execution id, and the bytes. No JSON
  surrounds them.

The socket also has these limits: 1 MiB for each message, 64 KiB for each terminal input, and 256 channels. The
socket checks its sign-in again every few seconds. Thus, if you revoke a browser from the CLI, its sockets close
in 5 seconds or less. If you revoke it from the console, its sockets close immediately.

## The embedded console

The build compiles the React console in `web/` into the binary with `rust-embed`
(`crates/ostra-server/src/assets.rs`). You install nothing next to `ostra`. The console always agrees with the
server that serves it.

Files under `assets/` have hashed names. Thus, the server serves them with a one-year immutable cache. All other
files, `index.html` included, are `no-cache`. Thus, an upgrade has an effect at the next load. Each path that is
not a file loads `index.html`. Thus, a deep link such as `/w/ws_1/f/app/src/main.rs` opens the correct screen.
But paths that look like assets or API calls return 404. Thus, a missing script is an error and not a page of
HTML.

When the console changed, run `npm run build` in `web/` before `cargo build --release`. The reason is that the
build embeds the console at compile time. A binary built without this step serves a one-line page that tells
you so.

## Web Push without OpenSSL

Ostra can send a notification to your phone or desktop when a gate needs your answer or a session finishes.
The service worker of the browser subscribes. The server sends the notification through the push service of
the browser vendor.

`crates/ostra-notify` implements the protocol from the RFCs with pure-Rust cryptography crates (`p256`,
`hkdf`, `aes-gcm`, `sha2`). The HTTP client uses `rustls`. The crate implements these RFCs:

- **RFC 8291** message encryption: an ephemeral P-256 key agreement with the key of the subscription, HKDF to
  derive the content key and nonce, and AES-128-GCM.
- **RFC 8188** `aes128gcm` content coding, with the full message in one 4096-byte record.
- **RFC 8292** VAPID: an ES256-signed JWT that is valid for 12 hours. It proves that the push came from this
  server.

Ostra generates the VAPID key pair one time and seals the private half in the registry. Messages stay at the
push service for 24 hours. A message has a maximum of 3993 bytes of plaintext. This limit leaves space for the
header and the tag in the 4096-byte limit of the push services.

Without OpenSSL, `ostra` builds with no system crypto libraries. It needs no C toolchain other than the one
that SQLite needs. Thus, the binary is easy to build on Linux, on macOS, and in minimal containers.

Push needs a secure origin. Thus, it works on `localhost` and over HTTPS, but not on a plain-HTTP LAN address.

## Logs

The server writes its log to standard output and to `server.log` in the data dir. Thus, you can read a
problem later that you saw in a browser on a different device. `OSTRA_LOG` sets the filter, for example
`OSTRA_LOG=debug`. Logs never contain API keys, tokens, or file contents.

## Where to go next

- [Quick start](../start/quick-start.md) and [install](../start/install.md): how to build and start `ostra`.
- [Troubleshooting](../start/troubleshooting.md): a server that does not start, a used sign-in link, and a
  session that runs again after a restart.
- [OS compatibility](../platforms/os-compatibility.md): the differences between Linux and macOS.
- [Executors](../internals/executors.md) and [MCP](../internals/mcp.md): the parts that the hook bridge and the
  MCP shim connect to.
- [Server and browser security](../security/server-and-browser.md): the checks `guard` runs.
- [Storage](storage.md): the files the server reads and writes.
