# Ostra

Ostra runs the Ultracode engineering pipeline (research, spec, fact-check, plan, build, review, test, docs) as
a local server with a browser UI. Code drives the pipeline and holds every gate; models do the work inside a
stage and answer a small set of named judgment questions.

- `HANDOVER.md` is the design brief. It is the spec for behavior; read the section you are touching.
- The Ultracode plugin at `../ultracode` (written `UC/`) is the source of
  rules and measured facts. Read it for behavior; never copy its code.
- Priority: the Rust backend. The React UI in `web/` will be redesigned later with Claude Design, so do not
  polish it. Keep it compiling against API changes and nothing more.
- Sync every frontend UI change to the Claude Design project, because the redesign starts from that project
  and drifts otherwise. When a change touches `web/`, `design/`, or `site/`, finish by asking the user to run
  `/design-sync` for it and list the screens that changed. The target and the method (targeted edits to
  `ui_kits/console/*` and `templates/homepage/*`, never the converter) are in `.design-sync/NOTES.md`; add a
  dated line there for each sync.

## Crates and the direction of dependencies

```
ostra-core        ids, settings and route resolution, pipeline enums, submit schemas, the event log,
                  API DTOs (exported to TypeScript), the Executor and ExecutionHost traits
ostra-store       SQLite: workspace db (event log plus materialized tables), registry, project memory
ostra-sandbox     sandbox profiles, the bubblewrap and Seatbelt backends, the egress proxy, decoys, and the
                  per-OS layer (`sys/`, one `Os` impl per OS, picked in `sys/mod.rs` only)
ostra-policy      guards and permissions over canonical ToolCalls; bash parsing
ostra-tools       native tool implementations (Read, Write, Edit, Bash, Grep, Glob, Skill, WebFetch, ...)
ostra-providers   Anthropic and OpenAI streaming clients; ScriptedProvider for tests
ostra-agents      embedded assets/, prompt rendering per executor, typed spawn structs, the repo brief
ostra-engine      event-sourced session state, the pure planner, judges, the runner, the spawn factory
ostra-exec-native the native agent loop (providers + tools + policy)
ostra-exec-harness harness executors: PTY, per-harness adapters, hook bridge, MCP stdio shim
ostra-notify      Web Push without OpenSSL
ostra-mcp         MCP client for workspace MCP servers: stdio and streamable HTTP, OAuth
ostra-code        tokenizer, per-project code index (usages, imports, symbols), LSP client, code providers
ostra-workspace   workspaces: settings checks, projects, command approvals (trust), create/delete, WorkspaceRt
ostra-server      the `ostra` binary: axum, auth, REST, WebSocket, embedded web build, CLI
```

The browser code is one npm workspace, installed at the root (`npm ci`): `design/` (`@ostra/design`, tokens and
React components), `web/` (the console, embedded in the binary), and `site/` (the homepage and docs). `web/` and
`site/` import the design system from `@ostra/design`, never from each other.

`ostra-core` depends on nothing internal. `ostra-engine` knows no executor, provider, or server: it reaches
them through the `Services`, `SpawnFactory`, and `Executor` traits, and `ostra-server` wires the real ones in.
Keep it that way. A new capability the engine needs becomes a trait method, not a dependency. The same holds
for `ostra-workspace`: it reaches the server only through `WorkspaceHost`, which `Shared` implements.

## The patterns every change follows

**1. Every state change is an event.** A session's state is `SessionState::fold(events)`
(`crates/ostra-engine/src/state.rs`). Nothing mutates session state except `SessionState::apply`, and
nothing appends events except `Inner::append` in `runner.rs`, which stores, folds, broadcasts, and wakes the
driver in one place. The fold must stay a pure function of the log: if a fact from outside the log changes
how an event folds, record it in the event. Example: the project's auto-fixable rule IDs travel in the review
execution's `params` (`AUTO_FIXABLE_PARAM`) because the fold cannot read `project.toml`.

