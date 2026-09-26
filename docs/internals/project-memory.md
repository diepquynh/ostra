# Project memory

Every project in a workspace has a store of lessons: short, durable facts that one run learned and a later run
would otherwise pay to rediscover. "Refund requires an ownership check that the handler does not show." "The
`orders` tests need `TZ=UTC` or the date fixtures shift." "`cargo build` fails with E0599 on `Order::builder`
until the `derive` feature is on."

Agents record lessons as they work and recall them before they start. The engine also pushes lessons at an
agent when it is stuck on a build failure, and refuses to let it finish after a hard-won fix until it records
what fixed it. This page covers the store, the two memory tools, the build streak and lesson gate that tie
memory to failures, and what carries from one session to the next.

The store is `crates/ostra-store/src/memory.rs`. Its schema and ranking follow Ultracode's
`mcp/lib/memory.js`.

## The store

Lessons live in one SQLite file per project, `<project>/.ostra/memory/knowledge.sqlite3`. It sits in the project
folder, not in Ostra's data directory, so lessons belong to the code they describe and stay with the project
when it moves to another workspace.

Each lesson has four fields:

| Field | Meaning |
| --- | --- |
| `area` | The module it applies to, such as `orders` or `orders::Service` |
| `lesson` | One line: the fact, not the story of finding it |
| `source` | Who recorded it: the agent and execution by default, or `user` after an edit in the browser |
| `created_at` | When it was recorded or last confirmed |

The table is unique on `(area, lesson)`. Recording the same lesson again updates its source and time instead of
adding a copy, so a lesson that several runs confirm rises in recency instead of filling the store with
duplicates. A full-text index (SQLite FTS5) over `area` and `lesson` backs every search, and triggers keep it in
step with the table.

The store is never capped and never expires lessons on its own. A lesson leaves the store only when someone
removes it: the user in the Memory screen, or an exact `forget` of a lesson confirmed to be stale.

Each call opens its own connection with a 5-second busy timeout, so parallel executions in the same project
wait on SQLite's file lock instead of sharing a handle.

## Recording a lesson

Agents that learn about the code (explore, implementer, write-test) have the `Memory` tool:

```json
{"area": "orders::Service", "lesson": "cancel() must check ownership; the route guard does not cover it"}
```

The tool description sets the bar for what belongs in memory: a constraint the code does not state, behavior
that contradicts a name, a version-specific API detail, an invariant that spans files, or how a build failure
was fixed. It rules out anything the code makes obvious, the current task, and a lesson recall already
returned. The explore prompt gives the reason: a store full of the obvious is worse than an empty one, because
it costs every future recall.

Agents cannot write the database any other way. The memory store is engine-owned state under the state
ownership guard, so a `Write`, an `Edit`, or a shell command that names it is refused. The browser's file editor
treats it as read-only for the same reason. The Memory tool is the one path in, which keeps every lesson in the
schema and deduplicated.

A Memory call in an implementer run, recorded after a failed check and its fix:

![An expanded Memory call recording a build lesson](../images/console/memory-call.png)

## Recalling lessons

`MemoryRecall` takes an optional `area`, an optional `query`, and a `limit` (8 by default, 50 at most). It fills
its answer from buckets, most relevant first, and skips any lesson it already listed:

1. Lessons in `area` or one of its sub-scopes (`orders` also matches `orders::Service`), ranked by BM25 against
   the query, or by recency when there is no query.
2. Lessons from any area that match the query, ranked by BM25, to fill the remaining slots.
3. With neither an area nor a query, the most recent lessons overall.

The query is split into words and matched with OR, each word quoted, so an error message pasted as a query
matches lessons that share any of its words and punctuation cannot break the search syntax.

The prompts tell each agent when to recall:

- **explore** recalls before it explores anything, with the area it is about to read and the topic as the
  query. It treats a recalled lesson as evidence to verify against current code, and cites the lessons it used
  in its research document (the `lessons` field, each with whether current code confirmed it).
- **implementer** and **write-test** recall with the diagnostic text as the query and the affected module as the
  area when they hit a failure, and say in their report which lesson they applied.
- **quick-answer**, the side panel's agent, can recall but not record.

## Lessons and build failures

