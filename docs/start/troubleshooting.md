# Troubleshooting

Each section here names a symptom, tells what causes it, and gives the fix. Most symptoms come from one of
three design choices:

- A sign-in link works one time only.
- The server records itself in `server.json`.
- A restart resumes interrupted work.

## Read the log first

The server writes its log to its terminal. It also appends the same lines to `server.log` in the data folder:

| System | Log |
| --- | --- |
| Linux | `~/.local/share/ostra/server.log` |
| macOS | `~/Library/Application Support/ostra/server.log` |
| Any, with `OSTRA_DATA_DIR` set | `$OSTRA_DATA_DIR/server.log` |

The file keeps every run. Thus you can read a problem again later, also when you saw it in a browser on a
different machine. For more detail, start the server with `OSTRA_LOG=debug`. You can also use a filter, for
example `OSTRA_LOG=info,ostra_engine=debug`. Under the login service, Linux also writes the log to
`journalctl --user -u ostra`. On macOS, the login service writes early start failures to `service.err.log` in
the data folder.

Agents cannot access the data folder or the log in it. Thus an agent cannot read the log back into the context of
a model.

## Sign-in

### "This sign-in link was already used"

A sign-in URL contains a one-time token after `#token=`. The first request that sends the token gets the
cookie, and then the token is spent. The page shows how many seconds ago a request used the token.

If you did not open the link yourself, a different program got it first. These programs do this:

- Clipboard managers that show a preview of links.
- Chat apps that show a preview of a pasted link.
- Link scanners in mail clients.

The token is in the URL fragment, and browsers do not send the fragment to a server. Thus only programs that
load the page in a browser engine spend the token. Many preview programs do this.

Run `ostra url` to get a new link. Open the link directly, and do not put it through a tool that shows a preview
of links. Under the login service, `./install.sh url` does the same. On Windows, use `.\install.ps1 url`.

### "This sign-in link expired"

A token is valid for 15 minutes. Run `ostra url`.

### "This sign-in link was not issued by this server"

The token is not in the registry of this server. This occurs when `ostra url` used a different data folder
than the server. For example, a different user ran it, or `OSTRA_DATA_DIR` was not set for a scratch server.
Run `ostra url` as the user that runs the server. Use the same `OSTRA_DATA_DIR` and `OSTRA_CONFIG`.

### "This host is not allowed"

Ostra accepts only the host names that it knows in the `Host` header. This blocks DNS rebinding. With DNS
rebinding, a web page on a different domain makes your browser send requests to Ostra under the name of that
domain. If you reach Ostra through a reverse proxy or through a name of your own, add that name. Use
`--allow-host ostra.example.com`, or set `allowed_hosts` under `[server]` in `config.toml`.

### The address changes to `ostra-….localhost`

Ostra redirects `127.0.0.1` to a private name for this installation. The reason is that cookies do not have a
port scope. A cookie that Ostra sets on `127.0.0.1` goes to every other local server. Every current browser
resolves `*.localhost` to the loopback address. If your browser does not, set `use_ip_host = true` under
`[server]`. Then the cookie goes to every other port on the machine.

### Signed out unexpectedly

A sign-in is valid for 30 days. `ostra sessions` lists the sign-ins that are valid now. To end them, use
`ostra sessions revoke <id>` or `ostra signout`. A running server refuses a revoked browser in 5 seconds or
less. If you did not revoke a sign-in, find out if a different data folder replaced the data folder. A new
Docker volume is an example.

## The server record

### What `server.json` is

When the server starts, it writes `server.json` to the data folder. The file contains the port, the URL host,
and the process id of the server. The server removes the file when it stops cleanly (Ctrl-C). Two commands
read the file:

- `ostra url` reads the port and the host to make the link.
- `ostra stop` reads the process id to make sure that no server runs.

### `ostra url` says no server is running

The data folder that `ostra url` looked in has no `server.json`. One of these is the cause:

- The server does not run.
- The server runs with a different `OSTRA_DATA_DIR`.

Set the same variable for `ostra url`.

### `ostra stop` refuses: "The Ostra server is running"

`ostra stop` changes the event log of a workspace directly. This is safe only when no server holds that log.
Thus `ostra stop` refuses when the process id in `server.json` belongs to a live process. If the server
runs, push Stop on the board of the session. The board sends `POST /api/sessions/<session id>/stop`.

If the server is stopped but `ostra stop` refuses, a different process now has the recorded process id. Do
these steps:

1. Run `ps -p <pid>` and make sure that the process is not Ostra.
2. Delete `server.json`.
3. Run `ostra stop` again.

### A crash leaves `server.json` behind

A server that was killed or that crashed cannot remove the file. The next start writes over the file. The
process check counts a dead process id as no server. Thus you do not have to remove the file.

## Restarts and recovery

### Restarting re-runs work you thought was finished

