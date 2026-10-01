# Security policy

Ostra runs shell commands, edits files, and spends money on model calls for whoever is signed in. A bug in its
security boundaries can hand those powers to a web page, a repository, or an MCP server, so please report one
privately and give us time to fix it before you publish.

## Reporting a vulnerability

Report it to us at [lillya@lillya.dev](mailto:lillya@lillya.dev) with "Ostra security" in the subject. Do not open a public issue, pull request, or discussion for a vulnerability, because those are visible
to everyone at once.

Include what you can of the following:

- The Ostra commit you tested and your OS (Linux, macOS, Windows, WSL, or Docker).
- The sandbox mode and backend, from the setup screen's "Agent command sandbox" check or `GET /api/environment`.
- Which boundary you crossed (see [What counts as a vulnerability](#what-counts-as-a-vulnerability)).
- Steps to reproduce: a repository, a prompt, a web page, or a tool call sequence. A repository that makes an
  agent escape is the most useful form.
- What you expected to be refused and what happened instead.

Please do not include real API keys, tokens, or private source code in a report. Use decoys or scratch accounts.

We aim to acknowledge a report within 7 days and to agree on a disclosure date with you once the issue is
confirmed. We credit reporters in the advisory unless you ask us not to.

## Supported versions

Ostra has no tagged releases yet. Security fixes land on `master`, and only the latest `master` is supported.
Update and rebuild (`./install.sh`, or `cargo build --release` after `npm run build` in `web/`) to get them.

## What counts as a vulnerability

The [threat model](docs/security/threat-model.md) lists what Ostra protects and who from. A report is in scope
when one of these boundaries fails:

- **A web page drives the server.** A page on another site or another `localhost` port sends an accepted API
  request, opens the WebSocket, frames the console, or gets past the `Host`, `Origin`, and `Sec-Fetch-*` checks,
  for example through DNS rebinding. See [Server and browser](docs/security/server-and-browser.md).
- **Sign-in is bypassed.** A request to `/api` or `/ws` succeeds without a valid session cookie, a one-time
  sign-in token can be reused or guessed, or `/internal/*` accepts a non-local peer or a token from an execution
  that has ended.
- **Untrusted text runs as script.** Agent output, tool output, repository content, or MCP results execute
  script in the console or the docs site, load a remote image, or break out of the CSP. Terminal output that makes
  xterm.js send input on its own is in scope too.
- **An agent gets past layer 1 of the policy.** A tool call, native or through a harness adapter, writes outside
  its write scope, edits engine state, reads a secret path, edits `.git/` metadata, or touches Ostra's own files
  when a guard should refuse it. Permission rules, user answers, and YOLO must never lift these guards. See
  [Agent containment](docs/security/agent-containment.md).
- **A sandboxed command escapes on Linux or macOS.** A command reads a hidden path (SSH and GPG keys, cloud
  credentials, the keychain, browser profiles, Ostra's data directory), writes outside the workspace, edits a
  shell startup file or a protected `.git/config` or hook, signals a process outside its sandbox, reaches a host
  the egress proxy should refuse, reaches the LAN or the cloud metadata address, or turns its own sandbox off from
  a folder file. See [Sandboxing](docs/security/sandboxing.md).
- **A repository starts a program without approval.** A command named in `.ostra/workspace.toml` or
  `.ostra/project.toml` (an MCP server, a language server, a formatter) runs before you approved that exact
  content, or a folder file sets the permission mode or YOLO.
- **Credentials leak.** A sealed credential is readable without the master key, a provider key reaches the
  native executor's Bash tool, a harness receives another provider's key or Ostra's bridge variables, or a
  secret appears in the server log. See [Secrets and data](docs/security/secrets-and-data.md).
- **Spend limits are bypassed.** YOLO answers a budget gate, or a fan-out runs more executions than
  `limits.max_parallel_executions` allows.
- **Another local user reads Ostra's data.** The data directory is not `0700`, or its databases, key file, or
  terminal transcripts are not `0600`.

## What is not a vulnerability

These risks are accepted on purpose and recorded in the
[threat model](docs/security/threat-model.md#risks-ostra-accepts). Reports that only restate them will be closed,
though a report showing that one of them reaches further than the docs say is welcome.

- **Windows has no sandbox yet.** Every Windows execution runs unsandboxed, with your user's full rights, and
  Ostra says so at startup, on the setup screen, and in each execution. The Windows gaps are listed in
  [Sandboxing](docs/security/sandboxing.md#windows). Run Ostra in WSL 2 or Docker for the full sandbox.
- **Sandbox mode `off`, or `auto` without a backend.** You chose to run commands unsandboxed; the policy still
  applies, the kernel boundary does not.
- **Typing into a harness terminal is a shell.** Keystrokes go straight to the harness CLI, and a `!` shell
  escape does not pass the policy. The sign-in cookie is the boundary.
- **A harness sees the credential its own CLI signs in with**, such as `ANTHROPIC_API_KEY` for Claude Code.
- **The execution token is in the harness's environment.** It acts only as that execution, only while it runs.
- **Your own git reads what an agent's repository names.** Ostra's git runs with overrides that start no
  fsmonitor command or filter driver; a `git status` you run yourself in a submodule an agent added does not.
- **Allowed hosts receive what a command sends them.** Under `allowlist`, GitHub, package registries, and model
  APIs accept uploads. Hiding secrets is the defense; `none` is the only closed network.
- **Domain fronting inside TLS.** The proxy checks the TLS server name against the `CONNECT` host but does not
  decrypt the tunnel.
- **Local services on macOS.** With the loopback open, the macOS default, a sandboxed command reaches services on
  the Mac that you did not block. Ostra's own port and other executions' proxies stay closed.
- **Secrets in unusual places.** A token file in the repository or a key in `~/Documents` is readable unless
  `extra_hidden` names it.
- **Decoys are a tripwire, not a wall.** Their paths are documented, so an agent can avoid them.
- **A process running as your own user** can already read your files and keychain, and Ostra does not defend
  against it.
- **Plain HTTP on a non-loopback bind.** Ostra binds to `127.0.0.1` by default. If you bind elsewhere, put it
  behind an SSH tunnel or a TLS reverse proxy.
- **Model providers see your code.** Agents send what they read to the providers you configure.
- **Prompt injection that the boundaries contain.** A model that follows injected instructions is expected;
  Ostra does not rely on the model refusing. It is a vulnerability only when one of the boundaries above fails.
- **Antivirus reports on Windows.** Some behavior monitors flag Ostra's Windows sandbox probes. See the note in
  the [README](README.md).

## Running Ostra safely

- Keep the default bind to `127.0.0.1`. For remote access use an SSH tunnel or a TLS reverse proxy.
- Keep the sandbox mode `required` on Linux and macOS, and install `bubblewrap` on Linux.
- Use the egress `allowlist` or `none`, and add `extra_hidden` entries for secrets outside the default list.
- Set `limits.session_budget_usd`, and review before turning on YOLO for a repository you do not trust.
- Approve folder-file commands only after reading them, and review any submodule an agent added before you run
  git in it.
- Do not paste sign-in links into tools that preview URLs, because a preview spends the one-time token.

## How Ostra is tested

The defenses above have tests that run with the normal suite (commands in the [README](README.md)):

- Policy guards and permission rules: `crates/ostra-policy/tests/`.
- Sandbox profiles for both backends, built and tested on every OS: `crates/ostra-sandbox`.
- Server checks, sign-in, and the whole stack: `crates/ostra-server/tests/e2e.rs`.
- The browser security suite for the console and the docs site, including CSP and rendering of hostile text:
  `tests/browser` (`npm test`).

## Further reading

- [Threat model](docs/security/threat-model.md)
- [Agent containment](docs/security/agent-containment.md)
- [Sandboxing](docs/security/sandboxing.md)
- [Server and browser](docs/security/server-and-browser.md)
- [Secrets and data](docs/security/secrets-and-data.md)
