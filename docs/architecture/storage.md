# Storage

Ostra keeps its state in three kinds of SQLite database and a handful of plain files. Every database is
opened through `crates/ostra-store`, which uses `rusqlite` with SQLite compiled in, so no system SQLite and
no database server is needed. This page covers what lives where, why it is split that way, and how the
files are protected.

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

On macOS the config and data dirs follow the platform's own locations (the `dirs` crate picks them).
`OSTRA_CONFIG` and `OSTRA_DATA_DIR` move them anywhere, which is how a scratch setup keeps a test run away
from your real workspaces. Every path is derived in one file, `crates/ostra-core/src/paths.rs`, so every
reader and writer resolves a session and project key to the same place.

The split follows ownership. The machine owns the registry. A workspace owns its sessions, because one
session can span several projects, so session state cannot live inside any one of them. A project owns
what is true about the project regardless of which workspace imports it: its inventory, its commands, its
skills, and its lessons. You can commit a project's `.ostra/` folder and a teammate's Ostra picks it up.

## The workspace database

`<workspace>/.ostra/workspace.db` holds two kinds of table: the event log, which is the truth, and the
tables built from it, which exist so the browser can read quickly.

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

Every change to a session is one row: the session was created, a stage was entered, an execution started
or finished, a gate opened or was answered, a judge made a decision. The payload is the event serialized as
JSON. A session's state is the fold of its events in `seq` order, and nothing else. Load the rows, fold
them, and you have the session exactly as it was, which is how the server recovers after a restart and how
the tests build a session from a written history.

Appends happen in one place, `Inner::append` in `crates/ostra-engine/src/runner.rs`. It holds the session's
state lock, so events of one session are numbered in the order they are folded. The store's `append_event`
takes an immediate write transaction, reads the next `seq`, inserts the row, and bumps the session's
`updated_at`. Then the runner updates the tables below, applies the event to the in-memory state,
broadcasts it to the browser, and wakes the planner.

### The tables built from the log

| Table | Holds | Read by |
| --- | --- | --- |
| `sessions` | One row per session: kind, request, title, category, status, lane, stage label, projects, YOLO, cost | The Sessions tree, the home page, search |
| `executions` | One row per agent run: agent, stage, project, executor, model, spawn block, report path, status, usage, cost, the submit payload, the final text | Execution views, cost reports, resume |
| `gates` | Every question the pipeline asked you, and your answer | The gate panels |
| `decisions` | Every judge decision, its reason, and whether you overrode it | Decision cards, overrides |
| `messages` | The transcript of each native execution, message by message | Resume of a native execution |
| `activity` | Each execution's live deltas: text, thinking, tool calls, policy decisions, usage | The Activity feed, after a reload |
| `tool_calls` | Each tool call with its input, the policy decision, the rule that decided, and the duration | Execution details |
| `projects` | Imported projects: key, path, init status, stack | Workspace detail |
| `meta` | The workspace id and the console layout (`ui_state`) | Startup, the console |

These tables are a cache of the log with a few extras that are too large or too fast-moving to be events:
activity deltas and transcript messages are written as the execution streams, and tool call output is cut
to 8 KiB. If a materialized row and the fold ever disagreed, the fold wins, because the planner reads only
the fold.

### Search

The ⌘K search box is SQLite FTS5. `search_docs` holds one document per session (its title and request) and
one per artifact, and `search_fts` indexes them with the `unicode61` tokenizer, diacritics removed, and
prefix indexes for 2 and 3 characters. Triggers keep the index in step with `sessions`, so a renamed
session is found by its new title at once. A query becomes a prefix match on every word: typing
`cance ord` searches for `"cance"* "ord"*`.

## The machine registry

`registry.db` in the data dir lists what this machine knows about:

- `workspaces`: id, name, and root folder of each registered workspace.
- `push_subscriptions`: the browsers and phones that asked for Web Push.
- `kv`: everything else, keyed by prefix. Sign-ins live under `auth:cookie:<hash>` (the cookie's hash, never
  the cookie), and credentials live under `provider_credentials:`, `git_credential:`, `mcp_oauth:`,
  `mcp_secret:`, and `vapid_private`.

### Credentials are sealed

Every value under a credential key is encrypted with AES-256-GCM before it is written
(`crates/ostra-store/src/secrets.rs`). Each value gets a fresh nonce, and the row's key is the associated
data, so a sealed value copied into another row fails to decrypt. Sealed values start with `enc:v1:`.

