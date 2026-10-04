# Secrets and data

This page follows each type of secret that Ostra uses, from its source to the place where Ostra uses it. It also
lists what Ostra writes to disk, what it sends to model providers, and what it keeps out of logs. Read this page
before you add a credential, a file under the data directory, or a log line. Each of these changes can break a
protection that this page describes.

For the threats behind these rules, see the [threat model](threat-model.md). For the browser side (sign-in
tokens and cookies), see [Server and browser](server-and-browser.md).

## Where Ostra keeps things

| What | Where | Mode |
| --- | --- | --- |
| Data directory | `$OSTRA_DATA_DIR`, or `~/.local/share/ostra` on Linux and `~/Library/Application Support/ostra` on macOS | `0700` |
| Registry: workspaces, sign-ins, sealed credentials, approvals | `<data dir>/registry.db` and its `-wal`, `-shm`, `-journal` files | `0600` |
| Master key file, when Ostra does not use the keychain | `<data dir>/master.key`, or the file that `OSTRA_MASTER_KEY_FILE` names | `0600` |
| Server log | `<data dir>/server.log` | In the `0700` directory |
| Workspace database: the event log of each session | `<workspace>/.ostra/workspace.db` and its sibling files | `0600`, in a `0700` directory |
| Project memory | `<project>/.ostra/memory/knowledge.sqlite3` | |
| Harness terminal transcripts | One for each execution | `0600`, in a `0700` directory |
| Harness config of each execution (holds the bridge token) | One for each execution | `0600` |

Ostra sets the directory mode at each start, not only when it creates the directory. The reason is that SQLite
creates its `-wal` and `-shm` files world-readable. Also, an existing directory can have a less strict mode
([`crates/ostra-core/src/paths.rs`](../../crates/ostra-core/src/paths.rs)). [Storage](../architecture/storage.md)
describes the contents of each database.

Global settings (`config.toml`) and folder settings (`.ostra/workspace.toml`, `.ostra/project.toml`) never hold
a secret value. They name environment variables or keychain services. Thus you can safely commit them.

## Provider credentials

Ostra looks for a provider key in this order, and it uses the first key that it finds:

