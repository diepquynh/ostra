# Storage

Ostra keeps its state in three kinds of SQLite database and some plain files. Ostra opens each database
through `crates/ostra-store`. This crate uses `rusqlite` with SQLite compiled in. Thus, Ostra needs no system
SQLite and no database server. This page tells where each part of the state is, why Ostra divides the state
in this way, and how Ostra protects the files.

## Where things live

```
~/.config/ostra/config.toml                  providers, tiers, global permissions   (OSTRA_CONFIG)
~/.local/share/ostra/                         the data dir, mode 0700                (OSTRA_DATA_DIR)
  registry.db                                 workspaces, sign-ins, push subscriptions, sealed credentials
  master.key                                  the key that seals credentials, when no OS keychain is reachable
  server.json                                 the running server's port, host, and pid
  server.log                                  the server log
  models-dev.json                             the cached model price catalog
  assets/                                     embedded prompts and skills, written out at startup
  tmp/                                        short-lived private files, such as an SSH key for one git clone
  hidden-artifacts/<workspace-id>/            workspace artifacts the user hid from every agent

<workspace>/.ostra/
  workspace.toml                              workspace settings
  workspace.db                                the event log and the tables built from it
  artifacts/                                  workspace artifacts: skills, docs, guidelines, sample data
  docs/<book>/                                documentation books: book.json and its Markdown; engine-written
  sessions/<session-id>/                      spec, plan, phases, reports, ledgers, uploads
    <project-key>/                            per-project reports of that session
    .state/                                   engine-owned; no agent may write here
      harness/<execution>/terminal.log        raw PTY bytes of a harness run, for replay

<project>/.ostra/                             committable, like a `.github/` folder
  INVENTORY.md                                what init learned about the project
  project.toml                                build, test, lint, and format commands
  memory/knowledge.sqlite3                    lessons that outlive any one session
<project>/.agents/skills/                     project skills, the cross-harness `.agents` layout
```

On macOS, the config and data dirs use the locations of the platform (the `dirs` crate selects them).
`OSTRA_CONFIG` and `OSTRA_DATA_DIR` move them to a different location. A scratch setup uses them to keep a test
run apart from your real workspaces. One file, `crates/ostra-core/src/paths.rs`, derives each path. Thus, each
reader and writer resolves a session and a project key to the same location.

The division follows ownership:

- The computer owns the registry.
- A workspace owns its sessions. One session can include more than one project. Thus, session state cannot be
  in one of the projects.
- A project owns the facts about the project that do not change with the workspace that imports it. These
  facts are its inventory, its commands, its skills, and its lessons.

You can commit the `.ostra/` folder of a project. Then the Ostra of a teammate uses it.

## The workspace database

`<workspace>/.ostra/workspace.db` holds two kinds of table. The event log is the source of truth. The tables
built from the log let the browser read quickly.

### The event log

```sql
CREATE TABLE events (
  session_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  type TEXT NOT NULL,
  payload TEXT NOT NULL,
  at TEXT NOT NULL,
  PRIMARY KEY (session_id, seq)
);
```

Each change to a session is one row. Examples are these changes:

- The session was created.
- The session entered a stage.
- An execution started or finished.
- A gate opened or got an answer.
- A judge made a decision.

The payload is the event, serialized as JSON. The state of a session is only the fold of its events in `seq`
order. If you load the rows and fold them, you get the session exactly as it was. The server uses this method
to recover after a restart. The tests use it to build a session from a written history.

Only one function appends events: `Inner::append` in `crates/ostra-engine/src/runner.rs`. It holds the state
lock of the session. Thus, the events of one session get numbers in the order of the fold. The `append_event`
function of the store takes an immediate write transaction. It reads the next `seq`, inserts the row, and
updates the `updated_at` of the session. Then the runner updates the tables below and applies the event to the
in-memory state. It also broadcasts the event to the browser and wakes the planner.

### The tables built from the log

| Table | Holds | Read by |
| --- | --- | --- |
| `sessions` | One row for each session: kind, request, title, category, status, lane, stage label, projects, YOLO, cost | The Sessions tree, the home page, search |
| `executions` | One row for each agent run: agent, stage, project, executor, model, spawn block, report path, status, usage, cost, the submit payload, the final text | Execution views, cost reports, resume |
| `gates` | Each question that the pipeline asked you, and your answer | The gate panels |
| `decisions` | Each judge decision, its reason, and whether you overrode it | Decision cards, overrides |
| `messages` | The transcript of each native execution, one row for each message | Resume of a native execution |
| `activity` | The live deltas of each execution: text, thinking, tool calls, policy decisions, usage | The Activity feed, after a reload |
| `tool_calls` | Each tool call with its input, the policy decision, the rule that decided, and the duration | Execution details |
| `projects` | Imported projects: key, path, init status, stack | Workspace detail |
| `meta` | The workspace id and the console layout (`ui_state`) | Startup, the console |

These tables are a cache of the log. They also hold some data that is too large or that changes too quickly
for events. The runner writes the activity deltas and the transcript messages during the stream of the
execution. It cuts tool call output to 8 KiB. If a materialized row and the fold do not agree, the fold is
correct, because the planner reads only the fold.

### Search

The ⌘K search box uses SQLite FTS5. `search_docs` holds one document for each session (its title and request)
and one for each artifact. `search_fts` indexes them with the `unicode61` tokenizer. The index removes
diacritics and has prefix indexes for 2 and 3 characters. Triggers keep the index the same as `sessions`. Thus,
search finds a renamed session by its new title immediately. A query becomes a prefix match on each word. For
example, the input `cance ord` searches for `"cance"* "ord"*`.

## The machine registry

