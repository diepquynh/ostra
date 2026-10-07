# Threat model

Ostra is a server that runs shell commands, edits files, and spends money on model calls for the signed-in
user. Read this page first when you decide if a change to Ostra is safe. Each defense on the other security pages
answers one of the threats on this page.

For the relation between the parts, see the [architecture overview](../architecture/overview.md). This page
names the assets that Ostra protects and the attackers that it protects them from. It also gives the trust
boundaries and the page that describes each defense. It also lists the risks that Ostra accepts on purpose, so
that a reader does not think that they are gaps.

## What Ostra protects

| Asset | Where it lives | Why it matters |
| --- | --- | --- |
| The ability to run commands as you | The server process and its executions | A person who can control Ostra can run each command that your user account can run |
| Provider API keys and OAuth tokens | Environment, OS keychain, or the registry, sealed | They bill your account. An OAuth token also acts as you on a different service |
| Git and MCP credentials | The registry, sealed | They push to your repositories and call your connected services |
| Your source code | Workspace folders on disk | Agents read it and send parts of it to model providers |
| Session history | The workspace database: every prompt, tool call, and tool output | It holds file contents, command output, and all text that a tool printed |
| Your shell setup and later logins | `~/.bashrc`, `~/.profile`, and similar files | A program in these files runs the next time that you open a terminal |
| Your budget | `limits.session_budget_usd` and the executions that spend it | A loop of top-tier executions that does not stop costs real money |

## Who Ostra defends against

### A hostile web page

You use the same browser for the web and for Ostra. A page on a different site, or on a different port of
`localhost`, can try these attacks:

- Send requests to the port of Ostra.
- Open a WebSocket to it.
- Show it in a frame.
- Use DNS rebinding to make its own origin resolve to `127.0.0.1`.

If one of these attacks works, the page can control a server that runs commands as you.

Ostra uses these defenses against the hostile page:

- A strict `Host` allowlist.
- `Origin` and `Sec-Fetch-*` checks on each API request and on the WebSocket upgrade.
- A `SameSite=Strict` cookie that a script cannot read.
- A private `*.localhost` host name, so that other local ports do not get the cookie.
- No CORS headers.

See [Server and browser](server-and-browser.md).

### A prompt-injected repository

Ostra does not trust the code that it works on. A README, a code comment, an issue body that an agent reads, or a
test fixture can contain instructions for the model. A model that obeys them can try to do these actions:

- Read your credentials.
- Write outside the project.
- Install a program in your shell profile.
- Send data out through a Markdown image URL that the browser gets.

Ostra does not depend on a refusal from the model. Each tool call goes through the execution policy. A
permission rule, an answer from a user, or YOLO mode cannot override the first layer of the policy. This layer
contains state ownership, secret paths, and git metadata. When tool enforcement is on, it also contains write
scope, report path, and self-protection.

Below the policy, agent commands run in a kernel sandbox:

- The host is read-only.
- The agent cannot see the data directory of Ostra or your credential stores.
- Shell startup files stay read-only.

If one execution tries again and again to get secrets, the files of Ostra, `.git/`, or local network addresses,
Ostra pauses the session and asks you. This also occurs in YOLO mode. The browser shows agent text under a CSP
that allows no remote images and no inline scripts. See [Agent containment](agent-containment.md) for the policy
and the sandbox. See [Secrets and data](secrets-and-data.md) for the data that they keep out of reach. See
[Server and browser](server-and-browser.md) for the rendering rules.

A repository can also contain settings. `.ostra/workspace.toml` and `.ostra/project.toml` can name commands (MCP
servers, language servers, formatters). These files can arrive with a clone, a `git pull`, or an edit by an
agent. Ostra starts those programs only after you approve that exact content. Ostra never reads the permission
mode and YOLO from a folder file. The same approval covers these parts of the workspace, because each part decides
what runs in a session:

- The custom agents (`.ostra/agents/`).
- The workflows (`.ostra/workflows/`).
- The composite transform functions (`.ostra/transforms/`).
- The plugin programs (`[[plugins]]`).

See [`crates/ostra-workspace/src/trust.rs`](../../crates/ostra-workspace/src/trust.rs).

### A malicious MCP server

An MCP server that you connect can do these attacks:

- Return tool results that contain injected instructions.
- Show tool names and descriptions that are not true.
- Try to reach the endpoints of Ostra.

Ostra is the only MCP client. Harnesses reach MCP tools through the MCP shim of Ostra. Thus each MCP call, native
or harness, goes through the same policy and allowlist under its canonical name `mcp__<server>__<tool>`. Ostra
handles MCP output the same as all other tool output, so the browser shows it as untrusted text. The registry
keeps OAuth tokens and secrets for remote MCP servers sealed. A stdio MCP server is a program, so it starts only
after you approve its command. See [MCP](../internals/mcp.md) for the operation of the gateway.

### Another program or user on the same machine

A local process can connect to `127.0.0.1` the same as your browser. But it must have a sign-in cookie for
`/api` and `/ws`, and it cannot get one without a one-time token. The harness callback endpoints under
`/internal/*` accept only local peers. They also accept only a per-execution bearer token, and this token stops
when the execution ends.