The event log is the source of truth. When the server starts, it folds the events of every session. It finds
the executions that were `running` when the last server stopped. It marks each of them `interrupted`, and runs
each again from its original spawn block. An implementer continues from its progress log, so it does not do
the finished steps again. But each run is a new model call that costs money.

Ostra does this on purpose, because a crash or a reboot must not lose a session.
[The event log](../internals/event-log.md) tells how the replay works. This behavior surprises people who stop
the server to stop the work.

Before you stop the server, do one of these for each session:

- **Stop** the sessions that you do not need. Stop them from the board, or with `ostra stop <session id>`
  after the server stops. A stopped session never resumes.
- **Pause** the sessions that you want to keep. A paused session is still paused after the restart. It starts
  nothing until you push Continue. Then it resumes each interrupted execution from where it stopped, and does
  not run it again from the start.

Session ids start with `s_`. They show in the URL of the console and in the Sessions tree.

### Leftover processes on macOS

On macOS, nothing ends a sandboxed process when its parent stops. When no other server runs, a new server
kills the processes that the sandboxes of an earlier server left. It does this before recovery starts, and it
logs how many processes it stopped.

## Executions refused

### Every execution fails with a sandbox message

The default sandbox mode is `required` on Linux and macOS. On Windows the default is `auto`, because Windows
has no sandbox yet. On Linux, Ostra needs bubblewrap and unprivileged user namespaces. The message tells which
one is missing:

- "Install bubblewrap (the `bwrap` command)": install it with `sudo apt install bubblewrap` or
  `sudo dnf install bubblewrap`.
- "`bwrap` is installed but cannot create a sandbox here": user namespaces are blocked. On Ubuntu 24.04 and
  later, AppArmor restricts them. Set `kernel.apparmor_restrict_unprivileged_userns=0` to remove the
  restriction. In a container, the default seccomp profile of Docker blocks them. Run the container with a
  profile that allows them.

To run without the sandbox on purpose, set the sandbox mode of the workspace to off. You can also set
`[sandbox] mode = "off"` in `config.toml`. [Install](install.md#the-sandbox-check) describes the modes.
[OS compatibility](../platforms/os-compatibility.md) tells what each system supports.
[Agent containment](../security/agent-containment.md) tells what the sandbox protects.

### A harness execution fails to start

A harness execution runs the CLI of a provider, and that CLI signs in with its own login. If the CLI is missing
or not signed in, the session opens a harness failure gate. The phase does not fail. To continue, do one of
these:

- Sign in with the login of the CLI in a terminal, then retry from the gate.
- Change the agent to the native executor in the routing of the workspace.

Under the login service, a CLI that you installed after `./install.sh` can be missing from the `PATH` of the
service. The reason is that the service keeps the `PATH` of the shell that installed it. Run `./install.sh`
again. On Windows, the task reads the saved user `PATH` at each start. Run `.\install.ps1 restart` after the
installer of the CLI adds the CLI to `PATH`.

### A tool call waits for permission

The permission mode of the workspace asks before edits and before commands that no rule allows. The ask shows
as a permission gate on the board and above the Activity tab of the execution. The gate has Allow once, Deny,
and the rule that "always in this workspace" adds.

Ostra never allows some calls, whatever the permissions are:

- Writes outside the project.
- Reads of the data folder or of credential files.
- Writes to engine state.

The denial names the rule and tells what to do in its place. [Tools](../internals/tools.md) and
[Agent containment](../security/agent-containment.md) list these rules.

## Spend

### "The session reached its budget"

Each session has a budget, `limits.session_budget_usd` in the workspace settings. The default is 25 USD. The
planner checks the budget before every new execution. When the session has spent the budget, no new execution
starts, and a budget gate opens. Executions that run at that time continue to the end. Thus the final spend can
be more than the budget by the cost of those executions.

The gate has two answers: raise the budget by an amount that you type, or stop the session. YOLO never answers
this gate, because the decision to spend more is yours. To remove the limit for a workspace, set the budget to
`0`.

Ostra calculates the price of spend from the models.dev catalog. Ostra caches the catalog in the data folder
and gets a new copy one time each day. The Cost screen shows the spend for each session, stage, agent, and
executor. [Spend and limits](../internals/spend-and-limits.md) tells how the budget and the slots work in the
engine.

### Executions wait in a queue

`limits.max_parallel_executions` sets the maximum number of executions that run at the same time in a
workspace, across all its sessions. The default is 3. A fan-out stage, for example init scouts or parallel
explores, starts the other executions when slots become free. Raise the limit in the workspace settings if the
rate limits of your provider allow it.

## Git

### Pull, switch branch, or commit is refused

Ostra refuses each git command that changes the checkout when a running or waiting session has the project in
scope. The reason is that the command changes files that an implementer works on. Pause the session or let it
finish first.

### A clone asks for a password and fails

Ostra runs git with prompts turned off. Thus a missing credential makes the command fail, and the command does
not wait. Save a token or an SSH key for the host under Settings, Git. Ostra refuses a URL that contains a
password, because git keeps the password in `.git/config`.
