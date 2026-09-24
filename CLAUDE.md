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
  and drifts otherwise. When a change touches `web/`, finish by asking the user to run `/design-sync` for it
  and list the screens that changed. The target and the method (targeted edits to `ui_kits/console/*`, never
  the converter) are in `.design-sync/NOTES.md`; add a dated line there for each sync.

## Crates and the direction of dependencies

```
ostra-core        ids, settings and route resolution, pipeline enums, submit schemas, the event log,
                  API DTOs (exported to TypeScript), the Executor and ExecutionHost traits
ostra-store       SQLite: workspace db (event log plus materialized tables), registry, project memory
ostra-policy      guards and permissions over canonical ToolCalls; bash parsing
ostra-tools       native tool implementations (Read, Write, Edit, Bash, Grep, Glob, Skill, WebFetch, ...)
ostra-providers   Anthropic and OpenAI streaming clients; ScriptedProvider for tests
ostra-agents      embedded assets/, prompt rendering per executor, typed spawn structs, the repo brief
ostra-engine      event-sourced session state, the pure planner, judges, the runner, the spawn factory
ostra-exec-native the native agent loop (providers + tools + policy)
ostra-exec-harness harness executors: PTY, per-harness adapters, hook bridge, MCP stdio shim
ostra-notify      Web Push without OpenSSL
ostra-server      the `ostra` binary: axum, auth, REST, WebSocket, embedded web build, CLI
```

`ostra-core` depends on nothing internal. `ostra-engine` knows no executor, provider, or server: it reaches
them through the `Services`, `SpawnFactory`, and `Executor` traits, and `ostra-server` wires the real ones in.
Keep it that way. A new capability the engine needs becomes a trait method, not a dependency.

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

**Change an API type.** Types in `ostra-core` carry `#[ts(export)]`. Run `cargo test -p ostra-core` to
regenerate `web/src/api/gen/`, then `cd web && npm run typecheck` and fix only what breaks.

## Code conventions

- Rust 2024, toolchain pinned in `rust-toolchain.toml`. `cargo clippy --workspace --all-targets -- -D warnings`
  stays clean.
- Default to no comments. Write one short line only where the reason is not obvious: a hidden constraint, an
  invariant, a workaround. Rule citations are the exception and always stay.
- No error handling for cases that cannot happen; validate at boundaries (API input, model output, files).
- Engine errors are `EngineError`; API errors are `ApiErr`, which turn into `{error, issues}` JSON.
- Prose that users or models read (prompts, judge prompts, UI copy, error messages) follows HANDOVER
  section 20: no em dashes, sentence-case headings, the instruction first and then the reason, no metaphor,
  no superlatives, and keep "because" clauses.

## Tests

```bash
cargo test --workspace                      # unit, conformance, runner, recovery, server end to end
cargo clippy --workspace --all-targets -- -D warnings
cd web && npm run typecheck && npx vitest run
```

- `tests/conformance/main.rs`: planner fixtures, one per rule. The fastest way to test engine behavior.
- `crates/ostra-engine/tests/runner.rs` and `recover.rs`: a real `Engine` with fake services.
- `crates/ostra-server/tests/e2e.rs`: the whole stack with a `ScriptedProvider` playing each agent.
- Provider live tests are `#[ignore]`d and run with `cargo test -p ostra-providers -- --ignored`.

## Live runs cost money

Running Ostra against a real repo, and especially against this repo, starts many top-tier executions.

- Use a scratch setup: `OSTRA_CONFIG=/tmp/x/config.toml OSTRA_DATA_DIR=/tmp/x/data`, cheap tiers
  (`advanced = "anthropic:claude-sonnet-5"`), a small `session_budget_usd`, and a tiny scratch repo.
- Never run init or a session against this repository without the user's go-ahead and a budget.
- Stop a runaway session from its board or `POST /api/sessions/<id>/stop`. With the server down, run
  `ostra stop <session-id>` before starting it again; otherwise recovery re-runs every interrupted execution.

## Traps

- The `rtk` hook rewrites `curl` and mangles JSON bodies. Use `rtk proxy curl ...` when parsing output.
- `pkill -f "<pattern>"` matches the shell running it. Find the pid with `pgrep` and `kill` it.
- The release binary embeds `web/dist` at compile time: run `npm run build` in `web/` before
  `cargo build --release` when the UI changed.
- One-time sign-in tokens: `ostra url` mints a new one; clipboard managers that preview links spend them.
  Server log: `~/.local/share/ostra/server.log`.
- `server.json` in the data dir records the running server; `ostra stop` refuses while it is alive.
