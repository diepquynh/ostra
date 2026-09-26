# Agents

Ostra splits the pipeline's work among twelve agents. Each one does one job, such as researching a request,
writing a spec, reviewing a change, or writing tests. Code decides which agent runs, what it is given, and what
happens with its result. The agent does the work inside its stage and then hands back a structured answer.

This page covers what the agents are, how Ostra describes them, how it briefs them before a run, and how it
reads their results. The central rule is simple: **an agent's result is the data it submits through a typed
tool call, never the text it writes at the end of its run.** The rest of this page follows from that rule.

## The roster

Every agent lives in `assets/agents/<name>/` as two files: `agent.toml`, which describes it, and `prompt.md`,
its system prompt. Both are embedded in the `ostra` binary at compile time, so a running server never reads
agent definitions from disk and a user cannot swap one out by editing a file.

| Agent | Stage | Default tier | What it does |
| --- | --- | --- | --- |
| `explore` | Research | advanced | Researches one task and writes one research document. It is the only pipeline agent that searches the web, and every external page it relies on is cited by URL and date. It reports findings, never requirements. |
| `generate-spec` | Spec | advanced | Reads every research document for the request and writes one spec: requirements in EARS notation with Given/When/Then criteria, grouped into ordered deliverables. It states what to build, never how. |
| `fact-check` | Fact-check | advanced | Checks a spec or a plan for claims that would break the implementer and for external facts that no longer trace to a cited page. It runs after every spec and every plan, and Ostra refuses approval without a recorded `PASS`. |
| `plan` | Plan | advanced | Turns an approved spec into a master plan plus one file per phase. Each step names an exact path, an action, the skills to load, and a verification command. It reads the spec and nothing else. |
| `implementer` | Build | balanced | Writes the code for one plan phase, one review fix, or one inline change, and verifies each step with the project's build command. It never writes tests. |
| `code-reviewer` | Review | balanced | Reviews the unstaged changes of one review loop against the project's rule set and the phase's requirements, and runs a security scan whose BLOCKER findings no instruction can override. |
| `execution-path-analyzer` | Test | balanced | Traces every path through the functions a phase changed (branches, early returns, error paths, boundaries) and writes a report that `write-test` turns into one test per path. |
| `write-test` | Test | balanced | Writes tests for the paths the analyzer found, following the project's test skills. It writes only test code. |
| `module-documentation` | Docs | advanced | Updates the module-hub area references from what the phases actually changed, and checks every documented name against real source. It runs only when the user asks for documentation. |
| `prompt-generation` | Build | advanced | Writes or edits instruction files (system prompts, `SKILL.md` skills, agent definitions). It runs for prompt requests and when an implementer hands off prompt authoring. |
| `initializer` | Project setup | balanced | Bootstraps a project in one of six modes: detect, scout, propose, generate-skill, generate-inventory, and adopt. |
| `quick-answer` | Side panel | balanced | Answers one question about the workspace from the code, project memory, and fetched pages. It never writes files and never changes pipeline state. |

The tier column is a default. Workspace routing settings pick the model behind each tier and can route an agent
differently per phase complexity, so a low-complexity implementer phase can run on a cheaper model than a
high-complexity one.