1. **The environment.** `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `OPENAI_API_KEY`, and each variable that a
   provider entry in `config.toml` names.
2. **The key that you saved in the browser.** Settings > Providers stores the key in the registry, sealed.
3. **The OS keychain**, under the service name that a provider entry sets as `keychain_service`.

The key stays on the server. The API never returns it. The settings screen shows only whether a key is set and
its source (`env:NAME`, `saved`, or `keychain`). See
[`crates/ostra-server/src/credentials.rs`](../../crates/ostra-server/src/credentials.rs) and
[`crates/ostra-providers/src/lib.rs`](../../crates/ostra-providers/src/lib.rs).

[Provider usage](../providers/README.md) tells which credentials each provider path accepts. It also tells which
credentials each path refuses because of the terms of the provider.

## How saved credentials are sealed

Ostra encrypts each credential that it stores in the registry before the credential gets to SQLite. This
includes these credentials:

- Provider keys that you saved in the browser.
- Git credentials (`git_credential:*`).
- MCP OAuth tokens and client secrets (`mcp_oauth:*`, `mcp_secret:*`).
- The Web Push signing key (`vapid_private`).

The full list is `SECRET_KEYS` in [`crates/ostra-store/src/secrets.rs`](../../crates/ostra-store/src/secrets.rs).

- **AES-256-GCM** with a new 12-byte random nonce for each write.
- **The key of the row is the associated data.** A sealed value that a person copies into another row does not
  decrypt. Thus a person who can write to the database cannot move your GitHub token into the row that Ostra
  reads as an MCP secret.
- **Ostra detects changes.** A changed byte fails authentication, and Ostra reads the value as not set.
- **Ostra seals old plaintext at start.** An older Ostra can write a registry with plaintext values. The new
  Ostra encrypts these values in place the first time that it opens the registry.

### The master key

Ostra keeps the 256-bit key that seals credentials outside the database. Thus a copy of `registry.db` alone is not
enough to read the credentials ([`crates/ostra-server/src/master_key.rs`](../../crates/ostra-server/src/master_key.rs)).

1. **`OSTRA_MASTER_KEY_FILE`**, when you set it, names the key file. Use it for a mounted Docker secret, or for
   each setup where you manage the key yourself.
2. **The OS keychain** in all other cases: the macOS Keychain, or the Secret Service (GNOME Keyring, KWallet) on
   Linux. For headless machines, see [OS compatibility](../platforms/os-compatibility.md). Each data directory has
   its own entry. Thus two Ostra installs keep separate keys. Ostra writes the key and reads it back. Ostra uses
   the keychain only if the value that it read is the same.
3. **`<data dir>/master.key`**, owner-only, when Ostra cannot reach a keychain. If an existing file has a less
   strict mode, Ostra makes the mode stricter.

Ostra records which backend it used. If the keychain held the key before and is now locked or unreachable, Ostra
does not start. It tells you to unlock the keychain. It does not create a new key, because a new key loses your
saved credentials without a warning. A keychain call can stop on an unlock prompt or on a dead D-Bus. Thus each
call runs on its own thread, and Ostra abandons it after ten seconds.

A check value sealed under the key detects a key change. If the key does not open the check value, these things
occur:

- Saved credentials read as not set.
- A warning goes to the log.
- You must enter the credentials again in settings.

## Git credentials

Ostra stores credentials for clone and pull operations sealed. It uses them without a write to the config of the
repository or to a shared temp directory
([`crates/ostra-server/src/git.rs`](../../crates/ostra-server/src/git.rs)).

- **HTTPS tokens** go to git through an inline credential helper. The helper reads them from environment
  variables of that one git process. It answers only for the host of the credential. Ostra also sets
  `http.proxy=` and `http.sslVerify=true` on the command line. Thus the config of a repository cannot send the
  token through a proxy or skip certificate checks.
- **SSH keys** go to a new `0600` file in a private directory in the data directory. The sandbox hides this
  directory from agents. Ostra gives the key to git with `IdentitiesOnly=yes`. Ostra removes the directory when
  the git command ends.
- **Ostra redacts output.** Ostra replaces each occurrence of the secret in the output or progress lines of git
  with `***`. It does this before it logs the output or sends it to the browser.

## MCP credentials

Remote MCP servers sign in with OAuth 2.1 (PKCE, dynamic client registration, and the `resource` parameter). Ostra
stores the tokens that result, sealed. The `oauth.client_secret_env` of an MCP server must name a variable that
belongs to that server ([MCP](../internals/mcp.md)). Ostra refuses a name that belongs to a provider key or a
bridge variable, because MCP servers never get those variables. Stdio MCP servers start without the configured
credential variables in their environment.

## Keeping secrets away from agents

A model that reads a secret can print it, send it to a provider, or write it into a file. Ostra keeps secrets
away from agents with three layers. Thus you do not need to trust a prompt-injected agent to leave secrets
alone. This section covers only the parts about secrets. [Agent containment](agent-containment.md) describes the
policy and the sandbox in full.

### The policy refuses secret paths

Each tool call goes through the execution policy. Permissions and YOLO cannot override the first layer of the
policy. The policy refuses each read, search, or write that names one of these items:

- The data directory of Ostra (registry, master key, server log) and the file that `OSTRA_MASTER_KEY_FILE`
  names.
- Workspace databases, because they hold each tool output of each session.
- `/proc/<pid>/...`, because its `environ` and `fd` entries show the environment and the open files of the
  server.
- The harness directory of each execution, because it holds the bridge token of that execution.
- Credential stores under your home directory:
  - On all systems: `.ssh`, `.gnupg`, `.aws`, `.azure`, `.config/gcloud`, `.kube`, `.docker`, `.netrc`,
    `.git-credentials`, `gh` and `hub` tokens, Cargo and PyPI credentials, Vault and Terraform tokens, keyrings,
    `.password-store`, and browser profiles.
  - On macOS: the Keychains, Cookies, Mail, Messages, and TCC folders.
  - On Windows: the DPAPI master keys (`AppData\Roaming\Microsoft\Protect`), the two Credential Manager stores,
    the Chrome, Edge, Brave, and Firefox profiles, the `gcloud` and GitHub CLI configs under `AppData\Roaming`,
    the `globalStorage` of VS Code, the PowerShell history, and the `_netrc` of curl. On Windows, the check
    ignores letter case, as NTFS does.

Grep and Glob also skip these paths. Thus a broad search does not list them. The full list is `HOME_CREDENTIALS`
in [`crates/ostra-core/src/paths.rs`](../../crates/ostra-core/src/paths.rs). The checks are in
[`crates/ostra-policy/src/guards.rs`](../../crates/ostra-policy/src/guards.rs).

Each harness CLI has its own credential file, for example `~/.claude/.credentials.json` for Claude Code and
`~/.codex/auth.json` for Codex. Only that harness process can read the file, because it needs the file to sign
in. Agents still cannot name the file in a tool call.

### The sandbox hides them from commands

The policy examines a tool call by the paths that it names. But a shell script can make a path at run time, and
the policy does not see that path. Thus agent commands run in a sandbox that the kernel enforces: bubblewrap on
Linux and Seatbelt (`sandbox-exec`) on macOS (the [`ostra-sandbox`](../../crates/ostra-sandbox/src/lib.rs)
crate). [OS compatibility](../platforms/os-compatibility.md) tells what each platform supports.

- The host is read-only. Only the roots of the execution are writable.
- On Linux, the sandbox does not mount the data directory and the credential stores above. On macOS, these paths
  return `EPERM`.
- The sandbox hides container engine sockets, because a person who reaches one can start a container that mounts
  the host.
- Shell startup files (`.bashrc`, `.profile`, `.zshrc`, and similar files) stay read-only, also where their
  folder is writable. Thus an agent cannot leave a program that runs in your next terminal.

### The environment is scrubbed

Commands that the native Bash tool starts do not get these variables:

- Each configured credential variable: the provider defaults, each variable that a `providers.*` entry names,
  and the client secret variable of each MCP server.
- Each `OSTRA_*` variable.

The sandbox also unsets `SSH_AUTH_SOCK`, `GPG_AGENT_INFO`, `DBUS_SESSION_BUS_ADDRESS`, `DOCKER_HOST`, `DISPLAY`,
`WAYLAND_DISPLAY`, and `XAUTHORITY`. Thus a command cannot use your SSH agent, your keyring over D-Bus, or your
display.

A harness CLI keeps the variables that it uses to sign in, and it does not get the other credential variables
([`crates/ostra-exec-harness/src/launch.rs`](../../crates/ostra-exec-harness/src/launch.rs)):

- For Claude Code: `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, and `ANTHROPIC_BASE_URL`.
- For Codex: `OPENAI_API_KEY` and `OPENAI_BASE_URL`.

