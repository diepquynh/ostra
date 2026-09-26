# MCP servers

A workspace can list Model Context Protocol servers, such as a GitHub server, a docs lookup, or an issue
tracker, and every agent in that workspace can use their tools on every executor. Ostra is the only MCP client
involved. It connects to each server itself, signs in to it, lists its tools, and then offers those tools to
native runs directly and to harness runs through its own MCP server.

This page explains why it works that way, how a connection is made and kept, how OAuth sign-in works, and how
MCP tool calls pass the same policy as every other tool call. The client lives in the `ostra-mcp` crate. The
per-workspace gateway that owns the connections is `crates/ostra-server/src/mcp.rs`.

## Why Ostra is the only client

Each harness CLI has its own way of registering MCP servers, with different config files, different flags, and
different limits. Antigravity only registers servers through a global `agy mcp add`, and Codex cancelled MCP
calls under its default approval policy on the version Ostra measured. If every harness registered workspace
servers itself, each would need its own configuration, and each would enforce permissions its own way.

Ostra avoids all of that by keeping the connection on its own side:

```text
                     ┌──────────── Ostra server ────────────┐
native agent loop ──▶│                                      │
                     │  policy check ─▶ MCP gateway ─▶ ─────┼──▶ github (HTTP, OAuth)
harness CLI ─────────┼▶ ostra mcp-stdio ─▶ policy check ─┘  │──▶ docs (stdio, local command)
 (Claude Code,       │                                      │──▶ linear (HTTP, OAuth)
  Codex, Grok, agy)  └──────────────────────────────────────┘
```

- One connection per workspace server, shared by every execution in the workspace.
- One policy and one allowlist cover all four harnesses and the native loop.
- A harness sees workspace tools next to Ostra's own tools (`report`, `memory`, the code tools, and the
  `submit_*` tools), over the one MCP registration shape that works on every harness: a stdio server
  (`ostra mcp-stdio --execution <id>`).

## Configuring a server

Servers are listed in `workspace.toml` under `mcp_servers`. A `command` starts a local server; a `url` reaches a
remote one.

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

The Settings screen edits the same list as a form. It shows each server's live state, the server's own name
and version, a sign-in button for remote servers, a reconnect button, and a switch per tool that edits
`disabled_tools`.

Server names are lowercase letters, digits, and dashes, at most 24 characters, and never `ostra`. The name is
part of every tool name, so it has to split cleanly from the tool.

### Secrets stay out of the file

`workspace.toml` can travel with a repository, so it should never hold a secret. Two mechanisms keep it clean:

- `env` and `headers` values may name a variable of the Ostra server's environment as `${VAR}`. An unset
  variable is a connection error that names it, with the fix: set it and restart the server.
- A value typed in Settings as plain text is stored encrypted in Ostra's registry. The file then holds the
  placeholder `${ostra_secret}`, so the value never reaches the browser or a folder file again.

Some variables are refused outright. A server's `env` and `headers` may not name a provider API key, a bridge
token, or an OAuth client secret of another server, and a local server's process does not inherit them from
Ostra's environment either. A hostile or careless `workspace.toml` therefore cannot forward your Anthropic key
to an MCP server.

### Nothing starts before you approve it

A `workspace.toml` can arrive with a clone or a `git pull`, and an `mcp_servers` entry is a command Ostra would
run. Rule A1 says the command-bearing parts of a folder file start only while the registry holds the user's
approval of that file's exact hash. For MCP servers that covers `command`, `env`, `url`, `headers`, `oauth`, and
`enabled`.

While the file waits, no server starts and the Settings screen shows exactly what would run (commands, env and
header names, never values) with an Approve button. The approval carries the hash the browser showed and is
refused if the file changed in between. The gateway connects from the approved copy of the file, not from
whatever is on disk:

```rust
// Rule A1: no server starts from a workspace file that waits for approval, and it
// starts from the approved copy.
let approved = crate::trust::approved_workspace_file(&self.registry, root)
    .ok_or_else(|| ConnError::Other(crate::trust::PENDING_MESSAGE.into()))?;
```

A save made in Ostra keeps an approved file approved, so your own edits never ask again.

## Transports

### Local servers over stdio

A `command` server starts in the workspace root, in its own process group, and is ended with its connection.
It is the workspace's own program, so it runs under the agent sandbox where the platform provides one
(bubblewrap on Linux, Seatbelt on macOS). The last 4 KiB of its standard error are kept to explain a crash.

### Remote servers over streamable HTTP

A `url` server is reached over the streamable HTTP transport, with JSON or server-sent-event answers and the
server's `Mcp-Session-Id`. Ostra speaks protocol versions 2024-11-05 through 2025-11-25. The older HTTP plus SSE
transport is not supported.

Redirects are followed only within the origin of the first request, and never from `https` to `http`, because
a header carrying a token must never reach another host.

## Connection lifecycle

The gateway keeps one connection per workspace and server name. A connection is keyed by a fingerprint of the
parts of its settings that matter (command, env, URL, headers, OAuth, and the sandbox mode), so changing any
of them starts a fresh connection on next use.

| Situation | What happens |
| --- | --- |
| An execution opens | Each server that serves this agent is connected, or its live connection is reused |
| The tool list is older than 30 seconds, or the server announced a change | The list is fetched again |
| A server failed | It is not retried for 20 seconds, because parallel executions would each start it again |
| A server cannot be reached when an execution opens | The execution gets one Activity line and runs without that server |
| A server exited during an execution | It is started once more on the next call |

Tools are fixed when an execution opens, before a harness starts, because a harness lists MCP tools only once.

## Signing in with OAuth

