# MCP servers

A workspace can list Model Context Protocol servers, such as a GitHub server, a docs lookup, or an issue
tracker. Each agent in that workspace can use the tools of these servers on every executor. Ostra is the only
MCP client in this design. Ostra does these steps itself:

1. It connects to each server.
2. It signs in to the server.
3. It lists the tools of the server.
4. It gives those tools to native runs directly, and to harness runs through its own MCP server.

This page tells why Ostra works this way and how Ostra makes and keeps a connection. It also tells how OAuth
sign-in works and how MCP tool calls pass the same policy as all other tool calls. The client is in the
`ostra-mcp` crate. The gateway for each workspace, which owns the connections, is
`crates/ostra-server/src/mcp.rs`.

## Why Ostra is the only client

Each harness CLI has its own method to register MCP servers, with different config files, flags, and limits.
Antigravity registers servers only through a global `agy mcp add`. On the version that Ostra measured, Codex
cancelled MCP calls under its default approval policy. If each harness
registers workspace servers itself, each harness needs its own configuration. Each harness also applies
permissions in its own way.

Ostra prevents these problems because it keeps the connection on its own side:

```text
                     ┌──────────── Ostra server ────────────┐
native agent loop ──▶│                                      │
                     │  policy check ─▶ MCP gateway ─▶ ─────┼──▶ github (HTTP, OAuth)
harness CLI ─────────┼▶ ostra mcp-stdio ─▶ policy check ─┘  │──▶ docs (stdio, local command)
 (Claude Code,       │                                      │──▶ linear (HTTP, OAuth)
  Codex, Grok, agy)  └──────────────────────────────────────┘
```

- Ostra keeps one connection for each workspace server. All executions in the workspace share it.
- One policy and one allowlist cover all four harnesses and the native loop.
- A harness sees workspace tools next to the tools of Ostra (`report`, `memory`, the code tools, and the
  `submit_*` tools). It gets them through the one MCP registration shape that works on every harness: a stdio
  server (`ostra mcp-stdio --execution <id>`).

## Configuring a server

The servers are in `workspace.toml` under `mcp_servers`. A `command` starts a local server. A `url` connects to
a remote server.

```toml
[[mcp_servers]]
name = "github"
url = "https://api.githubcopilot.com/mcp/"
headers = { Authorization = "Bearer ${GITHUB_TOKEN}" }
disabled_tools = ["delete_repository"]

[[mcp_servers]]
name = "docs"
command = ["npx", "-y", "@upstash/context7-mcp"]
agents = ["explore", "generate-spec"]   # empty or absent: every agent
timeout_secs = 120                      # one tool call, 1 to 600

[[mcp_servers]]
name = "linear"
url = "https://mcp.linear.app/mcp"      # signs in with OAuth when the server asks
```

The Settings screen edits the same list as a form. For each server, it shows these items:

- The live state.
- The name and version that the server reports.
- A sign-in button for remote servers.
- A reconnect button.
- A switch for each tool. The switch edits `disabled_tools`.

A server name contains only lowercase letters, digits, and dashes. It has at most 24 characters, and it is
never `ostra`. The name is part of each tool name, so Ostra must split it from the tool name without error.

The MCP servers tab of Settings edits each server entry. Each server shows its live state, its version, and a
switch for each tool. In this screenshot, `docs` is available to two agents, and `linear` waits for sign-in:

![The MCP servers settings tab with a remote server, a local server, and a server that needs sign-in](../images/console/settings-mcp.png)

### Secrets stay out of the file

`workspace.toml` can go with a repository, so it must never hold a secret. Two mechanisms keep secrets out of
it:

- An `env` or `headers` value can name a variable of the environment of the Ostra server as `${VAR}`. If the
  variable is not set, the connection fails with an error that names the variable. The error gives the fix:
  set the variable and restart the server.
- If the user types a value in Settings as plain text, Ostra stores it encrypted in its registry. The file then
  holds the placeholder `${ostra_secret}`. Thus the value never goes to the browser or a folder file again.

Ostra refuses some variables. The `env` and `headers` of a server cannot name these variables:

- A provider API key.
- A bridge token.
- An OAuth client secret of another server.

The process of a local server also does not inherit these variables from the environment of Ostra. Thus a
hostile or careless `workspace.toml` cannot send your Anthropic key to an MCP server.

### Nothing starts before you approve it

A `workspace.toml` can arrive with a clone or a `git pull`, and an `mcp_servers` entry is a command that Ostra
runs. Rule A1 controls the parts of a folder file that hold commands. These parts start only when the registry
holds the approval of the user for the exact hash of that file. For MCP servers, these parts are `command`,
`env`, `url`, `headers`, `oauth`, and `enabled`.