**2. The planner is pure.** `next_steps(&SessionState, &PlanCtx) -> Vec<Step>` in `plan.rs` decides what
happens next; the runner performs steps and appends what happened. Settings reach the planner only through
`PlanCtx`. Each `Step` has a `key()` so an in-flight step is never started twice. When you add behavior, add
it as fold state plus a planner rule, never as logic inside the runner.

**3. Rules are cited and tested.** Code that implements a HANDOVER or Ultracode rule cites its ID in a
one-line comment (`// Rule D3a: ...`). Every rule has a fixture in `tests/conformance/main.rs`: an event
history built with the `H` helper and the expected `Step::summary()` list. A new rule or a changed rule means
a new or changed fixture in the same change.

**4. Agents return structured data.** Every agent ends by calling `submit_<agent>`, whose schema is the struct
in `crates/ostra-core/src/submit.rs`. The engine reads only that payload, never a final message. Prompts live
in `assets/agents/<name>/prompt.md` and must describe the same fields. `validate_submit` guards the shape.

**5. Spawns are typed.** Each agent's `Label: value` spawn block is a struct in
`crates/ostra-agents/src/spawn.rs`; required parameters are non-`Option` fields. The engine's
`factory.rs` maps planner `SpawnInputs` onto them. The engine names every report path (`paths::report`).

**6. Every tool call passes the policy.** Native and harness executions alike call
`ExecutionPolicy::check` on a canonical `ToolCall` (Claude Code tool names and input shapes). Layer 1 guards
(write scope, state ownership, report path, lesson gate, build streak, self-protection) cannot be overridden
by permissions, users, or YOLO. Layer 2 is Claude Code's permission model. Harness adapters translate each
CLI's payload into the canonical form before the policy sees it.

**7. Settings are read fresh and validated at save.** Global config, workspace settings, and project profiles
are re-read per execution. A route that does not resolve is a validation error at save time
(`validate_workspace`), never a silent fallback at spawn time.

**8. Spend is bounded.** `limits.max_parallel_executions` caps concurrent executions per workspace (slots
in `runner.rs`), and `limits.session_budget_usd` turns spawns into a `BudgetReached` gate. YOLO never answers
a budget gate. Fan-out stages keep caps (`init::MAX_SCOUTS`, `init::MAX_DEFAULT_GENERATE`). Any new fan-out
needs a cap and must go through the slot limiter.

**9. Every implementation updates the docs.** A change that adds or alters behavior updates the pages in
`docs/` in the same change, so users and agents can understand Ostra's internals from the docs alone. Write
them as a deep dive into how Ostra works and why, not as code documentation; cite a file or excerpt code only
where it shows a behavior. Describe only what the code does now, and fix a page the change makes wrong. Edit
an existing page in `docs/` only. A new page also needs a `NAV` entry in `site/src/docs/pages.ts` (recipe
below).

## Recipes

**Add a gate kind.** Add the variant to `GatePayload` (`ostra-core/src/event.rs`) with `stage()` and
`kind_str()` arms; record it in the fold (`on_gate_opened`, `on_gate_answered`); open it from the planner;
decide its YOLO handling in `judge_input::yolo_plan` (fixed answer, judge, or `None` for gates YOLO must not
answer); accept its answer shape in `runner::validate_answer`; add a fixture; regenerate TypeScript (below).

**Add or change a judge.** Output struct and schema in `judge.rs`, input builder in `judge_input.rs`, prompt
in `assets/judges/<name>.md`, the planner step that asks, the fold that applies the decision, and whether it
can be overridden (`SessionState::can_override`).

**Change an agent.** Edit `assets/agents/<name>/prompt.md` and `agent.toml` (tier, effort per executor,
capabilities, timeout). If the return changes, change its submit struct and the fold that reads it. Prompts
follow the writing rules below and keep every rule ID.

**Add a native tool.** Implement it in `ostra-tools`, give it a definition modeled on Claude Code's own tool
description, map a `Capability` to it, and make sure `ostra-policy` classifies it (read, write, or other).

**Add a guard.** Port it into `ostra-policy/src/guards.rs` with a one-line citation of its Ultracode source and
a test in `crates/ostra-policy/tests/`. Guards deny with the correction first, because Grok clips reasons to
256 characters.