Eight smaller prompts in `assets/judges/` are not agents. They answer named judgment questions for the engine,
such as how risky a request is or whether a review finding should be fixed. See [the engine](../../HANDOVER.md#8-the-engine)
for how judges fit in.

## What `agent.toml` says

Here is the reviewer's definition, in full:

```toml
description = "Reviews the unstaged working-tree changes of one review loop against the project's Review Rule Set ..."
default_tier = "balanced"
timeout_seconds = 1200
capabilities = ["read", "shell", "search_text", "glob", "code"]

[effort]
native = "high"
claude = "high"
codex = "high"
grok = "high"
agy = "high"
```

Each field has one job:

- **`description`** says when the agent runs and what it may touch. The UI shows it, and the descriptions
  double as a quick statement of each agent's boundaries ("Read-only on project source", "Writes ONLY test
  code").
- **`default_tier`** is the model tier the agent runs on unless routing says otherwise.
- **`timeout_seconds`** bounds one execution. Quick answers get 5 minutes, reviewers 20, and agents that write
  code get 40.
- **`capabilities`** lists what the agent can do, in abstract terms. The reviewer has no `write` or `edit`, so
  no tool that changes files is offered to it on any executor. Capabilities are a first filter. The policy
  layer still checks every tool call the agent makes, including calls from agents that do hold `write`.
- **`[effort]`** sets reasoning effort per executor. The keys are `native` (Ostra's own agent loop) and one
  per harness: `claude` (Claude Code), `codex` (Codex), `grok` (Grok Build), and `agy` (Antigravity). The
  initializer is the one agent that differs: it runs at `xhigh` on Codex and Grok and at `max` on
  Antigravity. A workspace can override effort per agent or per phase complexity in its routing settings.

The definitions are parsed once and cached. A malformed `agent.toml` is caught by the crate's tests, so it is a
build defect and never a runtime error a user could hit.

## One prompt, five executors

An agent can run on Ostra's native loop or inside any of four harness CLIs, and each of those names its tools
differently. Reading a file is `Read` in Claude Code, `read_file` in Grok Build, and `view_file` in
Antigravity. A prompt that says "use `Read`" to Grok would send the model looking for a tool it does not have.

So prompts never name tools directly. They use template tokens such as `{{tool_read}}`, `{{tool_shell}}`, and
`{{tool_submit}}`, and Ostra renders each prompt once per executor with minijinja. The token values come from
`assets/tool-mapping.toml`:

```toml
[capabilities.read]
native = "Read"
claude = "Read"
codex = "exec_command"
grok = "read_file"
agy = "view_file"
```

Native names follow Claude Code's, because the prompts were tuned against those names. Rendering is strict: a
token with no value is an error, not an empty string, so a typo in a prompt fails the tests instead of shipping
a prompt with a hole in it.

Two sections can be prepended to the rendered body (`render_prompt` in `crates/ostra-agents/src/lib.rs`):

- **The code tools guide**, for every agent with the `code` capability. It explains the code index tools
  (outline, find, callers, callees, implementations, neighbors, impact, map) that let an agent navigate a
  project by symbol instead of by grep.
- **A tool vocabulary table**, for harness executors only. It lists, for each capability the agent holds, the
  tool that serves it on this harness, plus how to load skills and call Ostra's own tools there. Ostra's own
  tools (`report`, `document`, `memory`, `memory_recall`, and the submit tool) reach a harness through Ostra's
  MCP server, so in Claude Code the submit tool for the implementer is `mcp__ostra__submit_implementer`.

## The spawn block: what an agent is told

The system prompt says how the agent works. The first message says what this particular run is about. It opens
with a block of `Label: value` lines, which is the contract between the engine and the prompt. An implementer
fixing review findings on phase 2 might receive:

```text
Report file: /ws/.ostra/sessions/s1/backend/ostra-implementer-phase-2.md
Phase file: /ws/.ostra/sessions/s1/backend/ostra-plan-phase-2.md
Workspace root: /ws
Repo root: /ws/backend
Session dir: /ws/.ostra/sessions/s1/backend
Repo key: backend
Findings:
  [HIGH] src/auth.rs (R-b) - Token compared with ==. Fix: Use constant-time comparison.
Review ledger: /ws/.ostra/sessions/s1/backend/ostra-review-ledger-phase-2.md
```

Each prompt lists its required labels and tells the agent to stop with `ERROR: missing required parameter` if
one is absent. That instruction is a backstop. The real guarantee is in the type system.

### Required parameters are checked by the compiler

Every agent, and every initializer mode, has its own spawn struct in `crates/ostra-agents/src/spawn.rs`.
Required parameters are plain fields and optional ones are `Option`:

```rust
/// Rule D4, Hard rule 16: the plan agent gets the spec and nothing else. The only other inputs are
/// the fact-check findings of a re-pass (Rule D5) and the master plan being revised.
pub struct PlanParams {
    pub common: Common,
    pub spec_file: PathBuf,
    pub projects_in_scope: Vec<(String, PathBuf)>,
    pub findings: Option<String>,
    pub master_plan: Option<PathBuf>,
}
```

Code that spawns a plan agent without a spec file does not compile. In Ultracode, the plugin Ostra is ported
from, the same contract was a JSON file checked by a hook at spawn time, so a missing parameter surfaced as a
failed run. Here it surfaces as a build error.

The struct also enforces what an agent must *not* see. `PlanParams` has no field for research documents,
because Rule D4 says the plan is built from the approved spec alone. Research that did not make it into the
spec cannot leak into the plan, since there is nowhere to put it. `FactCheckParams` carries research documents
only on a spec target, for the same reason (Rule D5).

A few shapes recur:

- **`Common`** holds the four lines every spawn carries: workspace root, repo root, session dir, and repo key.
- **`WorkSource`** makes phase-bound agents (implementer, reviewer, write-test) declare either `Phase file:`
  or `No plan:` with a reason, never both and never neither (Hard rule 13). When no plan exists the engine
  writes the reason for the agent, for example "A quick change: the request names the whole edit, so
  research, spec, plan, and review were skipped."
- **`Extras`** holds optional context that renders only when present: research documents, user answers,
  verbatim findings, required skills, the review ledger, a rescue diagnostic, resume instructions after a
  handoff, prior phase reports, and a free-form task note.

Each struct renders itself in a fixed order: required lines, then the common four, then extras. The same module
has a parser, `parse_block`, that reads a block back and validates it against the agent's contract, including
value formats (`Phase:` must be `N`, `N-tests`, or `none`; enums must be one of their listed values; paths must
be absolute). The test suite renders every struct and parses the result, so a struct and its contract cannot
drift apart.

### From planner decision to spawn

The engine's planner decides *that* an agent should run and with what inputs, as a `SpawnRequest` holding loose
`SpawnInputs`. The spawn factory (`crates/ostra-engine/src/factory.rs`) turns those inputs into the agent's
typed struct. This is where missing data becomes a clear error ("missing spec file") instead of a vague
prompt, and where the engine makes decisions the agent should not make for itself:

- The reviewer's `Review scope:` is always `unstaged`, because Ostra stages each accepted change so the next
  review sees only the new one.
- A rerun after an interruption gets a task note telling the agent to continue from its progress log rather
  than start over.
- A rescue after a `stuck` result gets the verbatim diagnostic plus the fact the user supplied, so it is
  never a plain retry.

The factory then assembles the full execution: the rendered system prompt for the chosen executor, the first
message (spawn block plus repo brief), the parameters as JSON for the event log, the report path, and the
resolved effort.

## Report paths belong to the engine

An agent never chooses where its report goes. The engine names every file in `ostra_core::paths::report` and
passes the path in as `Report file:`:

| File | Written by |
| --- | --- |
| `ostra-implementer-phase-N.md` | implementer |
| `ostra-implementer-progress-phase-N.md` | implementer, as it works |
| `ostra-epa-phase-N.md` | execution-path-analyzer |
| `ostra-write-test-phase-N.md` | write-test |
| `ostra-review-ledger-phase-N.md`, `-phase-N-tests.md` | code-reviewer, then the fix agent |
| `ostra-module-docs.md` | module-documentation |
| `ostra-prompt-gen-N.md` | prompt-generation |

All of them live in the session directory for that project. There are three reasons for this:

1. The policy layer's report-path guard can allow an agent to write exactly one path outside the project's
   source, and deny everything else. A read-only agent can still write its report, and nothing more.
2. The engine knows where to look before the agent finishes, so a crash or a stop leaves a known file to
   resume from.
3. The next agent's spawn block can name the previous agent's output directly. Write-test is given the
   implementer's report and the analyzer's report by path.

The native executor also refuses an `ok` submit while the declared report file does not exist, and tells the
agent to write it first.

## The repo brief

Below the spawn block, separated by a `---` line, every execution gets a repo brief
(`crates/ostra-agents/src/brief.rs`). It carries the facts an agent would otherwise spend its first several
tool calls gathering: the build and test commands, the project's skills with their paths, conventions, and
the module-map rows that cover the paths in the task.

Each agent gets only the sections it uses:

| Agent | Brief sections |
| --- | --- |
| implementer | commands, skills, conventions, module map |
| write-test | commands, testing, skills, conventions, module map |
| code-reviewer | commands, the full review rule set, conventions, skills |
| explore, plan, quick-answer | stack, skills, module map (plan and quick-answer also get commands) |
| generate-spec, fact-check | stack, module map |
| execution-path-analyzer | testing, module map |
| prompt-generation | skills |
| initializer | nothing, because it is the agent that creates these facts |

The reviewer is the only agent that receives the complete rule catalog, because it is the only one that
grades against it.

After the brief come the project's own instruction files (`CLAUDE.md`, `AGENTS.md`, `AGENT.md`) and then the
workspace's custom instructions: first the entry for all agents, then the entry for this agent. If a repo ships
`AGENTS.md` as a symlink to, or a copy of, `CLAUDE.md`, it is included once.

A few rules keep the brief small and correct:

- **It is capped.** The brief is limited to 3,600 characters, at most 16 skill rows and 10 module rows, and
  each instruction file to 12,000 characters. A longer file is cut and the agent reads the rest from disk.
- **It does not repeat itself.** Before adding a profile fact, the brief checks whether the project inventory
  already states it. The same field can be a duplicate in one repo and the only statement of a rule in
  another, so the check runs against the inventory's actual text rather than a fixed list of fields.
- **It never carries routing settings.** Tier names and model choices mean nothing to an agent and would only
  add noise to its context.
- **It is added once.** If a message already contains a brief heading, adding the brief again returns the
  message unchanged, so a re-render or a resume never stacks two briefs.

## Structured returns

Every agent finishes by calling one tool: `submit_<agent>`, as in `submit_fact_check` or
`submit_implementer`. The tool's input schema is a Rust struct in `crates/ostra-core/src/submit.rs`, converted
to JSON Schema. Here is the fact-check return:

```rust
pub struct FactCheckFinding {
    pub severity: Severity,          // BLOCKER, HIGH, MEDIUM, or LOW
    pub location: String,
    /// The ID of the element the claim sits in: `R3`, `AC3.2`, `E2`, `D1`, `C4`, `phase 2`, or
    /// `step 2.3`. Ostra shows the finding on that element.
    pub element: Option<String>,
    pub claim: String,
    pub issue: String,
}

pub struct FactCheckSubmit {
    /// `PASS` only when no finding is HIGH or MEDIUM.
    pub verdict: Verdict,            // PASS, FAIL, or ERROR
    /// `spec` or `plan`.
    pub target: String,
    pub findings: Vec<FactCheckFinding>,
}
```

This struct is what lets the spec approval gate work. The engine does not read a sentence like "looks good
to me" and decide whether it means pass. It reads `verdict: "PASS"`, and approval is allowed or refused on
that value. Because each finding carries an `element` such as `R3`, the UI can pin it to requirement 3 in the
spec view.

The returns fall into a few families:

| Submit struct | Agents | Key fields |
| --- | --- | --- |
| `ExploreSubmit` | explore | research path, scope covered, findings summary, source count, open question count |
| `GenerateSpecSubmit` | generate-spec | spec path, open questions, deliverable and requirement counts |
| `FactCheckSubmit` | fact-check | verdict, target, findings |
| `PlanSubmit` | plan | master plan path, phases with project, complexity, test policy, and dependencies |
| `ImplementerSubmit` | implementer | status, report path, changed files, summary, stuck or handoff details |
| `CodeReviewerSubmit` | code-reviewer | findings, `security_block`, ledger path, summary |
| `ReportSubmit` | execution-path-analyzer, write-test, module-documentation, prompt-generation | status, report path, changed files, summary |
| `InitializerSubmit` | initializer | status, summary, files, a result object that differs per mode |
| `QuickAnswerSubmit` | quick-answer | the answer in Markdown, its sources |

Agents that do work which can fail carry a `status` of `ok`, `stuck`, or `handoff`. `stuck` means the agent hit
its retry limit on the same failure, and it must include the verbatim diagnostic and the one fact it needs to
continue. The engine turns that into a rescue, not a blind retry. `handoff` means an implementer needs a
specialist, currently always prompt-generation, and it must say what to author and how to resume afterwards.

### Validation happens before anything is recorded

When an agent calls its submit tool, both executors run the same checks before accepting it:

1. **Shape.** `validate_submit` parses the input into the agent's struct. A missing field or a wrong enum value
   comes back to the agent as a tool error with the parser's message and "Fix it and call submit again."
2. **Consistency.** Some rules cross fields. The reviewer's `security_block` must be true exactly when a
   BLOCKER finding is present, so a reviewer cannot report a BLOCKER while claiming nothing is blocked, or the
   reverse.
3. **Documents.** For explore, generate-spec, and plan, the referenced document is opened and checked. The
   counts the agent reported, such as the number of requirements or evidence rows, must match what the file
   contains. An agent cannot submit a spec summary that differs from the spec it wrote.
4. **Report file.** An `ok` submit from an agent with a declared report path is refused until that file
   exists.

Nothing reaches the event log until all four pass. A rejected submit costs the agent one more turn. It never
produces a half-valid record the engine has to interpret.

Models sometimes send a nested object or array as a string of JSON. Before validating, Ostra parses any
top-level argument that arrived that way when the schema types it as something other than a string, so a formatting slip does not fail an otherwise correct submit.

## Why the engine never reads the final message

In Ultracode, several agents ended by printing a JSON object as their last message, and hooks such as
`factcheck-record.js` scraped it out of the transcript. That approach fails in ways that are hard to notice. A
model wraps the JSON in a code fence, adds a sentence after it, or summarizes instead of printing it. Each
harness shows the final message in a different place and format. A scraper that handles all of those cases is
guessing, and a guess at a verdict is not acceptable for a gate that allows or blocks approval.

A tool call removes the guesswork:

- **The schema is enforced at the boundary.** The model sees the schema when it calls the tool, and a wrong
  shape is refused while the agent can still fix it.
- **It works the same on every executor.** Native runs get the submit tool as a regular tool definition.
  Harness runs get it from Ostra's MCP server. Either way the engine receives the same struct.
- **It marks the end of the run.** The submit tool's description says it must be the last action, and the
  engine treats it that way. On the native loop, any other tool calls in the same turn are skipped with "Not
  run: the run ended with the submit call."
- **It separates the result from the narration.** Text the model writes while working is streamed to the UI
  for people to watch. The engine never parses it, so a model's commentary cannot change what the engine
  does.

Ostra also makes sure a run ends with a submit. An agent may only call its own submit tool, so a call to
`submit_plan` from an implementer is denied and names the right tool. A harness run that tries to stop without
submitting is blocked, up to two times, with an instruction to submit first. A native run that goes 400 model
turns without submitting fails with that as the reason.

There are two narrow exceptions, and both are fallbacks, never a decision:

- If a quick-answer run ends `ok` with no submit, its final text is shown as the answer. A side-panel answer
  changes no pipeline state, so showing the text loses nothing.
- If an agent reports `stuck` without a diagnostic, the final text becomes the diagnostic shown in the rescue
  prompt. The routing decision still comes from the submitted `status`.

## Where to look in the code

| What | Where |
| --- | --- |
| Agent definitions and prompts | `assets/agents/<name>/` |
| Tool names per executor | `assets/tool-mapping.toml` |
| Loading definitions and rendering prompts | `crates/ostra-agents/src/lib.rs`, `mapping.rs` |
| Spawn structs and the block parser | `crates/ostra-agents/src/spawn.rs` |
| The repo brief | `crates/ostra-agents/src/brief.rs` |
| Planner inputs to spawn structs | `crates/ostra-engine/src/factory.rs` |
| Submit schemas and `validate_submit` | `crates/ostra-core/src/submit.rs` |
| Submit document checks | `crates/ostra-core/src/doc/check.rs` |
| Report file names | `crates/ostra-core/src/paths.rs` (`report`) |
| Submit handling, native | `crates/ostra-exec-native/src/lib.rs` |
| Submit handling, harness | `crates/ostra-exec-harness/src/bridge.rs`, `live.rs` |