When the file waits for approval, no server starts. The Settings screen shows exactly what Ostra will run, with
an Approve button. It shows commands and the names of env variables and headers, but never their values. The
approval holds the hash that the browser showed. If the file changed after that, Ostra refuses the approval.
The gateway connects from the approved copy of the file, not from the current file on disk:

```rust
// Rule A1: no server starts from a workspace file that waits for approval, and it
// starts from the approved copy.
let approved = crate::trust::approved_workspace_file(&self.registry, root)
    .ok_or_else(|| ConnError::Other(crate::trust::PENDING_MESSAGE.into()))?;
```

When you save a file in Ostra, the file stays approved. Thus Ostra does not ask again for your own edits.

## Transports

### Local servers over stdio

A `command` server starts in the workspace root, in its own process group. Ostra stops it when its connection
closes. The server is a program of the workspace, so it runs in the agent sandbox if the platform has one
(bubblewrap on Linux, Seatbelt on macOS). Ostra keeps the last 4 KiB of its standard error to explain a crash.

### Remote servers over streamable HTTP

Ostra connects to a `url` server over the streamable HTTP transport. The server can answer with JSON or with
server-sent events, and it gives an `Mcp-Session-Id`. Ostra uses protocol versions 2024-11-05 through
2025-11-25. Ostra does not support the older HTTP plus SSE transport.

Ostra follows a redirect only in the origin of the first request, and never from `https` to `http`. The reason
is that a header with a token must never go to another host.

## Connection lifecycle

The gateway keeps one connection for each workspace and server name. The key of a connection is a fingerprint
of the important parts of its settings: command, env, URL, headers, OAuth, and the sandbox mode. If any of
these parts change, the next use starts a new connection.

A connection stays open until the user removes, turns off, or changes its server, not only until Ostra stops.
Three actions compare the open connections with the workspace file:

- A save of the workspace settings.
- A read of the MCP status.
- The start of an execution.

Each of these actions closes the connections that the file no longer runs in their current form. When Ostra
closes a connection to a local server, it ends the full process tree of the server. Thus a server that `npx`
started stops, and the browser that it opened also stops. The close also ends the connection for executions that
still hold it. Their next call to that server fails, and it does not go to a server that the user turned off. If
the workspace file does not load, Ostra closes no connections, because a half-saved edit must not stop every
server.

| Situation | What happens |
| --- | --- |
| An execution opens | Ostra connects each server that serves this agent, or it uses the live connection again |
| The tool list is older than 30 seconds, or the server announced a change | Ostra gets the list again |
| A server failed | Ostra does not try it again for 20 seconds, because each parallel execution can start it again |
| A server is not available when an execution opens | The execution gets one Activity line and runs without that server |
| A server exited during an execution | Ostra starts it one more time on the next call, unless the user turned it off after the exit |
| A server is turned off, removed, or changed | Its connection closes and its processes end |

Ostra fixes the tools when an execution opens, before a harness starts. The reason is that a harness lists MCP
tools only one time.

## Signing in with OAuth

A remote server that answers 401 or 403 needs a sign-in. Its state in Settings changes to "needs sign-in". The
error that an agent sees tells the user where to sign in. Ostra follows the MCP authorization spec:

1. **Discovery.** Ostra reads the resource metadata URL from the `WWW-Authenticate` challenge. If there is no
   such URL, Ostra tries the RFC 9728 well-known paths. From there, Ostra reads the RFC 8414 or OpenID metadata
   of the authorization server. If a server publishes no metadata, Ostra uses the 2025-03-26 default endpoints.
2. **Client registration.** Ostra registers itself as a public client (RFC 7591), unless the entry sets
   `oauth.client_id`. With `oauth.client_id`, the entry can also set `oauth.client_secret_env` to the name of
   the variable that holds the secret.
3. **Authorization.** `POST /api/workspaces/:ws/mcp/:name/login` returns the authorization URL. Ostra builds
   this URL with PKCE (S256), the `resource` parameter, and a single-use `state`. The browser opens the URL.
4. **Callback.** The authorization server redirects to `<browser origin>/mcp/oauth/callback`. This path is
   outside `/api`, because a cross-site redirect does not carry the `SameSite=Strict` cookie of Ostra. The
   `state` authenticates the callback in place of the cookie. The `state` is valid for 10 minutes, and the
   callback can use it only one time.
5. **Tokens.** Ostra stores the client and its tokens encrypted in the registry, for each workspace and server.

After the sign-in, Ostra refreshes tokens automatically:

- Ostra refreshes an expired token before use.
- If the server refuses a token, Ostra refreshes it one time.
- Ostra does one refresh at a time for each server. The reason is that a refresh token can be single-use, and
  two parallel refreshes can lock the user out.