**Add an endpoint.** Handler in `ostra-server/src/api.rs`; everything under `/api` gets the Host, Origin, and
cookie checks from `guard`. `/internal/*` is for harness callbacks and accepts local peers only.

**Add a docs page.** Put the Markdown in the matching folder of `docs/` (`start/`, `platforms/`,
`internals/`, `architecture/`, `security/`, `providers/`), add an entry to `NAV` in `site/src/docs/pages.ts`,
and list it in `docs/README.md`; a section of README or HANDOVER is an entry with `section`. `site/src/docs/model.test.ts` fails when an entry
finds no content, so a renamed heading breaks the test instead of the page.

**Change an API type.** Types in `ostra-core` carry `#[ts(export)]`. Run `cargo test -p ostra-core` to
regenerate `web/src/api/gen/`, then `cd web && npm run typecheck` and fix only what breaks.

## Writing standard

Write every text that a person or a model reads in this project in Simplified Technical English (STE). STE is
the controlled English of the ASD-STE100 specification, written for aerospace maintenance manuals. It limits the
words, the verb forms, and the sentence length, so that each sentence has only one meaning. This section adapts
STE for software.

The standard covers the pages in `docs/`, `HANDOVER.md`, the README, prompts, judge prompts, UI copy, error
messages, code comments, commit messages, pull request descriptions, and your replies to the user. The
`documentation` and `system-architecture` prompts hold a copy of the same rules, so the books that Ostra writes
and the pages in `docs/` follow one standard. When you change a rule here, change both prompts and HANDOVER
section 20 in the same change.

Use STE because these readers must all get the same meaning from one text:

- A person who reads English as a second language.
- A person who reads quickly to find one fact.
- An agent that does exactly what the text says.

### Words

- Use one word for one meaning, and one meaning for one word. A reader who sees two words thinks that they name
  two things. If the text calls a thing a "job", do not call it a "task" in a different sentence.
- Do not use a project term with its general meaning in the same text. If `build` names a pipeline stage, write
  "compile" for the general action.
- Use technical names as the source spells them. Technical names are the names in the code and the domain:
  types, functions, fields, routes, tables, files, and business terms. Put a code name in backticks.
- You can use the verbs of computer processes, such as compile, parse, serialize, hash, cache, deploy, and
  commit.
- Do not make a verb from a name. Write "open a pull request", not "PR the change".
- Use a short, common word in place of a long or rare word. The table below gives the words to use.
- Use "can" for a possibility or an ability, "must" for a requirement, and "do not" for a prohibition. Do not
  use "may", "might", "should", or "would", because each has more than one meaning.
- Use "because" for a reason, "when" or "after" for a time, and "but" for a contrast. Do not use "since",
  "as", "while", or "once" to connect two clauses, because each has more than one meaning.
- Use a one-word verb in place of a phrasal verb when one exists. Write "start", not "start up".
- Do not use contractions, slang, idioms, metaphors, similes, or other figures of speech. Write the literal
  fact.
- Do not use a word that praises but does not inform, such as robust, seamless, powerful, or efficient. Do not
  use a superlative in place of a number. Give the number or the behavior.
- Write a number as digits with its unit: `30 seconds`, `512 KB`. Do not write "a few", "various", "etc.", or
  "and more". Give the full list or the count.
- Use American English spelling in text. Keep a code name as the source spells it.

| Do not write | Write |
| --- | --- |
| utilize, leverage | use |
| perform, carry out | do, run |
| ensure | make sure |
| prior to | before |
| subsequent to, following (as in "following the build") | after |
| in order to | to |
| commence, initiate, begin | start |
| obtain | get |
| modify, alter | change |
| assist, facilitate | help |
| indicate, demonstrate | show |
| sufficient | enough |
| additional | more, other |
| numerous | many, or the count |
| approximately | about |
| attempt | try |
| via | through, with |
| due to | because of |
| is able to, is capable of | can |
| in the event that | if |
| e.g., i.e. | for example, that is |

### Noun phrases

