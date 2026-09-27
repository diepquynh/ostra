# Troubleshooting

Each section here names a symptom, explains what causes it, and gives the fix. Most of them come from one of
three design choices: sign-in links work once, the server records itself in `server.json`, and a restart
resumes interrupted work.

## Read the log first

The server logs to its terminal and appends the same lines to `server.log` in the data folder:

| System | Log |
| --- | --- |
| Linux | `~/.local/share/ostra/server.log` |
| macOS | `~/Library/Application Support/ostra/server.log` |
| Any, with `OSTRA_DATA_DIR` set | `$OSTRA_DATA_DIR/server.log` |

The file keeps every run, so a problem seen in a browser on another machine can be read back later. Start
the server with `OSTRA_LOG=debug` for more detail, or a filter such as `OSTRA_LOG=info,ostra_engine=debug`.
Under the login service, Linux also logs to `journalctl --user -u ostra`, and macOS writes early start-up
failures to `service.err.log` in the data folder.

The data folder, and so the log, is off limits to agents, so an agent cannot read it back into a model's
context.

## Sign-in

### "This sign-in link was already used"

A sign-in URL carries a one-time token after `#token=`. The first request that presents it gets the cookie,
and the token is spent. The page says how many seconds ago it was used.

When you did not open it yourself, something else fetched it first. Clipboard managers that preview links,
chat apps that unfurl a pasted link, and link scanners in mail clients all do this. The token lives in the
URL fragment, which browsers do not send to a server, so only programs that load the page in a browser engine
spend it; many previewers do.

Run `ostra url` for a fresh link and open it directly, without passing it through a tool that previews
links. Under the login service, `./install.sh url` does the same.

### "This sign-in link expired"

Tokens last 15 minutes. Run `ostra url`.

### "This sign-in link was not issued by this server"

The token is not in this server's registry. This happens when `ostra url` ran against a different data
folder than the server's: another user, or a missing `OSTRA_DATA_DIR` for a scratch server. Run `ostra url`
as the user that runs the server, with the same `OSTRA_DATA_DIR` and `OSTRA_CONFIG`.

### "This host is not allowed"

Ostra accepts only the host names it knows in the `Host` header, which blocks DNS rebinding: a web page on
another domain cannot make your browser talk to Ostra under its own name. When you reach Ostra through a
reverse proxy or a name of your own, add that name with `--allow-host ostra.example.com` or
`allowed_hosts` under `[server]` in `config.toml`.

### The address changes to `ostra-….localhost`

Ostra redirects `127.0.0.1` to a private name for this install, because cookies are not scoped to a port and a
cookie set on `127.0.0.1` would reach every other local server. Every current browser resolves `*.localhost`
to the loopback address. If yours does not, set `use_ip_host = true` under `[server]`, knowing the cookie then
reaches every other port on the machine.

### Signed out unexpectedly

A sign-in lasts 30 days. `ostra sessions` lists the live ones, and `ostra sessions revoke <id>` or
`ostra signout` ends them; a running server refuses a revoked browser within 5 seconds. If you did not revoke
anything, check whether the data folder was replaced, for example by a new Docker volume.

## The server record

### What `server.json` is

At start the server writes `server.json` to the data folder with its port, its URL host, and its process id,
and removes it on a clean stop (Ctrl-C). Two commands read it:

- `ostra url` reads the port and host to build the link.
- `ostra stop` reads the process id to check that no server is running.

### `ostra url` says no server is running

There is no `server.json` in the data folder it looked at. Either the server is not running, or it runs with
a different `OSTRA_DATA_DIR`. Set the same variable for `ostra url`.

### `ostra stop` refuses: "The Ostra server is running"

`ostra stop` changes a workspace's event log directly, which is only safe while no server holds that log. It
refuses while the process id in `server.json` is alive. With the server up, press Stop on the session's board,
which sends `POST /api/sessions/<session id>/stop`. If the server is truly gone but `ostra stop` still refuses, the recorded
process id now belongs to another process. Check with `ps -p <pid>` that it is not Ostra, delete
`server.json`, and run `ostra stop` again.

### A crash leaves `server.json` behind

A killed or crashed server cannot remove the file. The next start overwrites it, and the process check treats
a dead process id as no server, so nothing needs cleaning up by hand.

## Restarts and recovery

### Restarting re-runs work you thought was finished