- If a refresh fails, Ostra clears the tokens and asks for a new sign-in.

Sign out (`POST .../mcp/:name/logout`) deletes the tokens.

Ostra checks each endpoint URL in the metadata of a server before Ostra fetches the URL or sends the browser to
it. The URL must be `https`, or plain `http` on a loopback host or on the origin of the server itself. Ostra
refuses all other URLs:

```rust
/// An OAuth URL Ostra fetches or sends the browser to must be `https`, or plain `http` on a
/// loopback host or on the MCP server's own origin (which the user chose). Anything else, such
/// as a `javascript:` URL from a hostile server's metadata, is refused, because the browser would
/// run it on Ostra's origin.
pub fn check_endpoint(what: &str, url: &str, server_origin: Option<&str>) -> Result<(), String> {
```

If a server asks for a sign-in but publishes no OAuth metadata, Ostra cannot sign in to it. Ostra tells the user
this and suggests a token in a header.

## How tools reach an agent

When an execution opens, the gateway does these steps:

1. It collects the tools of each enabled server whose `agents` list includes the agent. An empty list means
   every agent.
2. It removes the tools that `disabled_tools` names.
3. It gives the executor a fixed list.

Each tool keeps the description and input schema that its server gives.

### Names

The name of a workspace tool is `mcp__<server>__<tool>`, which is the canonical form of Claude Code. The native
loop, the Activity view, and permission rules use this name. A harness sees the tool through the server of
Ostra as `mcp__ostra__<server>__<tool>`.

Providers limit tool names to 64 characters, and some tool names contain characters that providers refuse.
Ostra shortens a tool name in two cases:

- The name is not `[A-Za-z0-9_-]`.
- The `<server>__<tool>` part is longer than 52 characters.

Ostra adds a hash of the original name to the shortened name. Thus two tools never get the same name.

### Results

Results are text. Ostra changes images and binary resources to a one-line note, and structured content to JSON.
Ostra cuts a result at 100 KiB, because one large answer can fill the context of the model.

## Policy

Each MCP call passes `ExecutionPolicy::check`, the same as all other tool calls. Two rules control the check.

**Rule M1: configured means allowed.** Ostra allows a workspace MCP tool unless a permission rule says
otherwise. The reason is that the addition of the server to the workspace is the decision to use it. Rules can
make the permission narrower:

```toml
[permissions]
deny = ["mcp__github__delete_repository"]
ask  = ["mcp__linear"]    # every tool of the linear server asks first
```

Plan mode is read-only. Thus in plan mode, Ostra allows only the tools that the server marks with
`readOnlyHint`. The other tools get the default of the mode, because Ostra cannot know which of them write. A
harness can register its own MCP tools, not through Ostra. These tools are not workspace tools, and they keep
the default of the mode.

**Rule M2: one check, one log line.** A harness gets a workspace tool only through the MCP server of Ostra. The
hook bridge sees the tool call of the harness first, and it lets the call through without a check. Then the MCP
handler does these steps:

1. It checks the call.
2. It asks the user, if necessary.
3. It runs the call.
4. It logs the call one time.

A check at both points asks the user two times for the same call.

```rust
// Rule M2: a workspace MCP tool reaches the harness only through Ostra's MCP server,
// which checks and logs the call when it runs it, so the hook passes it unchecked.
if ostra_core::mcp::is_gateway_tool(&call.tool) && !r.inspect {
    return PolicyDecision::Allow { rule: None };
}
```

A read-only session gets no workspace tools. An example is a session that opens an ended harness run to scroll
through it.

## Status API

| Endpoint | What it does |
| --- | --- |
| `GET /api/workspaces/:ws/mcp` | Connects each server and reports its state, server info, sign-in state, and tools |
| `POST /api/workspaces/:ws/mcp/:name/refresh` | Connects again and lists the tools again |
| `POST /api/workspaces/:ws/mcp/:name/login` | Returns the authorization URL for an OAuth sign-in |
| `POST /api/workspaces/:ws/mcp/:name/logout` | Deletes the OAuth tokens of the server |
| `GET /mcp/oauth/callback` | The redirect target of the authorization server |

## Where to read next

- [Executors](executors.md): how the native loop and the harness CLIs run, and where the MCP shim is.
- [Tools](tools.md): the tools of Ostra, which go over the same MCP server.
- [Code index](code-index.md): the code tools that a harness gets as `mcp__ostra__code_*`.
- [Settings and routing](settings-and-routing.md): where `mcp_servers` goes in `workspace.toml`.
- [Agent containment](../security/agent-containment.md): the policy layers and the sandbox that a local server runs in.
- [Secrets and data](../security/secrets-and-data.md): how the registry stores OAuth tokens and saved header values.