- Use "a", "an", or "the" before a noun in a sentence. Do not remove articles to make a sentence shorter.
  Headings, labels, and table cells can omit them.
- Do not put more than three nouns in a row. Use a preposition to break a longer group. A code name counts as
  one noun.
- Do not use an -ing word as a verb or to start a clause, because such a clause hides its actor. Write "when the
  client retries", not "when retrying". An -ing word that names a thing or a process, such as logging or a
  `pending` state, is a technical name.

### Verbs

- Use the active voice, because the reader must know which part of the system does the action. Write "the
  worker sends the email", not "the email is sent". In a description, you can use the passive voice when the
  actor is not known or not important.
- Use only these verb forms: the simple present, the simple past, the future with "will", the imperative, and
  the infinitive. Do not use forms such as "has sent", "is sending", or "will have run".
- Use a verb for an action, not a noun that you make from a verb. Write "the parser validates the input", not
  "the parser performs validation of the input".

### Sentences

- Write one topic in each sentence.
- Write at most 20 words in an instruction and at most 25 words in a description. Short sentences are easier to
  read and to translate. A code name, a path, a number, and a hyphenated word each count as one word.
- Do not remove words to make a sentence shorter. Keep the articles, "that", and the verb.
- Put a condition before the fact or the instruction that it controls, and end the condition with a comma: "If
  the token expires, the client requests a new one."
- Use words such as "then", "because", "if", "when", "after", and "but" to connect sentences about related
  topics.
- Use a vertical list for steps in a sequence and for two or more conditions. Also use one when a list makes a
  sentence longer than its limit. Introduce the list with a colon.
- Write the items of one list in the same form. If one item starts with a verb, start each item with a verb.

### Paragraphs

- Start each paragraph with its topic sentence: the main fact. Then give the details.
- Write one topic in each paragraph, and at most six sentences.

### Instructions

An instruction tells the reader to do something: a step, a command to run, or a rule for a change.

- Write an instruction in the imperative: "Run the migrations."
- Write one instruction in each sentence, unless the reader must do two actions at the same time.
- Write the steps in the order in which the reader does them.
- Make each instruction specific: "Set `timeout_seconds` to `600`", not "Increase the timeout".
- Use a note only to give information. Do not put an instruction in a note.
- Write the instruction first, then the reason, with "because" or in the next sentence. A reader who knows the
  reason can apply the rule to a case that the rule does not name.

### Cautions

Write a caution before an action that can lose data, expose a secret, stop a service, or spend money:

1. Start with a clear and simple command.
2. In the next sentence, give the risk.

For example: "Copy the `orders` table to a backup before you run the migration. The migration deletes the
`legacy_status` column, and the data in it is lost."

### Punctuation and format

- Do not use semicolons. Write two sentences.
- Do not use em dashes. Use a colon, a comma, or a period.
- Use parentheses only for a code name, a path, a unit, an abbreviation, or a reference. Do not put a second
  idea in parentheses.
- Write headings in sentence case.

### Examples

| Do not write | Write |
| --- | --- |
| Once the build has completed, the artifacts are uploaded. | After the build completes, the CI job uploads the artifacts. |
| The webhook delivery retry limit is configurable. | You can set the retry limit for webhook delivery with `webhook.max_retries`. |
| Utilize the CLI in order to obtain the logs. | Use the CLI to get the logs: `app logs --tail 100`. |
| The scheduler is the heartbeat of the system. | The scheduler starts each job at the time in its `cron` field. |
| When retrying, the request is sent again with the same ID. | When the client retries, it sends the request again with the same ID. |
| Requests may fail due to rate limiting, so retries should be added. | The API returns status 429 when a client exceeds its rate limit. Retry the request after the delay in the `Retry-After` header. |

### Check your text

Before you finish, read each text again and fix each failure:

