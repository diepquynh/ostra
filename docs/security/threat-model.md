# Threat model

Ostra is a server that runs shell commands, edits files, and spends money on model calls for whoever is signed
in. Read this page first when you judge whether a change to Ostra is safe, because every defense described in
the other security pages exists to answer one of the threats listed here.

For how the pieces fit together, see the [architecture overview](../architecture/overview.md). This page names
what Ostra protects, who it protects it from, where the trust boundaries sit, and which page
describes each defense. It also lists the risks Ostra accepts on purpose, so they are not mistaken for gaps.

## What Ostra protects

| Asset | Where it lives | Why it matters |
| --- | --- | --- |
| The ability to run commands as you | The server process and its executions | Anyone who can drive Ostra can run any command your user account can |
| Provider API keys and OAuth tokens | Environment, OS keychain, or the registry, sealed | They bill your account and, for OAuth, act as you on another service |
| Git and MCP credentials | The registry, sealed | They push to your repositories and call your connected services |
| Your source code | Workspace folders on disk | Agents read it and send parts of it to model providers |
| Session history | The workspace database: every prompt, tool call, and tool output | It holds file contents, command output, and anything a tool printed |
| Your shell setup and later logins | `~/.bashrc`, `~/.profile`, and similar files | A program left there runs the next time you open a terminal |
| Your budget | `limits.session_budget_usd` and the executions that spend it | A runaway loop of top-tier executions costs real money |

## Who Ostra defends against

### A hostile web page

You browse the web in the same browser you use for Ostra. A page on any site, or on another port of
`localhost`, can try to send requests to Ostra's port, open a WebSocket to it, frame it, or use DNS rebinding to
make its own origin resolve to `127.0.0.1`. If any of these worked, the page would drive a server that runs
commands as you.

Ostra answers with a strict `Host` allowlist, `Origin` and `Sec-Fetch-*` checks on every API request and on the
WebSocket upgrade, a `SameSite=Strict` cookie that a script cannot read, a private `*.localhost` host name so the
cookie is not shared with other local ports, and no CORS headers at all. See
[Server and browser](server-and-browser.md).

### A prompt-injected repository

The code Ostra works on is not trusted. A README, a code comment, an issue body pulled in by an agent, or a test
fixture can carry instructions aimed at the model. A model that follows them might try to read your credentials,
write outside the project, install a program in your shell profile, or leak data through a Markdown image URL
that the browser would fetch.

Ostra does not rely on the model refusing. Every tool call passes the execution policy, whose first layer
(write scope, state ownership, report path, secret paths, self-protection) cannot be switched off by a
permission rule, a user, or YOLO mode. Underneath the policy, agent commands run in a kernel sandbox where the
host is read-only, Ostra's data directory and your credential stores are not visible, and shell startup files
stay read-only. The browser renders agent text under a CSP that allows no remote images and no inline scripts.
See [Agent containment](agent-containment.md) for the policy and the sandbox,
[Secrets and data](secrets-and-data.md) for what they keep out of reach, and
[Server and browser](server-and-browser.md) for the rendering rules.

A repository can also carry settings. `.ostra/workspace.toml` and `.ostra/project.toml` can name commands (MCP
servers, language servers, formatters) and can arrive with a clone, a `git pull`, or an agent's edit. Ostra
starts those programs only after you approved that exact content, and the permission mode and YOLO are never
read from a folder file. See [`crates/ostra-server/src/trust.rs`](../../crates/ostra-server/src/trust.rs).

### A malicious MCP server

An MCP server you connect can return tool results with injected instructions, advertise misleading tool names
and descriptions, or try to reach Ostra's own endpoints. Ostra is the only MCP client: harnesses reach MCP tools
through Ostra's own MCP shim, so every MCP call, native or harness, passes the same policy and allowlist under
its canonical name `mcp__<server>__<tool>`. MCP output is treated like any other tool output, which means it is
rendered as untrusted text in the browser. OAuth tokens and secrets for remote MCP servers are sealed in the
registry. A stdio MCP server is a program, so it starts only after its command was approved. See
[MCP](../internals/mcp.md) for how the gateway works.

### Another program or user on the same machine

A local process can connect to `127.0.0.1` just as your browser can. It still needs a sign-in cookie for
`/api` and `/ws`, and it cannot get one without a one-time token. The harness callback endpoints under
`/internal/*` accept only local peers and only a per-execution bearer token that stops working when the
execution ends. Ostra's data directory is `0700`, and its databases, the key file, and terminal transcripts are
`0600`, so another user account on the machine cannot read them. Ostra does not defend against a process
running as your own user: such a process can already read your files and your keychain.

### Someone on the network