Memory matters most when a build keeps failing. Ostra watches every build and test command an execution runs
(`crates/ostra-policy/src/build.rs`) and keeps a per-execution streak of consecutive failures, together with a
signature of why each one failed. The signature is the first line of output that looks like a diagnostic, with
paths, line numbers, and version numbers replaced, so the same error in two files gives the same signature.

| Consecutive failures | What happens |
| --- | --- |
| 2 | Up to 5 lessons matching the failure's signature are appended to the tool result |
| 3 and 4 | A warning asks the agent to state the root cause and what will change before the next attempt |
| 5 | Build and test commands are refused, and the agent is told to return `STUCK:` |

At two failures the agent does not have to think of recalling: the engine searches the store for it and puts
the result in front of it, as "Lessons recorded for failures like this one". This is where a lesson from last
week's session pays for itself.

At five failures the guard refuses the build. This Codex run then returned STUCK with its diagnostic:

![An ended Codex run with the build-streak denial and a STUCK diagnostic](../images/console/stuck-run.png)

### The lesson gate

The other half is making sure the fix gets recorded. A build that passes after 3 or more consecutive failures is
a verified recovery: something real was learned, and the next run will hit the same diagnostic. The agent is
told at once:

> Record what fixed this now, with the Memory tool: that passed after 3 consecutive failures on "...". Use area
> = the affected module and a one-line lesson naming the diagnostic and the fix ...

Until it does, the lesson gate refuses its `Report` call and any `submit_*` call with status `ok`. Two things
clear the gate:

- a successful `Memory` call, or
- a `Report` call with a `reason` stating that the fix was situational and teaches nothing reusable.

The gate is a Layer 1 guard. No permission rule, user answer, or YOLO setting overrides it. Its denial leads
with the correction, as every guard's does:

```rust
"Record the lesson with the Memory tool before submitting {what}: you recovered from {} consecutive build \
 failures on \"{}\" and have not recorded what fixed it. ..."
```

The streak and the gate are per execution. A new execution starts with a clean streak; what it inherits from
earlier ones is the lessons they recorded.

## The Memory screen

The console's Memory screen lists each project's lessons, newest first, with full-text search. The user can edit
any lesson (its source becomes `user`) or delete it. This is the place to remove a lesson that went stale after
a refactor, because agents only record and recall.

| Endpoint | What it does |
| --- | --- |
| `GET /api/workspaces/:ws/projects/:key/memory` | Lists lessons, optionally filtered by a search query |
| `PATCH /api/workspaces/:ws/projects/:key/memory` | Edits one lesson's area and text |
| `DELETE /api/workspaces/:ws/projects/:key/memory` | Deletes one lesson |

![The Memory screen listing backend lessons with their area and the agent that recorded them](../images/console/memory.png)

Add a lesson opens a form for the area and the lesson text:

![The Add a lesson form with Area and Lesson fields](../images/console/memory-add.png)

## What carries over between sessions

A session is one request through the pipeline. Most of what it produces stays with it; a few things outlast it.

**Carried over, per project:**

- The lessons in the memory store.
- `.ostra/INVENTORY.md` and `.ostra/project.toml`: the stack, commands, module map, and review rules the
  initializer wrote and the user may edit. They feed the repo brief at the top of every execution's first
  message.
- Skills in `.agents/skills/`, including the convention and module-hub skills.
- The project's own instruction files (`CLAUDE.md`, `AGENTS.md`, `AGENT.md`), which go into every execution's
  first message after the repo brief.

**Carried over, per workspace:** settings, routing, permissions, custom instructions, and MCP servers, all read
fresh for each execution.

**Kept with the session only:** its event log, research, spec, plan, reports, review ledger, and each execution's
build streak. Session files live under the workspace's `.ostra/sessions/`, which Ostra ignores in git.

So a new session knows what the project is and what earlier runs learned about it, but not what an earlier
session was trying to do. If a later session needs that, the spec or plan has to be brought in as input.

## Where to read next

- [Agents](agents.md): which agents have the memory tools and how their prompts use them.
- [Tools](tools.md): the full tool set, including `Memory` and `MemoryRecall`.
- [Agent containment](../security/agent-containment.md): the Layer 1 guards, including state ownership and the
  lesson gate.
- [Storage](../architecture/storage.md): where every database and file Ostra keeps lives.