`registry.db` in the data dir lists the items that this computer knows:

- `workspaces`: the id, the name, and the root folder of each registered workspace.
- `push_subscriptions`: the browsers and phones that asked for Web Push.
- `kv`: all other data, with a prefix for each kind of key. Sign-ins are under `auth:cookie:<hash>`. The key
  holds the hash of the cookie, never the cookie. Credentials are under `provider_credentials:`,
  `git_credential:`, `mcp_oauth:`, `mcp_secret:`, and `vapid_private`.

### Credentials are sealed

The store encrypts each value under a credential key with AES-256-GCM before it writes the value
(`crates/ostra-store/src/secrets.rs`). Each value gets a new nonce. The key of the row is the associated data.
Thus, if you copy a sealed value into a different row, the decryption fails. Sealed values start with
`enc:v1:`.

The master key seals the values (`crates/ostra-server/src/master_key.rs`). If the OS keychain answers in
10 seconds or less, Ostra keeps the key there. If not, Ostra keeps the key in `master.key`, an owner-only file
in the data dir. `OSTRA_MASTER_KEY_FILE` points to a different file, for example a Docker secret. At startup,
the server checks the key against a known sealed value. Then it seals each credential that an older build
stored as plaintext. The registry runs with `secure_delete` on. Thus, SQLite writes zeros over the old
plaintext page, and does not leave it in the file.

The key does not match if you moved the registry to a different computer, or if the keychain entry is gone. In
this case, Ostra logs the credential that it could not read. It asks you to enter the credential again in
Settings. It does not guess, and it does not use plaintext as a fallback.

## Project memory

Each project has a lesson store at `<project>/.ostra/memory/knowledge.sqlite3`
(`crates/ostra-store/src/memory.rs`). A lesson is a short, permanent fact that the pipeline learned about the
project. An example is "the integration tests need the `db` service running". Each lesson is in an area, such
as `build` or `auth::sessions`. The schema follows the memory of Ultracode:

- `(area, lesson)` is unique. Thus, the store keeps each lesson one time.
- Areas can have sub-scopes with `::`.
- Recall ranks lessons by FTS5 bm25 and returns eight lessons by default.
- The number of lessons has no limit, and lessons never expire. To forget a lesson, delete it from the Memory
  screen or the API.

The engine owns the file. Agents record and recall lessons through tools. The policy refuses each direct write
to the database. The store opens a new connection for each call. Thus, parallel executions wait on the file
lock of SQLite and do not share one handle.

## Migrations

Each database has a list of SQL migrations in its source file. It records the number of completed migrations
in the `user_version` of SQLite. When the store opens a database, it runs each missing migration in its own
transaction. It increases `user_version` in that same transaction. Thus, if a crash occurs during an upgrade,
the database stays at the last complete version (`migrate` in `crates/ostra-store/src/sqlite.rs`). New
migrations go only at the end of the list.

The workspace database is at version 3. The three migrations are:

1. The base schema.
2. Session titles and execution summaries.
3. Full-text search.

The lesson store has one schema. The store creates it with `CREATE ... IF NOT EXISTS` at each open, because the
store keeps the lesson schema of Ultracode unchanged.

## How the store code is organized

Each table has a repository. A repository is a small struct over a borrowed connection. It holds the SQL of
its table and changes the rows into API types. The workspace repositories are in
`crates/ostra-store/src/workspace/`, with one file for each table. Examples are `sessions.rs`, `events.rs`,
`executions.rs`, and `gates.rs`. The registry repositories are in `crates/ostra-store/src/registry/`. The
lesson store keeps its one repository in `memory.rs`.

`WorkspaceDb` and `RegistryDb` own the connection. They are the only types that other crates see. A method
runs its repository calls one statement at a time (`Db::run`), or puts them in one immediate transaction
(`Db::transaction`). Thus, one location decides which writes commit together. For example, the append of an
event inserts the event and sets the `updated_at` of the session in the same transaction:

```rust
self.db.transaction(|c| {
    let stored = Events(c).append(session, event)?;
    Sessions(c).touch(session, stored.at)?;
    Ok(stored)
})
```

The store reads rows by column name. Some columns hold JSON, a serde enum name, or an RFC 3339 time. `RowExt`
in `crates/ostra-store/src/sqlite.rs` decodes these columns (`json`, `variant`, `time`, and their `_opt` forms).
Thus, a bad value fails as a JSON error or an invalid-value error, not as a SQLite error.

## How the files are protected

The event log holds tool output, and the registry holds credentials. Thus, both are private to your user:

- Ostra creates the data dir and the folder of each database with mode `0700`, or changes them to that mode.
- Ostra sets each database file and its SQLite `-wal` and `-shm` side files to `0600`. The reason is that
  SQLite creates side files with the process umask, and this umask frequently lets all users read the file.
- Terminal transcripts are `0600` in a `0700` folder.
- The data dir is on the hidden list of the sandbox. Thus, no agent command can read the registry, the master
  key file, or the server log.
- The policy refuses writes to each engine state dir (`.state/`), to the databases, and to the config and binary
  of Ostra. Permissions and YOLO cannot turn off these guards.

All databases run in WAL mode with a 5-second busy timeout. Thus, when the browser reads a session, it never
blocks the engine that writes the session.

## Where to go next

- [The event log](../internals/event-log.md): the events, and how the fold reads them.
- [Project memory](../internals/project-memory.md): how agents record and recall lessons.
- [Secrets and data](../security/secrets-and-data.md): the master key, how Ostra handles credentials, and the
  data that never goes into a log.
- [Agent containment](../security/agent-containment.md): the sandbox and the guards that prevent agent access
  to these files.
- [The server](server.md): how the server opens these databases at startup.