The key that seals them is the master key (`crates/ostra-server/src/master_key.rs`). Ostra keeps it in the
OS keychain when one answers within 10 seconds, and otherwise in `master.key`, an owner-only file in the
data dir. `OSTRA_MASTER_KEY_FILE` points at another file, for example a Docker secret. At startup the server
checks the key against a known sealed value, then seals any credential that an older build stored as
plaintext. The registry runs with `secure_delete` on, so the old plaintext page is zeroed rather than left in
the file.

When the key does not match (you moved the registry to another machine, or the keychain entry is gone),
Ostra logs which credential it could not read and asks you to enter it again in Settings. It does not guess
and it does not fall back to plaintext.

## Project memory

Each project has a lesson store at `<project>/.ostra/memory/knowledge.sqlite3`
(`crates/ostra-store/src/memory.rs`). A lesson is a short, durable fact the pipeline learned about the
project, such as "the integration tests need the `db` service running", filed under an area like `build` or
`auth::sessions`. The schema follows Ultracode's memory:

- `(area, lesson)` is unique, so the same lesson is stored once.
- Areas can have sub-scopes with `::`.
- Recall ranks by FTS5 bm25 and returns eight lessons by default.
- Lessons are never capped and never expire. You forget one on purpose, from the Memory screen or the API.

The engine owns the file. Agents record and recall lessons through tools, and the policy refuses any direct
write to the database. The store opens a fresh connection per call, so parallel executions wait on SQLite's
file lock instead of sharing one handle.

## Migrations

Each database carries a list of SQL migrations in its source file and records how many have run in
SQLite's `user_version`. On open, the store runs each missing migration in its own transaction and bumps
`user_version` in that same transaction, so a crash during an upgrade leaves the database at the last
complete version (`migrate` in `crates/ostra-store/src/sqlite.rs`). Migrations are only ever appended.
The workspace database is at version 3: the base schema, then session titles and execution summaries, then
full-text search.

The lesson store has a single schema, created with `CREATE ... IF NOT EXISTS` on every open, because it
keeps Ultracode's lesson schema unchanged.

## How the store code is organized

Each table has a repository: a small struct over a borrowed connection that holds that table's SQL and
turns its rows into API types. The workspace repositories live in `crates/ostra-store/src/workspace/`, one
file per table (`sessions.rs`, `events.rs`, `executions.rs`, `gates.rs`, and so on), and the registry's in
`crates/ostra-store/src/registry/`. The lesson store keeps its one repository in `memory.rs`.

`WorkspaceDb` and `RegistryDb` own the connection and are the only types other crates see. A method either
runs its repository calls one statement at a time (`Db::run`) or wraps them in one immediate transaction
(`Db::transaction`), so the choice of what commits together sits in one place. Appending an event, for
example, inserts the event and stamps the session's `updated_at` in the same transaction:

```rust
self.db.transaction(|c| {
    let stored = Events(c).append(session, event)?;
    Sessions(c).touch(session, stored.at)?;
    Ok(stored)
})
```

Rows are read by column name. Columns that hold JSON, a serde enum name, or an RFC 3339 time are decoded
through `RowExt` (`json`, `variant`, `time`, and their `_opt` forms) in `crates/ostra-store/src/sqlite.rs`,
so a bad value fails as a JSON or invalid-value error, not as a SQLite one.

## How the files are protected

The event log holds tool output, and the registry holds credentials, so both are private to your user:

- The data dir and every database's folder are created or tightened to mode `0700`.
- Every database file and its SQLite `-wal` and `-shm` side files are set to `0600`, because SQLite creates
  side files with the process umask, which is often world-readable.
- Terminal transcripts are `0600` inside a `0700` folder.
- The data dir is on the sandbox's hidden list, so no agent command can read the registry, the master key
  file, or the server log.
- The policy refuses writes to any engine state dir (`.state/`), to the databases, and to Ostra's config and
  binary. These guards cannot be turned off by permissions or YOLO.

All databases run in WAL mode with a 5-second busy timeout, so the browser reading a session never blocks the
engine writing it.

## Where to go next

- [The event log](../internals/event-log.md): the events themselves and how the fold reads them.
- [Project memory](../internals/project-memory.md): how agents record and recall lessons.
- [Secrets and data](../security/secrets-and-data.md): the master key, credential handling, and what never
  reaches a log.
- [Agent containment](../security/agent-containment.md): the sandbox and the guards that keep agents out of
  these files.
- [The server](server.md): how the server opens these databases at startup.