The event log is the source of truth. When the server starts, it folds every session's events and finds the
executions that were `running` when the last server stopped. It marks them `interrupted` and runs each one
again from its original spawn block. An implementer continues from its progress log, so it does not redo
finished steps, but it is still a new model call that costs money.

This is on purpose: a crash or a reboot should not lose a session. [The event log](../internals/event-log.md)
explains the replay. It surprises people who stop the server
to stop the work. Before you stop the server, either:

- **Stop** the sessions you are done with, from the board or with `ostra stop <session id>` after the server
  is down. A stopped session never resumes.
- **Pause** the sessions you want to keep. A paused session survives the restart paused and starts nothing
  until you press Continue, which then resumes each interrupted execution where it stopped instead of
  re-running it.

Session ids start with `s_` and appear in the console's URL and the Sessions tree.

### Leftover processes on macOS

On macOS nothing ends a sandboxed process when its parent dies. When no other server is running, a new
server kills the processes an earlier server's sandboxes left behind before recovery starts, and logs how
many it stopped.

## Executions refused

### Every execution fails with a sandbox message

The sandbox mode defaults to `required` on Linux and macOS (`auto` on Windows, which has no sandbox yet). On Linux,
Ostra needs bubblewrap and unprivileged user namespaces.
The message says which is missing:

- "Install bubblewrap (the `bwrap` command)": install it with `sudo apt install bubblewrap` or
  `sudo dnf install bubblewrap`.
- "`bwrap` is installed but cannot create a sandbox here": user namespaces are blocked. On Ubuntu 24.04 and
  later, AppArmor restricts them; `kernel.apparmor_restrict_unprivileged_userns=0` lifts it. In a container,
  Docker's default seccomp profile blocks them; run the container with a profile that allows them.

To run without the sandbox on purpose, choose sandbox mode off for the workspace, or set
`[sandbox] mode = "off"` in `config.toml`. [Install](install.md#the-sandbox-check) explains the modes,
[OS compatibility](../platforms/os-compatibility.md) what each system supports, and
[Agent containment](../security/agent-containment.md) what the sandbox protects.

### A harness execution fails to start

A harness execution runs a provider's own CLI, which signs in with its own login. When the CLI is missing or
not signed in, the session opens a harness failure gate instead of failing the phase. Sign in with the CLI's
own login in a terminal, then retry from the gate, or switch the agent to the
native executor in the workspace's routing.

Under the login service, a CLI installed after `./install.sh` may not be on the service's `PATH`, because the
service keeps the `PATH` of the shell that installed it. Run `./install.sh` again.

### A tool call waits for permission

The workspace's permission mode asks before edits and before commands no rule allows. The ask shows as a
permission gate on the board and above the execution's Activity tab, with Allow once, Deny, and the rule
"always in this workspace" would add. Some calls are never allowed, whatever the permissions say: writes
outside the project, reads of the data folder or credential files, and writes to engine state. The denial
names the rule and what to do instead. [Tools](../internals/tools.md) and
[Agent containment](../security/agent-containment.md) list these rules.

## Spend

### "The session reached its budget"

Each session has a budget, `limits.session_budget_usd` in the workspace settings, 25 USD by default. The
planner checks it before every new execution. Once the session has spent it, no new execution starts and a
budget gate opens. Running executions finish, so the final spend can pass the budget by what they cost.

The gate has two answers: raise the budget by an amount you type, or stop the session. YOLO never answers
it, because spending more is your decision. Set the budget to `0` to turn the limit off for a workspace.

Spend is priced from the models.dev catalog, which Ostra caches in the data folder and refreshes once a day.
The Cost screen breaks spend down per session, stage, agent, and executor. [Spend and limits](../internals/spend-and-limits.md)
covers how the budget and the slots work inside the engine.

### Executions wait in a queue

`limits.max_parallel_executions` (3 by default) caps how many executions run at once in a workspace, across
all its sessions. A fan-out stage, such as init scouts or parallel explores, starts the rest as slots free
up. Raise the limit in the workspace settings when your provider's rate limits allow it.

## Git

### Pull, switch branch, or commit is refused

Every git command that changes the checkout is refused while a running or waiting session has the project in
scope, because it would change files under an implementer. Pause or finish the session first.

### A clone asks for a password and fails

Ostra runs git with prompts turned off, so a missing credential fails instead of hanging. Save a token or an
SSH key for the host under Settings, Git. A URL with a password in it is refused, because git would keep the
password in `.git/config`.