By default Ostra binds to `127.0.0.1`, so nothing on the network can reach it. When you bind to another
interface, the sign-in token and the cookie still gate every request, but plain HTTP is not encrypted. Use an SSH
tunnel or a TLS reverse proxy in that case. Failed token exchanges from non-loopback addresses are limited to five
per minute per address. [Install](../start/install.md) covers binding and remote access.

## Trust boundaries

```
 browser tab ──(Host, Origin, Sec-Fetch, cookie, CSP)──▶ ostra server ──▶ registry and workspace dbs
                                                            │
                                  ┌─────────────────────────┼──────────────────────────┐
                                  ▼                         ▼                          ▼
                          native executor           harness executor             MCP servers
                     (policy on each tool call)   (hook bridge on /internal,   (through Ostra only,
                                  │                per-execution token)          same policy)
                                  ▼                         ▼
                          sandboxed commands ◀──── same sandbox profile
                                  │
                                  ▼
                          model providers (receive prompts, code, and tool output)
```

1. **Browser to server.** The browser is trusted only after a sign-in, and the page it loads is trusted only
   because the CSP stops agent or repository text from running as script.
2. **Server to execution.** The server trusts nothing an execution returns except its structured `submit_*`
   payload, which is validated against a schema ([Agents](../internals/agents.md)). Every tool call crosses the
   policy before it runs ([Executors](../internals/executors.md)).
3. **Execution to kernel.** The sandbox covers what the policy cannot see, such as a path built inside a shell
   script at run time.
4. **Server to provider.** Model providers receive what agents read. Ostra does not filter source code before
   it is sent, so choose providers you are willing to show your code to.
5. **Server to MCP server.** Ostra holds the credentials and the connection. The MCP server sees only the tool
   arguments an agent sends it.

## Which page covers each defense

| Threat | Defense | Page |
| --- | --- | --- |
| Cross-site requests, DNS rebinding, framing | Host allowlist, Origin and Sec-Fetch checks, CSP, `X-Frame-Options` | [Server and browser](server-and-browser.md) |
| Stolen or replayed sign-in link | One-time 256-bit token in the URL fragment, 15-minute expiry, recorded as used | [Server and browser](server-and-browser.md) |
| Script injection through agent or repository text | React rendering, URL filtering, CSP, browser security suite | [Server and browser](server-and-browser.md) |
| Terminal escape sequences | Server-side answers to terminal queries, muted xterm.js replies | [Server and browser](server-and-browser.md) |
| Credential theft from disk | AES-256-GCM sealing under a master key in the keychain or an owner-only file | [Secrets and data](secrets-and-data.md) |
| Prompt injection turning into writes outside scope | Layer 1 guards, kernel sandbox | [Agent containment](agent-containment.md) |
| Credential theft by an agent | Secret-path guard, sandbox, scrubbed environment for agent commands | [Agent containment](agent-containment.md), [Secrets and data](secrets-and-data.md) |
| Credentials in logs | Redaction of git secrets, no bodies or keys in the server log | [Secrets and data](secrets-and-data.md) |
| Harness sign-in abuse | The provider's own CLI signs in; Ostra never handles the consumer token | [Secrets and data](secrets-and-data.md), [Provider usage](../providers/README.md) |
| Unapproved programs from a repository | Content-hash approval of folder-file commands | [`trust.rs`](../../crates/ostra-server/src/trust.rs) |
| Runaway spend | Parallel execution slots, session budget gate that YOLO never answers | [Spend and limits](../internals/spend-and-limits.md) |

## Risks Ostra accepts

These are recorded in HANDOVER section 15 and repeated here so a reviewer does not report them as bugs.

- **Typing into a harness terminal is a shell for the signed-in user.** Keystrokes go straight to the harness
  CLI, and a command the harness runs outside a tool call (a `!` shell escape, for example) does not pass
  Ostra's policy. The sign-in cookie is the boundary.
- **A harness sees the credentials its own CLI signs in with.** Harness sign-in depends on the environment, so
  a harness keeps the variables its CLI reads (`ANTHROPIC_API_KEY` for Claude Code, `OPENAI_API_KEY` for Codex,
  and so on), and those are visible to the commands it runs. Every other credential variable, including other
  providers' keys and Ostra's bridge variables, is removed before launch
  ([`crates/ostra-exec-harness/src/launch.rs`](../../crates/ostra-exec-harness/src/launch.rs)). Commands run by
  the native executor's Bash tool get no provider keys at all.
- **The execution token is in the harness's environment.** It can act only as that one execution, and only
  while the execution runs.
- **Model providers see your code.** That is the service you are paying them for. Pick providers and data
  retention terms accordingly.