The data directory of Ostra is `0700`. Its databases, the key file, and terminal transcripts are `0600`. Thus a
different user account on the machine cannot read them. Ostra does not defend against a process that runs as your
own user, because such a process can already read your files and your keychain.

### Someone on the network

By default, Ostra binds to `127.0.0.1`, so nothing on the network can reach it. If you bind to a different
interface, the sign-in token and the cookie still control each request. But plain HTTP is not encrypted. In that
case, use an SSH tunnel or a TLS reverse proxy. Ostra accepts at most five failed token exchanges per minute from
each non-loopback address. [Install](../start/install.md) describes binding and remote access.

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

1. **Browser to server.** Ostra trusts the browser only after a sign-in. Ostra trusts the page that the browser
   loads only because the CSP stops agent or repository text from running as script.
2. **Server to execution.** The server trusts nothing that an execution returns except its structured
   `submit_*` payload. The server validates this payload against a schema ([Agents](../internals/agents.md)).
   Each tool call goes through the policy before it runs ([Executors](../internals/executors.md)).
3. **Execution to kernel.** The sandbox covers the actions that the policy cannot see. An example is a path
   that a shell script makes at run time.
4. **Server to provider.** Model providers receive the data that agents read. Ostra does not filter source code
   before it sends it. Thus, use only providers that you trust with your code.
5. **Server to MCP server.** Ostra holds the credentials and the connection. The MCP server sees only the tool
   arguments that an agent sends to it.

## Which page covers each defense

| Threat | Defense | Page |
| --- | --- | --- |
| Cross-site requests, DNS rebinding, framing | Host allowlist, Origin and Sec-Fetch checks, CSP, `X-Frame-Options` | [Server and browser](server-and-browser.md) |
| Stolen or replayed sign-in link | One-time 256-bit token in the URL fragment, 15-minute expiry, recorded as used | [Server and browser](server-and-browser.md) |
| Script injection through agent or repository text | React rendering, URL filtering, CSP, browser security suite | [Server and browser](server-and-browser.md) |
| Terminal escape sequences | Server-side answers to terminal queries, muted xterm.js replies | [Server and browser](server-and-browser.md) |
| Credential theft from disk | AES-256-GCM sealing under a master key in the keychain or an owner-only file | [Secrets and data](secrets-and-data.md) |
| Prompt injection that causes writes outside scope | Layer 1 guards, kernel sandbox | [Agent containment](agent-containment.md) |
| An agent that tries again and again to get out | Containment signals pause the session after three from one execution. The signals include opens of decoy credential files (on macOS, on admin accounts only) | [Agent containment](agent-containment.md#containment-signals-pause-the-session) |
| Credential theft by an agent | Secret-path guard, sandbox, scrubbed environment for agent commands | [Agent containment](agent-containment.md), [Secrets and data](secrets-and-data.md) |
| Credentials in logs | Redaction of git secrets, no bodies or keys in the server log | [Secrets and data](secrets-and-data.md) |
| Harness sign-in abuse | The CLI of the provider does the sign-in. Ostra never handles the consumer token | [Secrets and data](secrets-and-data.md), [Provider usage](../providers/README.md) |
| Unapproved programs from a repository | Content-hash approval of folder-file commands | [`trust.rs`](../../crates/ostra-workspace/src/trust.rs) |
| Runaway spend | Parallel execution slots, session budget gate that YOLO never answers | [Spend and limits](../internals/spend-and-limits.md) |

## Risks Ostra accepts

HANDOVER section 15 records these risks. This page repeats them so that a reviewer does not report them as bugs.

- **Typing into a harness terminal is a shell for the signed-in user.** Keystrokes go directly to the harness
  CLI. A command that the harness runs outside a tool call does not go through the policy of Ostra. An example
  is a `!` shell escape. The sign-in cookie is the boundary.
- **A harness sees the credentials its own CLI signs in with.** Harness sign-in uses the environment. Thus a
  harness keeps the variables that its CLI reads, and the commands that it runs can see them. Examples are
  `ANTHROPIC_API_KEY` for Claude Code and `OPENAI_API_KEY` for Codex. Ostra removes all other credential
  variables before launch, which includes the keys of other providers and the bridge variables of Ostra
  ([`crates/ostra-exec-harness/src/launch.rs`](../../crates/ostra-exec-harness/src/launch.rs)). Commands that
  the Bash tool of the native executor runs get no provider keys.
- **The execution token is in the harness's environment.** The token can act only as that one execution, and
  only when the execution runs.
- **Your own git reads what an agent's repository names.** The sandbox stops an agent from going outside its
  limits. It does not guard the git that you run yourself. An agent can do these steps:
  1. Create a repository in the workspace.
  2. Give its `.git/config` an fsmonitor command or a filter driver.
  3. Stage it as a submodule of your repository.

  Each git command that Ostra runs on the host passes config overrides that start neither of the two (see
  [Git stays usable and closed](sandboxing.md#git-stays-usable-and-closed)). But a `git status` that you run in
  a terminal or a desktop app enters that submodule and reads its config. Git does this for every repository
  that you clone. Review a submodule that an agent added before you run git in its folder.
- **Model providers see your code.** You pay the providers for this service. Select providers and data
  retention terms for this fact.