- A sentence has more than one topic, or more than 20 words in an instruction or 25 words in a description.
- A paragraph has more than six sentences, or does not start with its topic sentence.
- A verb is in the passive voice when the actor is known, or uses a form such as "has sent" or "is sending".
- A thing has two names, or a word has two meanings.
- The text has a metaphor, an idiom, a contraction, "e.g.", "i.e.", "etc.", a semicolon, or an em dash.
- The text uses "may", "might", "should", or "would", or uses "since", "as", "while", or "once" to connect
  clauses.
- A caution does not start with the command.

## Code conventions

- Rust 2024, toolchain pinned in `rust-toolchain.toml`. `cargo clippy --workspace --all-targets -- -D warnings`
  stays clean.
- Default to no comments. Write one short line only where the reason is not obvious: a hidden constraint, an
  invariant, a workaround. Rule citations are the exception and always stay.
- No error handling for cases that cannot happen; validate at boundaries (API input, model output, files).
- Engine errors are `EngineError`; API errors are `ApiErr`, which turn into `{error, issues}` JSON.
- Prose that users or models read follows the writing standard below (HANDOVER section 20).
- Commit messages: a short title of at most 72 characters, a type prefix (`feat:`, `fix:`, `docs:`,
  `refactor:`, `test:`, `chore:`) and the change in a few words, for example `feat: Add planning evals`.
  The details go in the body after a blank line: what changed and why, one bullet per part, with the rule
  IDs it touches. Never put the details in the title.
- The title's type sets the version bump, because `./release.sh` computes the next version from the titles
  since the last tag (git-cliff, `cliff.toml`): `feat:` is a feature, `fix:` and the rest a patch. A change
  that breaks a user (config keys, API or CLI shape, stored data that no longer loads) adds `!` after the type,
  `feat!: Rename the routes table`, and a `BREAKING CHANGE: <what to do>` line at the end of the body.
  An optional scope names the crate or area, `fix(sandbox): ...`. Never bump versions by hand in a commit.

## Tests

```bash
cargo test --workspace                      # unit, conformance, runner, recovery, server end to end
cargo clippy --workspace --all-targets -- -D warnings
cd web && npm run check && npm run typecheck && npx vitest run   # biome lint + format check
cd design && npm run check && npm run typecheck && npx vitest run
cd site && npm run check && npm run typecheck && npx vitest run
cd tests/browser && npm test                # browser security suite: the console and the site
```

- `tests/conformance/main.rs`: planner fixtures, one per rule. The fastest way to test engine behavior.
- `tests/browser/specs/site.spec.ts`: the site as a static host serves it. Its pages carry their CSP as a meta
  tag (`site/vite.config.ts`, and `web/vite.config.ts` for the console shot), which must stay equal to the
  server's in `crates/ostra-server/src/api.rs`.
- `crates/ostra-engine/tests/runner.rs` and `recover.rs`: a real `Engine` with fake services.
- `crates/ostra-server/tests/e2e.rs`: the whole stack with a `ScriptedProvider` playing each agent.
- Provider live tests are `#[ignore]`d and run with `cargo test -p ostra-providers -- --ignored`.
- Judge routing evals (`tests/evals/judges.toml`, run by `crates/ostra-server/tests/judge_evals.rs`) are live and
  `#[ignore]`d: `OSTRA_EVAL_MODELS=anthropic:claude-opus-5-5 cargo test -p ostra-server --test judge_evals -- --ignored --nocapture`.
  Run every case several times per model before judging a prompt change, and add a counter-case with each fix.
- Advisor evals (`tests/evals/advisor.toml`, run by `crates/ostra-server/tests/advisor_evals.rs`) run the real
  advisor agent on failed init steps laid out on disk, 5 runs per model by default, and grade each decision:
  `OSTRA_EVAL_MODELS=anthropic:claude-opus-5-5,anthropic:claude-sonnet-5-5 cargo test -p ostra-server --test advisor_evals -- --ignored --nocapture`.
