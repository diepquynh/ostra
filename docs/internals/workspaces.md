# Workspaces

A workspace is the unit you create in Ostra. It is a folder that holds `.ostra/workspace.toml`, the workspace
database, and the files of every session run in it. It lists the projects it works on by absolute path and
never copies them. Everything else in Ostra happens inside one workspace: a session belongs to one workspace,
an execution runs under that workspace's settings and limits, and the console shows one workspace at a time.

This page covers what a workspace is on disk and in the running server, how one is created, opened, changed,
and deleted, and how workspaces are kept apart. The features that live inside a workspace have their own pages:
[the pipeline](pipeline.md) for sessions, [settings and routing](settings-and-routing.md) for what
`workspace.toml` configures, [project memory](project-memory.md), [the code index](code-index.md), and
[MCP servers](mcp.md).

## What a workspace is

Three things make up a workspace, and each has a different owner.

| Part | Where | What it holds |
| --- | --- | --- |
| The folder | `<workspace>/.ostra/` | `workspace.toml` (settings), `workspace.db` (the event log and the tables built from it), `sessions/` (each session's spec, plan, reports, and uploads), `uploads/` (files waiting for a session), `artifacts/` (the workspace artifacts every agent reads) |
| The registry row | `registry.db` in the data dir | The workspace's id, name, root folder, and creation time |
| The registry records | `registry.db`, keyed by the root folder | The permission mode, the YOLO default, the spend limits, the sandbox mode, command approvals, and sealed MCP header and env values |

The folder does not need to hold any code. A common layout keeps the workspace next to its projects:

```
/home/me/code/shop/             the workspace
  .ostra/workspace.toml
  .ostra/workspace.db
  .ostra/sessions/
/home/me/code/shop-backend/     a project, referenced by path
/home/me/code/shop-web/         another project
```

The registry is what makes a folder a workspace. A folder with a `workspace.toml` but no registry row is not
opened, and the registry row is what the console lists. The split exists because the folder can be changed by
anyone who can write to it (a `git pull`, a teammate, an agent), and the registry cannot. The settings that
could raise your budget or loosen your sandbox therefore live in the registry, not in the folder. The rules
that enforce this, A1 and A2, are explained in
[Why some settings stay out of the folder](settings-and-routing.md#why-some-settings-stay-out-of-the-folder).

A workspace id is `ws_` followed by a UUIDv7 (`crates/ostra-core/src/ids.rs`), so sorting ids sorts
workspaces by creation time. Ostra writes the id into the database's `meta` table each time it opens the
workspace.

The first time Ostra opens `workspace.db`, it writes `.ostra/.gitignore` if none exists, listing
`workspace.db*`, `sessions/`, and `uploads/`. The event log keeps every tool output, so a `git add -A` in a
workspace that is also a repository must not publish it. `workspace.toml` is not ignored, so a team can
commit it.

### Workspaces and projects

A project is a folder the workspace imports under a project key, a lowercase slug such as `backend`. The
workspace records the key and the absolute path in `[[projects]]` and nothing else about the project. What is
true about the project itself (its inventory, its commands, its skills, its lessons) lives in the project's
own `.ostra/` folder, so two workspaces that import the same folder share it.

One task can span several projects, which is why session state lives in the workspace and not in any one
project. Reports for one project go to `sessions/<session-id>/<project-key>/`, and cross-project artifacts
such as the spec and the plan go to `sessions/<session-id>/`.

## Creating a workspace

![The workspace list with two workspaces and one whose folder is missing](../images/console/workspaces.png)

`/` lists the registered workspaces. On a machine that has no workspace and has never finished setup, it opens
the setup guide instead. Otherwise **New workspace** opens the same steps in a dialog, minus the welcome page. The steps are the machine
check (provider keys and harness CLIs), the name and folder, projects, defaults, and review.

![The Name and folder step with a folder that does not exist yet](../images/console/new-workspace-name.png)

The folder can be one that does not exist yet; Ostra creates it when it creates the workspace. Until you type
a name, the name follows the folder you pick.

On Windows the picker takes `C:\...` and `C:/...` paths, and `~\` as well as `~/`. Windows has no single root, so
`/` lists the drives, `C:` opens the root of that drive, and going up from a drive root returns to the drive list.
Folders that Windows marks hidden or system, such as `$Recycle.Bin`, are left out, the way dot folders are
([`browse.rs`](../../crates/ostra-server/src/files/browse.rs)).

![The Review step with the settings summary and the workspace.toml preview](../images/console/new-workspace-review.png)

When you reach the review step, the dialog sends the whole request to `POST /api/workspaces/validate`, and **Create workspace** sends it
to `POST /api/workspaces`. Both run the same function, `setup::draft` in
`crates/ostra-server/src/setup.rs`, which builds the complete `WorkspaceSettings` from the request and checks
it as a whole without writing anything. It refuses:

- an empty or relative folder, a path that is a file, and a folder that is already a registered workspace,
- an empty name,
- a project folder that does not exist, that is already in the request or the folder's existing file, or that
  sits inside the workspace's own `.ostra/` folder,
- a permission mode or routing preset it does not know,
- everything `validate_workspace` refuses for a saved workspace: bad project keys, relative project paths,
  a route that does not resolve, a harness that is not installed, a provider with no key, and a permission
  rule that does not parse ([Validation at save time](settings-and-routing.md#validation-at-save-time)).

Each problem carries the path of the request field it is about, so the dialog can show it on the right step.
Because the check covers the whole request, a route that names a model no provider serves is caught before any
file exists, not in the middle of the first session.

When the request passes, `setup::create` does the work in this order:

1. Creates the folder and resolves it to its canonical path, so one folder cannot be registered twice under
   two spellings.
2. Writes `.ostra/workspace.toml`. The permission mode, YOLO, limits, and sandbox mode are removed from the
   file and written to the registry (Rule A2), and literal MCP header and env values move into the sealed
   registry, leaving a marker in the file. The dialog's preview shows `[yolo]` and `permissions.mode`, but
   the file on disk does not carry them.
3. Records the file's commands as approved, because you just wrote them yourself (Rule A1).
4. Adds the registry row with a new id.
5. Opens the workspace (next section) and marks the machine as onboarded.

![The finished checklist with Initialize and Open workspace](../images/console/new-workspace-created.png)

A workspace with no projects is valid. Repositories you asked to clone are cloned after the workspace exists,
because the clone endpoint belongs to a workspace. A project you add is not initialized yet; the dialog offers
to start its init session, and no pipeline task can target it until init has written its
`.ostra/INVENTORY.md`.

### Registering a folder again

If the folder already holds a `workspace.toml`, for example because the registry was lost or the folder came
from a colleague, the request builds on that file instead of the seeded defaults. Its routing, instructions,
projects, and MCP servers are kept. The request's name replaces the file's, and the request's projects are
added after the file's. Three things are not trusted:

- its permission mode, YOLO, limits, and sandbox mode are reset to the defaults, and only the request sets
  them, because a repository could otherwise register itself with the sandbox off,
- nothing in it that starts a program is approved, so its MCP servers, language servers, code providers, and
  allow rules wait until you approve them in Settings,
- a problem with one of its existing project entries is reported against the folder, not a request field.

## Opening a workspace

The server opens every registered workspace when it starts (`app::build` in
`crates/ostra-server/src/app.rs`). A workspace whose `workspace.toml` is missing is skipped with a warning in
the server log, and one that fails to open is logged as an error. Neither is deleted or unregistered. The list
shows both with **Folder missing** and zero projects, because Ostra did not open them, and they cannot be
selected. Restore the folder and restart the server to open it again.

Opening a workspace (`WorkspaceRt::open` in `crates/ostra-server/src/workspace.rs`, then `App::attach`) does
the following:

1. For a workspace registered before command approvals existed, approves its current files once and copies
   its mode, YOLO, limits, and sandbox mode from the file into the registry.
2. Moves any literal MCP header or env value still in `workspace.toml` into the sealed registry.
3. Opens `workspace.db`, applying any schema migration, and writes the workspace id into it.
4. Starts an engine for the workspace.
5. Copies `[[projects]]` into the database's `projects` table, with each project's init status.
6. Runs recovery: executions that were running are marked interrupted and every unfinished session's planner
   starts again ([The event log](event-log.md)).
7. Forwards the engine's notices to the browser hub, tagged with the workspace id.

Only startup and creation open a workspace. There is no endpoint that opens or closes one while the server
runs.

## The workspace in the running server

Each open workspace is a `WorkspaceRt`: its id, its root folder, its `WorkspaceDb`, its `Engine`, and the list
of clones and pulls in progress. The server keeps them in one map keyed by workspace id.

![The workspace overview with the New task form and the sessions table](../images/console/workspace.png)

In the console, everything under `/w/<workspace-id>/` belongs to one workspace. The overview shows the root
folder and project count, a banner when the settings have validation problems (an agent whose route does not
resolve cannot start), a banner for each project that is not initialized, the New task form, and the
sessions table. The workspace menu in the title bar switches workspaces and opens the workspace's pages:

![The workspace menu with the current workspace's pages, other workspaces, and New workspace](../images/console/workspace-switcher.png)

The console's layout (open tabs, the focused tab, the left dock tab, and whether the sidebar and the
quick-question dock are open) is stored per workspace in the database's `meta` table through
`GET` and `PATCH /api/workspaces/<id>/ui`, capped at 64 KiB of JSON, so another browser opens the workspace
the way you left it.

### Keyboard shortcuts

The console follows VS Code's keys: ⌘K opens the command palette, ⌘/ the quick-question dock, ⌘B toggles the
sidebar, ⇧⌘E the Files tab, and Ctrl+Tab and Ctrl+Shift+Tab step through the open tabs (Ctrl in place of ⌘
outside macOS). A browser keeps some combos for itself, such as Ctrl+Tab, Ctrl+W, and Ctrl+N, so the page
never sees them.

In Chromium browsers the palette offers **Lock keyboard shortcuts in fullscreen**. It puts the page in
fullscreen and calls `navigator.keyboard.lock()`, the Keyboard Lock API that vscode.dev uses, because the
browser honors the lock only in fullscreen. While it holds, key presses reach the console before the browser:
the console's shortcuts work, ⌘W (Ctrl+W) closes the focused console tab instead of the browser tab, and
Escape goes to the page, so a harness terminal receives it. Inside a terminal Ctrl+W stays with the shell,
where it deletes a word. The status bar shows **Shortcuts locked** for as long as the lock holds. Holding
Escape, leaving fullscreen, clicking that item, or running **Release keyboard shortcuts** from the palette
releases the lock. The code is `web/src/lib/keyboardLock.ts`; browsers without the API do not list the command.

### Settings are read from disk each time

Ostra keeps no copy of a workspace's settings in memory. It reads `workspace.toml` each time it needs it and
applies the registry overlay, in one of two ways:

- `WorkspaceRt::settings()` is what the Settings screen edits: the file with the registry's mode, YOLO,
  limits, and sandbox mode.
- `effective_settings()`, and the engine's `Services::workspace()`, are what programs run with: the same, and
  while the file waits for approval, no MCP server, language server, code provider, or allow rule from it,
  and no project outside the workspace folder.

An edit, whether from the Settings screen, by hand, or from a `git pull`, therefore applies to the next
execution without a restart. If the file fails to parse, both fall back to seeded defaults named `workspace`
with no projects. The first logs a warning to the server log each time it reads the broken file.

### Saving settings

`PATCH /api/workspaces/<id>` takes the whole `WorkspaceSettings`. The server validates it, and on any issue
answers 422 with every issue and writes nothing. Otherwise it writes the file, updates the registry records,
copies the project list into the database again, and renames the registry row if the name changed.
`POST /api/workspaces/<id>/validate` runs the same checks without saving, which is how the Settings screen
checks as you edit.

## Adding and removing projects

![The Add a project dialog with the folder picker, project key, and stack](../images/console/add-project.png)

`POST /api/workspaces/<id>/projects` imports a folder. The key must be a project key that is not already
used in this workspace, and the path must be absolute, must exist, must be a folder, must not already be
imported, and must not be inside the workspace's `.ostra/` folder. The path is stored in canonical form. Each
refusal names its field (`key`, `path`, or `stack`). `POST /api/workspaces/<id>/clone` clones a repository
into `<workspace>/<key>` or a chosen empty folder and then imports it the same way (HANDOVER 6.4).

`DELETE /api/workspaces/<id>/projects/<key>` removes the project from `[[projects]]` and from the database's
`projects` table. It deletes nothing on disk: the project folder, its `.ostra/` files, and the session folders
that mention it stay. Ostra refuses with a 409 while the project has live work, because removing it would
strand that work:

- a running, waiting, or paused session that may use the project. Once the Classify judge has scoped a
  session, that means its scope lists the project. Before that, the session may still pick any project it
  started with, so every project in its starting list counts.
- a running execution in the project,
- a clone or pull on it,
- a request that is starting work in the workspace at that moment.

The message names what to stop or wait for. The last case exists because checking and removing are two
steps. Each workspace has a work lock, and the requests that start work hold it shared while they do:
creating a session, initializing a project, reopening a harness run, asking a quick question, and claiming a
clone or pull. A removal takes it exclusively without waiting, and refuses if it cannot, so no work starts
between its check and its write. A session created after the removal reads the settings fresh and never
sees the project.

## Deleting a workspace

The General tab of Settings holds **Delete workspace** (see
[Settings and routing](settings-and-routing.md#why-some-settings-stay-out-of-the-folder) for the tab).
`DELETE /api/workspaces/<id>` first checks that the workspace is idle, while holding both the lock on the
map of open workspaces and the workspace's work lock exclusively. It refuses with 409 when:

- a session is running, waiting, or paused, because deleting would lose its state,
- an execution is still running,
- a clone or pull is in progress,
- a request is starting work at that moment, because it holds the work lock.

Before it releases the work lock, it marks the workspace deleted. A request that looked the workspace up
before the removal and is waiting on the work lock then sees the mark and gets a 404 instead of starting a
session in a workspace that no longer exists. Then it:

1. closes the workspace and removes its registry row, with the push subscriptions scoped to it,
2. forgets its registry records: the approval of `workspace.toml`'s commands, the mode, YOLO, limits, sandbox mode, sealed MCP values,
   and MCP sign-ins, so registering the folder again starts fresh,
3. drops its MCP connections and its entries in the Sessions tree and search,
4. deletes `workspace.toml`, `workspace.db`, and the database's `-wal` and `-shm` files.

Project folders, each project's `.ostra/` files, and the workspace's `.ostra/sessions/` folder stay on disk.
The session folders keep the specs, plans, and reports, but without the database the console no longer lists
those sessions.

## Workspace artifacts

Workspace artifacts are files you keep for every session of a workspace rather than for one task: a custom skill, a
design document, a guideline that applies to all projects, or a sample data set a test loads. They live in the
Artifacts tab of the left dock, next to Sessions, Files, and Git. The tab is the Files tab's own file tree pointed
at the artifacts folder, so it behaves the same way: folders open lazily, "Find an artifact" searches every path, a
row drags into a task or addition as a tag or onto another folder to move it there (onto empty space moves it to
the top), and a click opens the artifact in the file editor, with its pencil, save, and conflict check. Only the
actions differ. The header and the right-click menu add Upload files, Download, Hide from agents or Show to agents,
and Delete, and files dragged in from the computer upload into the folder they are dropped on.

![The Artifacts tab with the guides folder open and style.md in the file editor](../images/console/artifacts.png)

![The right-click menu on a hidden artifact](../images/console/artifacts-menu.png)

This works because the Files endpoints treat `_artifacts` as one more root. A tree listing, a file read, a
save, a new folder, and the file index for `_artifacts` read the visible folder, then the hidden one, so a
hidden artifact still appears in the tree, marked with a crossed-out eye, and opens like any other file. The
file editor refuses edits under `.ostra/`, because that is Ostra's own state, except in the artifacts folder,
which belongs to the user. An artifact has no git history, so its diff and "Changed by sessions" are empty.

### Where they are stored, and why hidden ones move

A visible artifact is a plain file in `<workspace>/.ostra/artifacts/`. The folder's `.gitignore` does not list
it, so a team that commits the workspace folder shares its artifacts the way it shares `workspace.toml`.

Hiding works on a folder or a file as one unit (Rule W2). Ostra records each hidden unit, the folder or file you
hid, in `<data dir>/hidden-artifacts/<workspace id>.units.json`, and a path is hidden when it or a folder above it
is a unit. A hidden folder therefore hides everything in it, including files you create, upload, or move into it
later: the new file lands hidden. A file inside a hidden folder cannot be shown on its own; its menu offers to show
the folder instead. Hiding a file hides only that file, not the folder that holds it.

Where a file is stored follows from the units. Everything within a unit lives under
`<data dir>/hidden-artifacts/<workspace id>/`, at the same relative path, and everything else in
`.ostra/artifacts/`. Every change keeps that true: hiding moves the unit's files into the hidden folder, showing
moves them back, and a move or an upload puts each file where its new path belongs. The data dir is already closed
to agents: the `secret-read` guard refuses every file tool and every path-like word of a shell command that points
into it, and the Bash sandbox mounts nothing there. A hidden artifact is therefore out of reach by any path, for
native and harness executors alike, from the moment the change returns, and no reader has to check a flag. The cost
is that hidden artifacts stay on this machine: they do not travel with the folder or a `git push`.

That protection against shell commands needs the sandbox. With the sandbox off, or on `auto` on a machine without
one, the file tools still refuse hidden artifacts, but a shell command that walks the disk, such as `find ~`, can
read them. The Artifacts tab then shows a warning with a link to the Sandbox setting, and the setting lists
everything else that stops working ([Agent containment](../security/agent-containment.md)).

### How agents find and read them

Agents read visible artifacts and never change them (Rule W1):

- **The brief lists them.** After the project's instruction files, the first message of every execution names
  the folder and up to 40 artifacts with their sizes, then how many more there are, which the agent finds with
  Glob. The agent reads the ones that bear on its task with Read, the same way it reads a project file.
- **Custom skills load by name.** `skills/<name>/SKILL.md` answers `Skill(name)` after the project's own
  `.agents/skills/`, so a skill every project should follow can be kept once for the workspace. Harness CLIs load
  it by the path the brief lists.
- **Harnesses may read the folder.** Claude Code and Antigravity get it with `--add-dir`, and the Bash sandbox
  mounts it read-only for every executor.
- **Nothing writes there.** The `workspace-artifacts` guard refuses any write, move, or delete under the folder,
  in every permission mode and under YOLO. A command that only reads an artifact, such as
  `python3 load.py .ostra/artifacts/data/orders.csv`, is allowed.

### Tagging an artifact

Type `@` in a new task, an addition, or an instruction field, and the list offers visible artifacts next to project
files. Dragging a row from the Artifacts tab onto the field does the same. A picked artifact becomes the tag
`@_artifacts/<path>` (Rule W3). `_artifacts` cannot collide with a project, because a project key starts with a
letter or digit. In a task or addition the tag works like a tagged project file: the engine checks that the
artifact exists and is visible, and the agents receive its absolute path in the request. In an instruction, Ostra
adds one line per tag under the instruction's text with the absolute path, and a tag of a project file
(`@web/docs/api.md`) resolves the same way.

![Typing @sample in a new task lists the orders sample artifact](../images/console/artifacts-tag.png)

Hidden artifacts are never offered. A request that tags one is refused, and saving settings with an instruction
that tags a hidden or missing artifact fails with an issue on that field, so a stale tag shows up at save time
instead of reaching an agent as a path that does not exist. A session that tagged an artifact before it was
hidden still names the old path, and that path no longer holds a file.

### Deleting and moving

An artifact can be deleted or moved only while no session in the workspace is running, waiting, stalled, or paused
and no execution is running (Rule W4), because an agent may be reading it at its current path, and both change that
path. Delete asks first. When a session is still live, the server refuses the delete or the move, and the tab shows
the reason under its header, naming the session to stop. Both take the workspace's work lock, the same lock that
removing a project takes, so a session cannot start between the check and the change. A folder moves with
everything in it, and hidden artifacts inside it stay hidden. A move does not rewrite tags, so an instruction that
tagged the old path fails validation at the next save and names the tag to fix. Hiding, editing, and uploading are
allowed at any time.

## How workspaces are kept apart

Each workspace has its own database, its own engine, and its own settings file. The engine holds the
workspace's live sessions, its running executions, and its execution slots, and it reads settings only from
its own workspace's `workspace.toml`.

- **Parallel executions.** `limits.max_parallel_executions` is counted per engine, so it caps one workspace.
  Two workspaces with the default of 3 can run 6 executions at once. The limit is read again each time a
  spawn waits for a slot, so lowering it applies to spawns already waiting
  ([Spend and limits](spend-and-limits.md)).
- **Settings and approvals.** The registry records are keyed by the canonical root folder, so one workspace's
  approvals, mode, and sealed values never apply to another.
- **Live updates.** Browser messages about a workspace go to the `workspace:<id>` channel, and session
  summaries also go to `home` for the workspace list ([The server](../architecture/server.md)).
- **Lookups by id.** A request that names a session, gate, execution, or decision without a workspace finds it
  by asking each open workspace's database, so ids never need a workspace prefix in URLs such as `/s/<id>`.

The machine-wide pieces are shared: the global config, the registry, the provider clients, the native
executor, the harness runtime, the Web Push notifier, and the MCP gateway, which keeps its connections apart
by workspace root. A project folder imported into two workspaces is one folder: both workspaces see the same
inventory, skills, and lessons, and the policy's write scope, not the workspace, decides what an agent may
change ([Agent containment](../security/agent-containment.md)).

`ostra stop <session-id>`, which runs with the server down, looks through every registered workspace's
database for the session, because the command line does not know which workspace holds it.

## Where to look in the code

| What | Where |
| --- | --- |
| Create request validation and creation | [`crates/ostra-server/src/setup.rs`](../../crates/ostra-server/src/setup.rs) |
| `WorkspaceRt`: open, settings, projects, busy check | [`crates/ostra-server/src/workspace.rs`](../../crates/ostra-server/src/workspace.rs) |
| Opening at startup, `attach`, `delete_workspace` | [`crates/ostra-server/src/app.rs`](../../crates/ostra-server/src/app.rs) |
| Registry overlay, approvals, and what the file keeps out | [`crates/ostra-server/src/trust.rs`](../../crates/ostra-server/src/trust.rs) |
| The `workspaces` table | [`crates/ostra-store/src/registry.rs`](../../crates/ostra-store/src/registry.rs) |
| `workspace.db` and its `.gitignore` | [`crates/ostra-store/src/workspace.rs`](../../crates/ostra-store/src/workspace.rs) |
| Every workspace path | [`crates/ostra-core/src/paths.rs`](../../crates/ostra-core/src/paths.rs) |
| `WorkspaceSettings`, `seeded`, `validate_workspace` | [`crates/ostra-core/src/config.rs`](../../crates/ostra-core/src/config.rs) |
| Execution slots | `acquire_slot` in [`crates/ostra-engine/src/runner.rs`](../../crates/ostra-engine/src/runner.rs) |
| Workspace artifacts: paths, tags, listing | [`crates/ostra-core/src/artifacts.rs`](../../crates/ostra-core/src/artifacts.rs) |
| Workspace artifacts: upload, save, hide, delete | [`crates/ostra-server/src/artifacts.rs`](../../crates/ostra-server/src/artifacts.rs) |
| The design brief | [HANDOVER section 6](../../HANDOVER.md#6-workspaces-and-projects) |
