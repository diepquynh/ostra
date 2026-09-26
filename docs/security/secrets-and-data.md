# Secrets and data

This page follows each kind of secret Ostra handles from where it comes from to where it is used, and lists
what Ostra writes to disk, what it sends to model providers, and what it keeps out of logs. Read it before you
add a credential, a file under the data directory, or a log line, because each of those can undo a protection
described here.

For the threats behind these rules, see the [threat model](threat-model.md). For the browser side (sign-in
tokens and cookies), see [Server and browser](server-and-browser.md).

## Where Ostra keeps things

| What | Where | Mode |
| --- | --- | --- |
| Data directory | `$OSTRA_DATA_DIR`, or `~/.local/share/ostra` on Linux and `~/Library/Application Support/ostra` on macOS | `0700` |
| Registry: workspaces, sign-ins, sealed credentials, approvals | `<data dir>/registry.db` and its `-wal`, `-shm`, `-journal` files | `0600` |
| Master key file, when the keychain is not used | `<data dir>/master.key`, or the file `OSTRA_MASTER_KEY_FILE` names | `0600` |
| Server log | `<data dir>/server.log` | inside the `0700` directory |
| Workspace database: the event log of every session | `<workspace>/.ostra/workspace.db` and its siblings | `0600`, in a `0700` directory |
| Project memory | `<project>/.ostra/memory/knowledge.sqlite3` | |
| Harness terminal transcripts | per execution | `0600`, in a `0700` directory |
| Per-execution harness config (holds the bridge token) | per execution | `0600` |

The directory mode is set on every start, not only on creation, because SQLite creates its `-wal` and `-shm`
files world-readable and an existing directory may have been created with a looser mode
([`crates/ostra-core/src/paths.rs`](../../crates/ostra-core/src/paths.rs)). [Storage](../architecture/storage.md)
describes what each database holds.

Global settings (`config.toml`) and folder settings (`.ostra/workspace.toml`, `.ostra/project.toml`) never hold
a secret value. They name environment variables or keychain services instead, so they are safe to commit.

## Provider credentials

Ostra looks up a provider key in this order and uses the first it finds:

1. **The environment.** `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `OPENAI_API_KEY`, and any variable a
   provider entry in `config.toml` names.
2. **What you saved in the browser.** Settings > Providers stores the key in the registry, sealed.
3. **The OS keychain**, under the service name a provider entry sets as `keychain_service`.

The key stays on the server. The API never returns it: the settings screen shows only whether a key is set and
where it came from (`env:NAME`, `saved`, or `keychain`). See
[`crates/ostra-server/src/credentials.rs`](../../crates/ostra-server/src/credentials.rs) and
[`crates/ostra-providers/src/lib.rs`](../../crates/ostra-providers/src/lib.rs).

Which credentials each provider path accepts, and which it refuses on the provider's terms, is in
[Provider usage](../providers/README.md).

## How saved credentials are sealed

Every credential Ostra stores in the registry is encrypted before it reaches SQLite. That covers provider keys
saved in the browser, git credentials (`git_credential:*`), MCP OAuth tokens and client secrets (`mcp_oauth:*`,
`mcp_secret:*`), and the Web Push signing key (`vapid_private`). The list lives in `SECRET_KEYS` in
[`crates/ostra-store/src/secrets.rs`](../../crates/ostra-store/src/secrets.rs).

- **AES-256-GCM** with a fresh 12-byte random nonce for every write.
- **The row's key is the associated data.** A sealed value copied into another row fails to decrypt, so someone
  who can write to the database cannot move your GitHub token into the row Ostra reads as an MCP secret.
- **Tampering is detected.** A changed byte fails authentication and the value reads as not set.
- **Old plaintext is sealed on start.** A registry written by an older Ostra is encrypted in place the first
  time the new one opens it.

### The master key

The 256-bit key that seals credentials is kept outside the database, so a copy of `registry.db` alone is not
enough to read them ([`crates/ostra-server/src/master_key.rs`](../../crates/ostra-server/src/master_key.rs)).

1. **`OSTRA_MASTER_KEY_FILE`**, when set, names the key file. Use this for a mounted Docker secret or any setup
   where you manage the key yourself.
2. **The OS keychain** otherwise: the macOS Keychain, or the Secret Service (GNOME Keyring, KWallet) on Linux
   (see [OS compatibility](../platforms/os-compatibility.md) for headless machines).
   The entry is per data directory, so two Ostra installs keep separate keys. Ostra writes the key, reads it
   back, and uses the keychain only if the value it read matches.
3. **`<data dir>/master.key`**, owner-only, when no keychain is reachable. Ostra tightens the mode of an
   existing file that is looser.

Ostra records which backend it used. If the keychain held the key before and is now locked or unreachable,
Ostra refuses to start and tells you to unlock it, rather than creating a new key and silently losing your
saved credentials. A keychain call can hang on an unlock prompt or a dead D-Bus, so each call runs on its own
thread and is abandoned after ten seconds.

A check value sealed under the key detects a key change. If the key no longer opens it, saved credentials read
as not set, a warning goes to the log, and you enter them again in settings.

## Git credentials

Credentials for cloning and pulling are stored sealed and used without ever touching the repository's config
or a shared temp directory ([`crates/ostra-server/src/git.rs`](../../crates/ostra-server/src/git.rs)).

- **HTTPS tokens** go to git through an inline credential helper that reads them from environment variables of
  that one git process, and answers only for the host the credential belongs to. Ostra also pins
  `http.proxy=` and `http.sslVerify=true` on the command line, so a repository's config cannot route the token
  through a proxy or past certificate checks.
- **SSH keys** are written to a new `0600` file in a private directory inside the data directory, which the
  sandbox hides from agents, and passed with `IdentitiesOnly=yes`. The directory is removed when the git
  command ends.
- **Output is redacted.** Any occurrence of the secret in git's output or progress lines is replaced with `***`
  before it is logged or sent to the browser.

## MCP credentials

Remote MCP servers sign in with OAuth 2.1 (PKCE, dynamic client registration, and the `resource` parameter), and
Ostra stores the resulting tokens sealed. An MCP server's `oauth.client_secret_env` must name a variable of that
server's own ([MCP](../internals/mcp.md)): Ostra refuses a name that belongs to a provider key or a bridge variable, because MCP servers
never receive those. Stdio MCP servers start with every configured credential variable removed from their
environment.

## Keeping secrets away from agents

A model that reads a secret can print it, send it to a provider, or write it into a file. Ostra keeps secrets
out of reach in three layers, so that a prompt-injected agent does not need to be trusted to leave them alone.
This section covers only the secret-related parts; [Agent containment](agent-containment.md) describes the
policy and the sandbox in full.

### The policy refuses secret paths

Every tool call passes the execution policy, and its first layer cannot be overridden by permissions or YOLO.
It refuses any read, search, or write that names:

- Ostra's data directory (registry, master key, server log) and the file `OSTRA_MASTER_KEY_FILE` names.
- Workspace databases, which hold every tool output of every session.
- `/proc/<pid>/...`, whose `environ` and `fd` entries expose the server's environment and open files.
- Any execution's harness directory, which holds that execution's bridge token.
- Credential stores under your home directory: `.ssh`, `.gnupg`, `.aws`, `.azure`, `.config/gcloud`, `.kube`,
  `.docker`, `.netrc`, `.git-credentials`, `gh` and `hub` tokens, Cargo and PyPI credentials, Vault and
  Terraform tokens, keyrings, `.password-store`, browser profiles, and on macOS the Keychains, Cookies, Mail,
  Messages, and TCC folders.

Grep and Glob skip these paths too, so a broad search does not list them. The full list is `HOME_CREDENTIALS`
in [`crates/ostra-core/src/paths.rs`](../../crates/ostra-core/src/paths.rs), and the checks are in
[`crates/ostra-policy/src/guards.rs`](../../crates/ostra-policy/src/guards.rs).

A harness CLI's own credential file (`~/.claude/.credentials.json` for Claude Code, `~/.codex/auth.json` for
Codex, and so on) is readable by that harness process only, because it needs the file to sign in. Agents still
cannot name it in a tool call.

### The sandbox hides them from commands

The policy judges a tool call by the paths it names, and a shell script can build a path at run time that the
policy never sees. So agent commands run in a sandbox the kernel enforces: bubblewrap on Linux and Seatbelt
(`sandbox-exec`) on macOS ([`crates/ostra-core/src/sandbox.rs`](../../crates/ostra-core/src/sandbox.rs)). What
each platform supports is in [OS compatibility](../platforms/os-compatibility.md).

- The host is read-only, and only the execution's own roots are writable.
- The data directory and the credential stores listed above are not mounted (Linux) or return `EPERM` (macOS).
- Container engine sockets are hidden, because anyone who reaches one can start a container that mounts the
  host.
- Shell startup files (`.bashrc`, `.profile`, `.zshrc`, and similar) stay read-only even where their folder is
  writable, so an agent cannot leave a program that runs in your next terminal.

### The environment is scrubbed

Commands started by the native Bash tool lose every configured credential variable (the provider defaults,
every variable a `providers.*` entry names, and each MCP server's client secret variable) and every `OSTRA_*`
variable. The sandbox also unsets `SSH_AUTH_SOCK`, `GPG_AGENT_INFO`, `DBUS_SESSION_BUS_ADDRESS`, `DOCKER_HOST`,
`DISPLAY`, `WAYLAND_DISPLAY`, and `XAUTHORITY`, so a command cannot borrow your SSH agent, your keyring over
D-Bus, or your display.

A harness CLI keeps the variables it signs in with (for Claude Code, `ANTHROPIC_API_KEY`,
`ANTHROPIC_AUTH_TOKEN`, and `ANTHROPIC_BASE_URL`; for Codex, `OPENAI_API_KEY` and `OPENAI_BASE_URL`) and loses
every other credential variable
([`crates/ostra-exec-harness/src/launch.rs`](../../crates/ostra-exec-harness/src/launch.rs)). The commands a
harness runs can see its own key; this is an accepted risk recorded in the threat model.

## Harness sign-in stays in the provider's CLI

When you run a stage on Claude Code, Codex, or another harness, the provider's own CLI signs in through the
provider's own flow: its own login command, its own browser page, its own credential file or keychain entry.
Ostra starts the CLI unmodified and never reads, copies, or forwards the consumer token that results. It checks
only whether a credential file exists, so the setup screen can say whether the CLI is signed in
([`crates/ostra-server/src/env.rs`](../../crates/ostra-server/src/env.rs)).

This is a policy as much as a technical choice. Ostra never uses a consumer subscription through a proxy, a
copied token, or a sign-in flow of its own, because the providers' terms do not allow it. The detail per
provider is in [Provider usage](../providers/README.md).

## What reaches model providers

Model providers receive everything an agent reads, because that is how the model works on it:

- The rendered agent prompt and the repository brief.
- The contents of files the agent reads, and the output of commands and searches it runs.
- MCP tool results and fetched web pages.
- Earlier reports from the same session that the stage takes as input.

Ostra does not send your provider keys, git credentials, MCP tokens, or the contents of secret paths, because
the policy and the sandbox keep agents from reading them. It does not filter source code for secrets that you
committed to the repository. Which model runs each stage, and so which provider sees the code, is set by routing
([Settings and routing](../internals/settings-and-routing.md)). If a repository contains a live key in a
tracked file, an agent that reads that
file sends it to the provider. Remove such keys before you point Ostra at the repository.

The provider's data retention terms apply to everything above. Choose providers and account settings with that
in mind.

## What reaches the logs

The server log (`<data dir>/server.log`, also printed to standard output) records requests refused and why,
executions starting and ending, and problems with settings, keychains, and connections. `OSTRA_LOG` sets the
level, `info` by default.

- **No keys, tokens, or cookies.** A refused request logs whether a cookie was sent and how many cookies there
  were, never their values. Sign-in tokens never reach the server in a URL, so they cannot appear in an access
  line.
- **No file bodies.** Tool outputs and file contents go to the workspace database, not to the log.
- **Git secrets are redacted** from git output before it is logged, as described above.

The workspace database, by contrast, holds every tool output of every session, which can include anything a
command printed. It is `0600` in a `0700` directory, and the policy refuses agent access to it. Treat a copy of
a workspace's `.ostra` directory with the same care as the repository itself.

## Push notifications

Web Push payloads are encrypted end to end to your browser with the RFC 8291 scheme, and signed with a VAPID key
that is sealed in the registry. The push service your browser uses (Google's, Mozilla's, or Apple's) sees only
the ciphertext and the timing. Push needs HTTPS or `localhost`.