- Coordination evals (`tests/evals/coordination.toml`, run by `crates/ostra-server/tests/coordination_evals.rs`) run real
  sessions on a snapshot of this repository with scripted judges; only the runs a case lists go live. Tiers 1 to 3
  (`OSTRA_EVAL_TIERS`), 3 runs per model by default:
  `OSTRA_EVAL_MODELS=anthropic:claude-opus-5-5,anthropic:claude-sonnet-5-5 cargo test -p ostra-server --test coordination_evals -- --ignored --nocapture`.
  Its offline test replays every case with stand-ins, so run the normal suite after editing a case. The harness wait is
  checked live with `harness_probe wake <harness> <model> <dir>` (`crates/ostra-exec-harness/examples/`).
- Book retrieval eval (`tests/evals/book_retrieval/`, run by `crates/ostra-core/tests/book_retrieval.rs`) is offline
  and runs with the normal suite: 221 questions about Ostra against `book.json`, a book an Opus docs run wrote about a
  snapshot of this repository, graded by `labels/` (the sections that state each answer) with floors on hit@1, hit@5,
  and MRR. `OSTRA_EVAL_REPORT=1 ... -- --nocapture` prints every miss. A new `book.json` needs new labels.
- Planning evals (`tests/evals/planning.toml`, run by `crates/ostra-server/tests/planning_evals.rs`) run generate-spec,
  plan, and both fact-checks live on 30 change requests against Ostra pinned to one upstream commit, on research
  recorded once into `tests/evals/planning/research/` (`OSTRA_EVAL_MODE=record`) and replayed through the Document
  tool, and report cost, code files read again, plan rounds, and plan coverage per stage into
  `tests/evals/planning/results/`, which is committed with the cases. `OSTRA_EVAL_BUDGET` caps the run. The same file
  runs on an unchanged engine in a worktree at the pin, and `planning_compare` diffs two reports:
  `OSTRA_EVAL_MODEL=anthropic:claude-sonnet-5-5 cargo test -p ostra-server --test planning_evals planning_evals -- --ignored --nocapture`.
- Test stage evals (`tests/evals/test_stage.toml`, run by `crates/ostra-server/tests/test_stage_evals.rs`) run the live
  analyzer, write-test, or both in real sessions on the small projects in `tests/evals/test_stage/`, and grade write-test
  by planted mutants its tests must catch. Tiers 1 to 3, 3 runs per model by default (Opus, Sonnet, and Haiku):
  `OSTRA_EVAL_MODELS=anthropic:claude-opus-5-5,anthropic:claude-sonnet-5-5 cargo test -p ostra-server --test test_stage_evals -- --ignored --nocapture`.
  Its offline test replays every case with the golden analysis and tests, and needs `python3` and `node`.

## Live runs cost money

Running Ostra against a real repo, and especially against this repo, starts many top-tier executions.

- Use a scratch setup: `OSTRA_CONFIG=/tmp/x/config.toml OSTRA_DATA_DIR=/tmp/x/data`, cheap tiers
  (`advanced = "anthropic:claude-sonnet-5-5"`), a small `session_budget_usd`, and a tiny scratch repo.
- Never run init or a session against this repository without the user's go-ahead and a budget.
- Stop a runaway session from its board or `POST /api/sessions/<id>/stop`. With the server down, run
  `ostra stop <session-id>` before starting it again; otherwise recovery re-runs every interrupted execution.

## Traps

- The `rtk` hook rewrites `curl` and mangles JSON bodies. Use `rtk proxy curl ...` when parsing output.
- `pkill -f "<pattern>"` matches the shell running it. Find the pid with `pgrep` and `kill` it.
- The release binary embeds `web/dist` at compile time: run `npm run build` in `web/` before
  `cargo build --release` when the UI or `design/` changed.
- The homepage's console shot is `web/` built with `--mode shot` (`web/.env.shot`: `VITE_MOCK=1 VITE_SHOT=1`)
  into `site/public/console` (`npm run build:console` in `site/`). Set these through Vite modes, not inline in npm
  scripts, because npm runs scripts through `cmd.exe` on Windows. `npm run dev` in `site/` shows it only after one such build.
- One-time sign-in tokens: `ostra url` mints a new one; clipboard managers that preview links spend them.
  Server log: `~/.local/share/ostra/server.log`.
- `server.json` in the data dir records the running server; `ostra stop` refuses while it is alive.