A remote server that answers 401 or 403 needs a sign-in. Its state in Settings becomes "needs sign-in" and the
error an agent sees tells the user where to sign in. Ostra follows the MCP authorization spec:

1. **Discovery.** Ostra reads the resource metadata URL from the `WWW-Authenticate` challenge, or tries the RFC
   9728 well-known paths. From there it reads the authorization server's RFC 8414 or OpenID metadata, and
   falls back to the 2025-03-26 default endpoints when a server publishes none.
2. **Client registration.** Ostra registers itself as a public client (RFC 7591), unless the entry sets
   `oauth.client_id`, optionally with `oauth.client_secret_env` naming the variable that holds the secret.
3. **Authorization.** `POST /api/workspaces/:ws/mcp/:name/login` returns the authorization URL, built with PKCE
   (S256), the `resource` parameter, and a single-use `state`. The browser opens it.
4. **Callback.** The authorization server redirects to `<browser origin>/mcp/oauth/callback`. This path is
   outside `/api` because a cross-site redirect does not carry Ostra's `SameSite=Strict` cookie. The `state`,
   valid for 10 minutes and usable once, authenticates the callback instead.
5. **Tokens.** The client and its tokens are stored encrypted in the registry, per workspace and server.

After that, tokens refresh on their own. An expired token is refreshed before use. A token the server refuses
is refreshed once. Refreshes are serialized per server, because a refresh token may be single-use and two
parallel refreshes would lock the user out. A failed refresh clears the tokens and asks for a sign-in again.
Sign out (`POST .../mcp/:name/logout`) forgets them.

Every endpoint URL a server's metadata names is checked before Ostra fetches it or sends the browser to it. It
must be `https`, or plain `http` on a loopback host or on the server's own origin. Anything else is refused:

```rust
/// An OAuth URL Ostra fetches or sends the browser to must be `https`, or plain `http` on a
/// loopback host or on the MCP server's own origin (which the user chose). Anything else, such
/// as a `javascript:` URL from a hostile server's metadata, is refused, because the browser would
/// run it on Ostra's origin.
pub fn check_endpoint(what: &str, url: &str, server_origin: Option<&str>) -> Result<(), String> {
```

A server that asks for a sign-in but publishes no OAuth metadata cannot be signed in to. Ostra says so and
suggests a token in a header instead.

## How tools reach an agent

When an execution opens, the gateway collects the tools of every enabled server whose `agents` list includes
the agent (an empty list means every agent), drops the ones named in `disabled_tools`, and hands the executor a
fixed list. Each tool keeps the server's own description and input schema.

### Names

A workspace tool is named `mcp__<server>__<tool>`, which is Claude Code's canonical form. That is the name in
the native loop, in the Activity view, and in permission rules. A harness sees it through Ostra's server as
`mcp__ostra__<server>__<tool>`.

Providers cap tool names at 64 characters, and some tool names contain characters providers refuse. A tool name
that is not `[A-Za-z0-9_-]`, or whose `<server>__<tool>` part passes 52 characters, is shortened and given a hash
of the original, so two tools never end up with the same name.

### Results

Results are text. Images and binary resources become a one-line note, structured content becomes JSON, and a
result is cut at 100 KiB, because a single large answer would fill the model's context.

## Policy

Every MCP call passes `ExecutionPolicy::check` like any other tool call. Two rules decide how.

**Rule M1: configured means allowed.** A workspace MCP tool is allowed unless a permission rule says otherwise,
because adding the server to the workspace is the decision to use it. Rules can narrow it:

```toml
[permissions]
deny = ["mcp__github__delete_repository"]
ask  = ["mcp__linear"]    # every tool of the linear server asks first
```

Plan mode is read-only, so in plan mode only tools the server marks with `readOnlyHint` are allowed. The rest
fall back to the mode's default, because Ostra cannot tell which of them write. A harness's own MCP tools (ones
it registered itself, not through Ostra) are not workspace tools and keep the mode default.

**Rule M2: one check, one log line.** A harness reaches a workspace tool only through Ostra's MCP server. So the
hook bridge, which sees the harness's tool call first, lets the call through unchecked, and the MCP handler
checks it, asks the user if needed, runs it, and logs it once. Checking at both points would ask the user
twice for the same call.

```rust
// Rule M2: a workspace MCP tool reaches the harness only through Ostra's MCP server,
// which checks and logs the call when it runs it, so the hook passes it unchecked.
if ostra_core::mcp::is_gateway_tool(&call.tool) && !r.inspect {
    return PolicyDecision::Allow { rule: None };
}
```

A read-only session (opening an ended harness run to scroll through it) gets no workspace tools at all.

## Status API

| Endpoint | What it does |
| --- | --- |
| `GET /api/workspaces/:ws/mcp` | Connects each server and reports its state, server info, sign-in state, and tools |
| `POST /api/workspaces/:ws/mcp/:name/refresh` | Reconnects and lists the tools again |
| `POST /api/workspaces/:ws/mcp/:name/login` | Returns the authorization URL for an OAuth sign-in |
| `POST /api/workspaces/:ws/mcp/:name/logout` | Forgets the server's OAuth tokens |
| `GET /mcp/oauth/callback` | The authorization server's redirect target |

## Where to read next

- [Executors](executors.md): how the native loop and the harness CLIs run, and where the MCP shim sits.
- [Tools](tools.md): Ostra's own tools, which travel over the same MCP server.
- [Code index](code-index.md): the code tools a harness reaches as `mcp__ostra__code_*`.
- [Settings and routing](settings-and-routing.md): where `mcp_servers` sits in `workspace.toml`.
- [Agent containment](../security/agent-containment.md): the policy layers and the sandbox a local server runs under.
- [Secrets and data](../security/secrets-and-data.md): how the registry stores OAuth tokens and saved header values.