The commands that a harness runs can see its own key. The threat model records this as an accepted risk.

## Harness sign-in stays in the provider's CLI

When you run a stage on Claude Code, Codex, or another harness, the CLI of the provider signs in through the
flow of the provider. It uses its own login command, its own browser page, and its own credential file or
keychain entry. Ostra starts the CLI without a change. Ostra never reads, copies, or forwards the consumer token
that results. It checks only whether a credential file exists. Thus the setup screen can show whether the CLI is
signed in ([`crates/ostra-server/src/env.rs`](../../crates/ostra-server/src/env.rs)).

This is a policy decision and also a technical decision. Ostra never uses a consumer subscription through a
proxy, a copied token, or its own sign-in flow, because the terms of the providers do not allow it.
[Provider usage](../providers/README.md) gives the details for each provider.

## What reaches model providers

Model providers get each thing that an agent reads, because the model needs it to do the work:

- The rendered agent prompt and the repository brief.
- The contents of files that the agent reads, and the output of commands and searches that it runs.
- MCP tool results and fetched web pages.
- Earlier reports from the same session that the stage uses as input.

Ostra does not send your provider keys, git credentials, MCP tokens, or the contents of secret paths. The reason
is that the policy and the sandbox prevent agent access to them. Ostra does not filter source code for secrets
that you committed to the repository. Routing sets which model runs each stage, and thus which provider sees the
code ([Settings and routing](../internals/settings-and-routing.md)). If a tracked file in a repository contains a
live key, an agent that reads that file sends the key to the provider. Remove such keys before you use Ostra on
the repository.

The data retention terms of the provider apply to all the items above. Think about these terms when you choose
providers and account settings.

## What reaches the logs

The server log (`<data dir>/server.log`, also printed to standard output) records these events:

- Refused requests, with the reason.
- The start and the end of each execution.
- Problems with settings, keychains, and connections.

`OSTRA_LOG` sets the level. The default is `info`.

- **No keys, tokens, or cookies.** For a refused request, the log records whether the request sent a cookie and
  the number of cookies. It never records their values. Sign-in tokens never get to the server in a URL. Thus
  they cannot show in an access line.
- **No file bodies.** Tool outputs and file contents go to the workspace database, not to the log.
- **Ostra redacts git secrets** from the git output before it logs it, as the section above describes.

But the workspace database holds each tool output of each session. Thus it can hold all the text that a command
printed. It is `0600` in a `0700` directory, and the policy refuses agent access to it. Protect a copy of the
`.ostra` directory of a workspace as carefully as the repository itself.

## Push notifications

Ostra encrypts Web Push payloads end to end to your browser with the RFC 8291 scheme. It signs them with a VAPID
key that it seals in the registry. The push service of your browser (from Google, Mozilla, or Apple) sees only
the ciphertext and the timing. Push needs HTTPS or `localhost`.
