# Agents

Ostra divides the work of the pipeline among fourteen agents. Each agent does one job. For example, an agent
researches a request, writes a spec, reviews a change, or writes tests. Code decides which agent runs, which
inputs it gets, and what Ostra does with its result. The agent does the work in its stage. Then it returns a
structured answer.

This page tells what the agents are and how Ostra describes them. It also tells how Ostra gives them their
inputs before a run, and how it reads their results. The main rule is this: **the result of an agent is the
data that it submits through a typed tool call, never the text that it writes at the end of its run.** The
remaining sections of this page come from that rule.

## The roster

Each agent has two files in `assets/agents/<name>/`:

- `agent.toml` describes the agent.
- `prompt.md` is the system prompt of the agent.

The compiler embeds the two files in the `ostra` binary. Thus, a running server never reads agent definitions
from disk. A user cannot replace a definition through a change to a file.

| Agent | Stage | Default tier | What it does |
| --- | --- | --- | --- |
| `explore` | Research | advanced | Researches one task and writes one research document. It is the only pipeline agent that searches the web. It cites each external page that it uses by URL and date. It reports findings, never requirements. |
| `generate-spec` | Spec | advanced | Reads each research document for the request and writes one spec. The spec holds requirements in EARS notation with Given/When/Then criteria, in ordered deliverables. The spec tells what to build, never how. A new codebase gets a new project key in the spec. Ostra creates that project only after the user approves the plan. |
| `fact-check` | Fact-check | advanced | Checks a spec or a plan for claims that can break the implementer. It also checks for external facts that no longer trace to a cited page. It runs after each spec and each plan. Ostra refuses approval without a recorded `PASS`. |
| `plan` | Plan | advanced | Changes an approved spec into a master plan and one file for each phase. Each step names an exact path, an action, the skills to load, and a verification command. It takes each requirement from the spec. It reads no research document, only the code facts that Ostra extracts from the research documents. It lists each new project that the spec names in `new_projects`. |
| `implementer` | Build | balanced | Writes the code for one of these: a plan phase, a review fix, an inline change, or the fix for a stuck run that the user sent it to (rule O8). It verifies each step with the build command of the project. It never writes tests. |
| `code-reviewer` | Review | balanced | Reviews the unstaged changes of one review loop against the rule set of the project and the requirements of the phase. It also runs a security scan. No instruction can override a BLOCKER finding of that scan. |
| `execution-path-analyzer` | Test | balanced | Plans how Ostra verifies a phase. It traces each path through the functions that the phase changed (branches, early returns, error paths, boundaries). It also traces the system flows that reach these functions (from a route, a CLI command, a screen, a job, or a consumer of a changed contract), and the existing tests that cover them. It gives each check a test level from the test types of the project. `write-test` changes each path and flow into one test. |
| `write-test` | Test | balanced | Verifies the phase. It writes unit, integration, and end-to-end tests at the levels that the analyzer assigned, and it obeys the test skills of the project. Then it runs these tests and the existing suites that the analyzer listed as regression. It writes only test code. |
| `documentation` | Docs | advanced | Writes the part of one project in the workspace documentation book. The part has an overview, glossary terms, and sections of one unit of work each (purpose, boundaries, assumptions, business flow, small Mermaid diagrams, tables, separation of concerns, code references). The agent checks all of it against the real source. It returns the part in its submit call and writes no file. It runs only when the user asks for documentation. |
| `system-architecture` | Docs | advanced | Writes how the projects of a book of two or more projects work together. It writes the components and what each one owns, the links with protocol and payload, failure and recovery, and scaling, with one flowchart. It returns them in its submit call and writes no file. |
| `prompt-generation` | Build | advanced | Writes or edits instruction files (system prompts, `SKILL.md` skills, agent definitions). It runs for prompt requests. It also runs when an implementer gives the prompt work to it. |
| `initializer` | Project setup | balanced | Sets up a project in one of six modes: detect, scout, propose, generate-skill, generate-inventory, and adopt. |
| `advisor` | Rescue | advanced (high effort) | Reads one of these: a failed or stuck step of the init of a created project (rule O5), or a build or test run that is stuck on its environment (rule O7). It reads the inputs, the outputs, and the project. Then it submits `retry` with guidance for the next run of the step, or `escalate` with a reason for the user. It is read-only. |
| `quick-answer` | Side panel | balanced | Answers one question about the workspace from the code, the project memory, and fetched pages. It never writes files and never changes the pipeline state. |

The tier column gives a default. The workspace routing settings select the model for each tier. They can also
route an agent in a different way for each phase complexity. Thus, an implementer phase of low complexity can
run on a cheaper model than a phase of high complexity.

Ten smaller prompts in `assets/judges/` are not agents. They answer named judgment questions for the engine.
For example, a judge decides the risk of a request, or decides if Ostra must fix a review finding. Read
[the engine](../../HANDOVER.md#8-the-engine) for the function of the judges.

The Routing tab of Settings lists the same agents. Each agent has a one-line role and its route:

![The Routing settings tab with every agent, its role, executor, model, and effort](../images/console/settings-routing.png)

## What `agent.toml` says

This is the full definition of the reviewer:

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

Each field has one function:

- **`description`** tells when the agent runs and what it can change. The UI shows it. The descriptions also
  give a short statement of the limits of each agent ("Read-only on project source", "Writes ONLY test
  code").
- **`default_tier`** is the model tier of the agent, if the routing does not set a different one.
- **`timeout_seconds`** limits the time of one execution. Quick answers get 5 minutes, and reviewers get 20
  minutes. Agents that write code get 40 minutes.
- **`capabilities`** lists the abstract actions that the agent can do. The reviewer has no `write` or `edit`.
  Thus, no executor gives it a tool that changes files. Capabilities are a first filter. The policy layer still
  checks each tool call of the agent, also for agents that hold `write`.
- **`[effort]`** sets the reasoning effort for each executor. The keys are `native` (Ostra's own agent loop) and
  one key for each harness: `claude` (Claude Code), `codex` (Codex), `grok` (Grok Build), and `agy`
  (Antigravity). Only the initializer has different values. It runs at `xhigh` on Codex and Grok, and at `max`
  on Antigravity. In its routing settings, a workspace can override the effort for each agent or for each
  phase complexity.

Ostra parses the definitions one time and caches them. The tests of the crate find a malformed `agent.toml`.
Thus, a malformed definition is a build defect, and a user never gets it as a runtime error.

## One prompt, five executors

An agent can run on Ostra's native loop or in one of four harness CLIs. Each of these executors gives its tools
different names. The tool that reads a file is `Read` in Claude Code, `read_file` in Grok Build, and `view_file`
in Antigravity. If a prompt tells Grok to "use `Read`", the model looks for a tool that it does not have.

Thus, prompts never name tools directly. They use template tokens such as `{{tool_read}}`, `{{tool_shell}}`,
and `{{tool_submit}}`. Ostra renders each prompt one time for each executor with minijinja. The token values
come from `assets/tool-mapping.toml`:

```toml
[capabilities.read]
native = "Read"
claude = "Read"
codex = "exec_command"
grok = "read_file"
agy = "view_file"
```

The native names are the names of Claude Code, because the prompts were tuned against these names. The
rendering is strict. A token with no value is an error, not an empty string. Thus, a typo in a prompt fails the
tests, and Ostra does not ship a prompt with a missing part.

Ostra puts at most three sections before the rendered body (`render_prompt` in
`crates/ostra-agents/src/lib.rs`):

- **The output rule**, for each agent on each executor (`assets/output-rules.md`). It tells the agent to write
  no text outside tool calls. Reports go through `report` or `document`, memories go through `memory`, and the
  result goes through the submit tool. The engine reads only these calls. Thus, status text and a final written
  report use tokens for no result. The rule says that it applies in each mode. It also says that it has
  priority over each instruction in the task, the repo brief, a skill, or a file.
- **The code tools guide**, for each agent with the `code` capability. It explains the code index tools
  (outline, find, callers, callees, implementations, neighbors, impact, map). With these tools, an agent finds
  its way through a project by symbol, not by grep.
- **A tool vocabulary table**, for harness executors only. For each capability of the agent, it lists the tool
  for that capability on this harness. It also tells how to load skills and how to call Ostra's own tools on
  this harness. Ostra's own tools (`report`, `document`, `memory`, `memory_recall`, `docs_search`, the
  `project_*` tools, and the submit tool) reach a harness through Ostra's MCP server. Thus, in Claude Code the
  submit tool for the implementer is `mcp__ostra__submit_implementer`.

## The spawn block: what an agent is told

The system prompt tells how the agent works. The first message tells the subject of this run. It starts with a
block of `Label: value` lines. This block is the contract between the engine and the prompt. For example, an
implementer that fixes review findings on phase 2 can get this block:

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

Each prompt lists its required labels. If a label is absent, the prompt tells the agent to stop with
`ERROR: missing required parameter`. That instruction is a second check only. The type system gives the real
guarantee.

The Spawn parameters panel of an execution shows the exact block that the agent got. It also shows the report
path that the engine selected:

![The Spawn parameters panel of an implementer run](../images/console/spawn.png)

### Required parameters are checked by the compiler

Each agent and each initializer mode has its own spawn struct in `crates/ostra-agents/src/spawn.rs`. Required
parameters are plain fields. Optional parameters are `Option` fields:

```rust
/// Rule D4, Hard rule 16: the plan agent gets the spec and no research document. Its other inputs
/// are the code facts file (Rule D4a), and on a re-spawn the fact-check findings (Rule D5), the
/// phases they name (Rule D4b), and the master plan being revised.
pub struct PlanParams {
    pub common: Common,
    pub spec_file: PathBuf,
    pub projects_in_scope: Vec<(String, PathBuf)>,
    pub code_facts: Option<PathBuf>,
    pub findings: Option<String>,
    pub phases_to_revise: Vec<u32>,
    pub master_plan: Option<PathBuf>,
}
```

If code spawns a plan agent without a spec file, the code does not compile. Ostra is a port of the Ultracode
plugin. In Ultracode, the same contract was a JSON file, and a hook checked it at spawn time. Thus, a missing
parameter showed as a failed run. In Ostra, it shows as a build error.

The struct also controls what an agent must *not* see. `PlanParams` has no field for research documents,
because Rule D4 says that the plan takes its requirements from the approved spec only. If a requirement is not
in the spec, it cannot get into the plan, because the struct has no field for it. The research findings about
the code arrive as `code_facts` (Rule D4a). This is a file that the engine writes with files, symbols, patterns,
and flows, and with no request text. For the same reason, `FactCheckParams` carries research documents only for
a spec target (Rule D5). For a plan target, it carries the code facts file.

These shapes occur in many structs:

- **`Common`** holds the four lines that each spawn carries: workspace root, repo root, session dir, and repo
  key.
- **`WorkSource`** makes phase-bound agents (implementer, reviewer, write-test) declare one of two lines:
  `Phase file:`, or `No plan:` with a reason (Hard rule 13). An agent cannot declare both lines or neither line.
  When no plan exists, the engine writes the reason for the agent. For example: "A quick change: the request
  names the whole edit, so research, spec, plan, and review were skipped."
- **`Extras`** holds optional context. Each item renders only when it is present: research documents, user
  answers, verbatim findings, required skills, the review ledger, a rescue diagnostic, resume instructions after
  a handoff, prior phase reports, user notes, and a free-form task note. The user notes are the answers that the
  Route-answer judge kept for this stage (Rule J1).

Each struct renders itself in a fixed order: the required lines, then the four common lines, then the extras.
The same module has a parser, `parse_block`. The parser reads a block and validates it against the contract of
the agent. It also validates the value formats:

- `Phase:` must be `N`, `N-tests`, or `none`.
- An enum value must be one of its listed values.
- A path must be absolute.

The test suite renders each struct and parses the result. Thus, a struct and its contract always agree.

### From planner decision to spawn

The planner of the engine decides *that* an agent must run, and with which inputs. It gives this decision as a
`SpawnRequest` that holds loose `SpawnInputs`. The spawn factory (`crates/ostra-engine/src/factory.rs`) changes
these inputs into the typed struct of the agent. In the factory, missing data becomes a clear error ("missing
spec file"), not an unclear prompt. Also, the engine makes these decisions in the factory, and the agent does
not make them:

- The `Review scope:` of the reviewer is always `unstaged`. Ostra stages each accepted change, so that the next
  review sees only the new change.
- A rerun after an interruption gets a task note. The note tells the agent to continue from its progress log,
  and not to start again.
- A rescue after a `stuck` result gets the verbatim diagnostic and the fact that the user gave. Thus, a rescue
  is never a plain retry. If the user sent an implementer to fix the cause (Rule O8), the fact is the summary,
  the report, and the changed files of that implementer.
- An implementer that the user sends to a stuck run gets `No plan:` and an `Unblock:` line. That line holds the
  diagnostic and the need of the stuck run, and the instructions of the user. The implementer also gets the
  phase file under `Context files:`. Its report is `ostra-implementer-unblock-phase-<N>-<round>.md`. Thus, it
  never writes over the report or the progress log of the stuck run.

Then the factory assembles the full execution:

- The rendered system prompt for the selected executor.
- The first message: the spawn block and the repo brief.
- The parameters as JSON for the event log.
- The report path.
- The resolved effort.

## Report paths belong to the engine

An agent never selects the location of its report. The engine names each file in `ostra_core::paths::report`.
It gives the path to the agent as `Report file:`:

| File | Written by |
| --- | --- |
| `ostra-implementer-phase-N.md` | implementer |
| `ostra-implementer-progress-phase-N.md` | implementer, during its work |
| `ostra-epa-phase-N.md` | execution-path-analyzer |
| `ostra-write-test-phase-N.md` | write-test |
| `ostra-review-ledger-phase-N.md`, `-phase-N-tests.md` | code-reviewer, then the fix agent |
| `ostra-docs-request.md` | the runner, for a `DOCS` request, in place of an implementer report |
| `ostra-prompt-gen-N.md` | prompt-generation |

All these files are in the session directory of the project. There are three reasons for this:

1. The report-path guard of the policy layer can let an agent write exactly one path outside the source of the
   project. The guard denies all other paths. Thus, a read-only agent can write its report, and nothing more.
2. The engine knows the location of the report before the agent finishes. Thus, after a crash or a stop, a
   known file is available to resume from.
3. The spawn block of the next agent can name the output of the previous agent directly. Write-test gets the
   report of the implementer and the report of the analyzer by path.

Also, the native executor refuses an `ok` submit if the declared report file does not exist. It tells the agent
to write the file first.

This image shows the report at the path that the engine named, opened as an artifact of the session:

![An implementer report with Changes, Verification, and Tests to write](../images/console/report.png)

### A new codebase across three agents

Some requests need a codebase that no project holds. Such a request goes through three agents. No agent
creates the project before the user approves the plan that needs it:

- **generate-spec** (Step 4A of its prompt) gives the codebase a new project key. It tags its criteria and
  deliverables with the key. It writes one `Constraint` criterion that names the stack. It also writes one
  `Constraint` criterion for each base requirement that the evidence decides. Its first deliverable in that
  project creates the project skeleton.
- **plan** puts phases in that key and lists the key in `new_projects`. It copies the stack, the purpose, and
  the base requirements into the context of the first phase in the key.
- **implementer** is the only agent with the `manage_projects` capability. The implementer of that first phase
  gets a `New project:` spawn line, and its session dir as `Repo root:`. It reads the phase file. Then it calls
  `ProjectCreate` with these facts and no other data (rule O2). After the project exists, Ostra stops the run
  and initializes the project. Then Ostra starts the phase again in the project. If the user denies the call,
  the implementer returns stuck, with the refusal as its need.

### What the advisor is given

The engine keeps little context about the cause of a failed step. Thus, the spawn of the advisor carries what
the step saw and did. `advisor_request` in [`init.rs`](../../crates/ostra-engine/src/init.rs) builds it:

| Line | Content |
| --- | --- |
| `Failed step:` | The agent and mode, for example `initializer detect`. For a stuck build or test run, the agent only, for example `write-test` |
| `Problem:` | One of these: the error, the stuck report (its summary, then what it needs), or the finding of the engine (no slices, no skills, a missing inventory) |
| `Step inputs:` | The spawn block of the failed run |
| `Step result:` | The submit payload of the failed run as JSON, cut at 8,000 characters. A step can submit `ok` with a result that Ostra cannot use |
| `Step context:` | The purpose of the step. For a created project: its stack, purpose, and base requirements. For a run: the phase, its file, and the loop (build or test) of the run |
| `Earlier guidance:` | The guidance that an earlier round gave this step, which did not fix it |

The advisor also reads the instructions of the failed agent. Ostra renders the prompt of each agent for the
native executor. It writes each prompt to `<data dir>/assets/agents/<agent>.md`, next to the stack references.
The prompt of the advisor tells it to read the part for the failed mode. In most init failures, a step did not
obey one of its own rules. An example is the rule for a project that has no source. Some advice tells a step to
break its rules, for example to scaffold code during initialization. Such advice fails again at a guard. The
advisor evals (`tests/evals/advisor.toml`) showed both facts.

## The repo brief

Each execution gets a repo brief below the spawn block, after a `---` line (`crates/ostra-agents/src/brief.rs`).
Without the brief, an agent uses its first tool calls to collect these facts:

- The build and test commands.
- The skills of the project, with their paths.
- The conventions.
- The module-map rows that cover the paths in the task.

Each agent gets only the sections that it uses:

| Agent | Brief sections |
| --- | --- |
| implementer | commands, skills, conventions, module map |
| write-test | commands, testing, skills, conventions, module map |
| code-reviewer | commands, the full review rule set, conventions, skills |
| explore, plan, quick-answer | stack, skills, module map (plan and quick-answer also get commands) |
| generate-spec, fact-check | stack, module map |
| execution-path-analyzer | commands, testing, module map |
| prompt-generation | skills |
| initializer | nothing, because it is the agent that creates these facts |
| advisor | stack, commands, skills, module map |
| documentation | stack, commands, module map |
| system-architecture | stack, module map |

Only the reviewer gets the full rule catalog, because only the reviewer grades against it.

These parts come after the brief, in this order:

1. The instruction files of the project (`CLAUDE.md`, `AGENTS.md`, `AGENT.md`).
2. If the session created a project, a "Projects created in this session" section. For each created project,
   it gives the folder, stack, purpose, and base requirements from its `ProjectCreate` call. Such a project
   has no inventory or profile before its init runs.
3. The workspace artifacts: the folder and at most 40 files (read
   [Workspace artifacts](workspaces.md#workspace-artifacts)).
4. The custom instructions of the workspace: first the entry for all agents, then the entry for this agent.

If an instruction tags a file or an artifact with `@`, Ostra lists it under that instruction with its absolute
path. If `AGENTS.md` in a repo is a symlink to `CLAUDE.md` or a copy of it, Ostra includes the file one time.

These rules keep the brief small and correct:

- **It has limits.** The brief has a limit of 3,600 characters, 16 skill rows, and 10 module rows. Each
  instruction file has a limit of 12,000 characters. Ostra cuts a longer file, and the agent reads the
  remaining text from disk.
- **It does not repeat itself.** Before the brief adds a profile fact, it checks if the project inventory
  already states the fact. In one repo, a field can be a duplicate. In a different repo, the same field can be
  the only statement of a rule. Thus, the check uses the actual text of the inventory, not a fixed list of
  fields.
- **It never carries routing settings.** Tier names and model choices give no information to an agent. They
  only add noise to its context.
- **Ostra adds it one time.** If a message already contains a brief heading, a second addition returns the
  message without a change. Thus, a re-render or a resume never puts two briefs in a message.

The Instructions tab of Settings holds the custom instructions for all agents and for each agent:

![The Instructions settings tab with a workspace artifact tagged in the text for all agents](../images/console/settings-instructions.png)

## Structured returns

Each agent ends with a call to one tool: `submit_<agent>`, for example `submit_fact_check` or
`submit_implementer`. The input schema of the tool is a Rust struct in `crates/ostra-core/src/submit.rs`, which
Ostra converts to JSON Schema. This is the fact-check return:

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

The spec approval gate works because of this struct. The engine does not read a sentence such as "looks good
to me" and then decide if it means pass. It reads `verdict: "PASS"`, and it allows or refuses approval on that
value. Each finding carries an `element`, for example `R3`. Thus, the UI can show the finding on requirement 3
in the spec view.

The returns are in these groups:

| Submit struct | Agents | Key fields |
| --- | --- | --- |
| `ExploreSubmit` | explore | research path, scope covered, findings summary, source count, open question count |
| `GenerateSpecSubmit` | generate-spec | spec path, open questions, deliverable and requirement counts |
| `FactCheckSubmit` | fact-check | verdict, target, findings |
| `PlanSubmit` | plan | master plan path, phases with project, complexity, test policy, and dependencies |
| `ImplementerSubmit` | implementer | status, report path, changed files, summary, stuck or handoff details |
| `CodeReviewerSubmit` | code-reviewer | findings, `security_block`, ledger path, summary |
| `ReportSubmit` | execution-path-analyzer, write-test, prompt-generation | status, report path, changed files, summary |
| `DocumentationSubmit` | documentation | status, summary, overview, sections with sub-sections, glossary |
| `ArchitectureSubmit` | system-architecture | status, summary, the architecture (overview, diagram, components, links, failure and recovery, scalability), glossary |
| `InitializerSubmit` | initializer | status, summary, files, a result object that differs per mode |
| `AdvisorSubmit` | advisor | `action` (`retry` or `escalate`), guidance for the next run, reason for the user |
| `QuickAnswerSubmit` | quick-answer | the answer in Markdown, its sources |

If the work of an agent can fail, its return carries a `status` of `ok`, `stuck`, or `handoff`. `stuck` means
that the agent got to its retry limit on the same failure. The agent must then include the verbatim diagnostic
and the one fact that it needs to continue. The engine changes that into a rescue, not into a blind retry.
`handoff` means that an implementer needs a specialist. At this time, the specialist is always
prompt-generation. The implementer must tell what to write and how to resume after the handoff.

### Validation happens before anything is recorded

When an agent calls its submit tool, the two executors do the same checks before they accept the call:

1. **Shape.** `validate_submit` parses the input into the struct of the agent. If a field is missing or an enum
   value is wrong, the agent gets a tool error. The error holds the message of the parser and "Fix it and call
   submit again."
2. **Consistency.** Some rules apply to more than one field. The `security_block` of the reviewer must be true
   exactly when a BLOCKER finding is present. Thus, a reviewer cannot report a BLOCKER and also say that nothing
   is blocked. The reverse is also not possible.
3. **Documents.** For explore, generate-spec, and plan, Ostra opens and checks the referenced document. The
   counts that the agent reported must agree with the contents of the file. Examples are the number of
   requirements and the number of evidence rows. An agent cannot submit a spec summary that is different from
   the spec that it wrote.
4. **Report file.** If an agent has a declared report path, Ostra refuses its `ok` submit until that file
   exists.

Nothing gets into the event log before all four checks pass. A rejected submit costs the agent one more turn.
It never makes a half-valid record that the engine must interpret.

Sometimes a model sends a nested object or array as a string of JSON. Ostra parses such a top-level argument
before the validation, if the schema gives it a type that is not a string. Thus, a format error does not cause
a correct submit to fail.

## Why the engine never reads the final message

In Ultracode, some agents printed a JSON object as their last message. Hooks such as `factcheck-record.js`
then copied the object from the transcript. This method fails in ways that are difficult to see. A model puts
the JSON in a code fence, adds a sentence after it, or writes a summary in place of it. Each harness shows the
final message in a different location and format. A scraper that handles all these cases only guesses. A guess
at a verdict is not acceptable for a gate that allows or blocks approval.

A tool call removes the guess:

- **Ostra enforces the schema at the boundary.** The model sees the schema when it calls the tool. Ostra refuses
  a wrong shape at a time when the agent can still fix it.
- **It works the same on each executor.** Native runs get the submit tool as a usual tool definition. Harness
  runs get it from Ostra's MCP server. In both cases, the engine gets the same struct.
- **It marks the end of the run.** The description of the submit tool says that it must be the last action, and
  the engine applies this rule. On the native loop, the engine skips each other tool call in the same turn with
  "Not run: the run ended with the submit call."
- **It keeps the result apart from the narration.** Ostra streams the text that the model writes during its work
  to the UI, where people can watch it. The engine never parses this text. Thus, the comments of a model cannot
  change what the engine does.

Ostra also makes sure that each run ends with a submit. An agent can call only its own submit tool. Thus, Ostra
denies a call to `submit_plan` from an implementer, and the denial names the correct tool. If a harness run tries
to stop without a submit, Ostra blocks the stop at most two times, with an instruction to submit first. If a
native run goes 400 model turns without a submit, it fails with that reason.

There are two narrow exceptions. Both are fallbacks, never a decision:

- If a quick-answer run ends `ok` with no submit, Ostra shows its final text as the answer. A side-panel answer
  changes no pipeline state, so the display of the text loses nothing.
- If an agent reports `stuck` without a diagnostic, the final text becomes the diagnostic in the rescue prompt.
  The routing decision still comes from the submitted `status`.

## Subagents that talk to each other

Reports carry results from one stage to the next. But a reader of a report does not get all the knowledge of
its writer. For example, the reader does not know these facts:

- Why a requirement has its wording.
- Which files the implementer already examined and rejected.
- What a finding refers to.

To read everything again costs tokens. A judge cannot give the missing facts, because it never saw the contents
of the two conversations. Thus, agents can also send questions to each other directly. Each agent keeps its
conversation, so that Ostra can wake it again. HANDOVER section 10.8 holds the rules (H1 to H9).

### A subagent ID names a conversation

Each execution is one run of a model. A **subagent** is a conversation that can contain more than one run. Its
subagent ID is the id of the first execution of the conversation. A run keeps the ID of the conversation that it
continues. This is true for a run that continues in place (the same execution, resumed). It is also true for a
new execution whose `resumes` field names the run before it. The fold keeps a map from each execution to its
subagent (`SessionState::subagents`). The **head** of a subagent is its latest run. A question to a subagent
goes to its head.

### The three tools

Seven agents have the `coordinate` capability: explore, generate-spec, fact-check, plan, implementer,
code-reviewer, and write-test. This capability gives them three tools. It also gives them a prompt section
(`assets/coordination.md`) that tells when to use the tools:

| Tool | What it does |
| --- | --- |
| `SubagentList` | Gives your own subagent ID and each subagent of the session, with its agent, status, report, and what it waits on. It marks the subagents that you work with. For example, a fact-checker sees "the author of the document you check". |
| `SubagentAsk` | Ask a question. With `agent: "explore"`, Ostra starts a helper research task from the question. With `subagent_id`, the question goes to an existing subagent. |
| `SubagentReply` | Answer the question that woke this run. |

The tools never ask you for permission in any mode, because they change no file. But a deny rule can still
refuse them. The engine checks each call against the fold. It records the call as an event before the tool
returns (`Engine::coordinate`, the checks in `crates/ostra-engine/src/coord.rs`).

### Asking waits

A run that asks a question stops its work until the answer arrives. The method of the wait depends on the
executor:

- **Native.** The loop ends the run with status `waiting`, in the same way that a submit ends it. The fold sets
  the stage of the run as paused, and the planner holds its next spawn. When the answer is ready, that held
  spawn resumes the same execution in place. The loop then replays the stored messages, with the answer as the
  new user turn. The prefix does not change, so the prompt cache of the provider still covers it.
- **Harness.** The CLI process stays alive. The MCP call returns an instruction: end the turn and reply only
  `Waiting`. Then the executor waits for the message of the engine, and it does not supervise for a submit.
  During the wait, these rules apply:
  - The executor gives back its execution slot.
  - Ostra lets a Stop without a submit through.
  - The time of the wait does not count against the timeout.

  When the answer arrives, Ostra types it into the terminal, in the same way that you type. Then the model
  continues. If the server restarts during the wait, recovery records the run as `waiting`, not as interrupted.
  Later, the answer resumes the harness session from its stored session id.

The release of the slot is important. If `max_parallel_executions = 1` and an asker keeps its slot, the asker
waits forever for a helper that can never start.

### Where a question goes

A question to `explore` becomes a research task, tagged with the question (`ExploreOrigin::Ask`). It is the
same type of task that classification starts. It does not hold the research stage, because only the asker waits
for it. Its submit is the answer: the findings summary, the path of the research document, and what it did not
cover. The document also becomes one of the research documents of the session, so later stages read it too. If
the helper fails, its failure is the answer, and no failure gate opens.

A question to a subagent goes to the subagent in the state that it is in:

1. **It waits on the asker.** An example is a spec author that waits on its helper, and the helper now asks the
   author a question. Ostra wakes the subagent in place with the question, and the subagent answers with
   `SubagentReply`. Then it waits for its own answer again. With this method, two agents can send messages in
   the two directions.
2. **Its last run ended `ok`** (or `stuck` or `handoff`). Ostra starts a **consult run**. A consult run is a new
   execution that continues the conversation of the subagent, with the question as its new turn. A consult run
   cannot write files. The policy refuses writes with "Answer with SubagentReply and change no file". The run
   ends when it replies.
3. **It is running, or waits on a different subagent.** The question waits until the subagent is free.
4. **It failed** (error, denied, cancelled). Ostra wakes the asker with that failure, so no run waits forever. If
   a run gets a question and ends without a reply, the asker gets the same type of message.

Some runs owe an answer: a consult run, and a run that Ostra woke with a question. Such a run must reply before
it can end for a different reason. The guard refuses its submit call with "Call SubagentReply with your answer
instead". Each of these reminders names the reply tool, not the submit tool:

- The reminder of the native loop after a turn with no tool call.
- The Stop that Ostra sends back to a harness.
- The nudge that Ostra types into a quiet terminal.

A harness gets the information that it owes an answer from the wake, because the question arrives after its
process started. The coordination evals showed the need for this. A consult run ended its turn in text. Ostra
reminded it to call the submit tool of its agent, so it never replied.

### The pair loops continue conversations

The loops of the pipeline between two agents use the same mechanism. The engine starts these continuations,
not a tool call, because the engine holds the gates. These loops continue a conversation:

- When a fact-check fails, the next spec or plan round continues the conversation of the author. The next
  fact-check pass continues the conversation of the checker.
- When a review has findings, the fix continues the last worker of the phase. The re-review continues the
  reviewer.
- A rescue after `stuck` and a resume after a handoff also continue the worker.

The fold makes this decision (`SessionState::continuation`), and the planner marks the spawn. Fixtures show it as
`spawn generate-spec spec#2 (continues)`.

The new turn of a continued run is a fixed header and then the new spawn block. The block carries what the round
needs: the findings, the answers, and the prior findings of a re-pass. The pass or fail result, the review cap,
and the recurring fact-check gate do not change. Only the input changes. The run stays on the executor and the
model where its conversation started.

In these cases, a loop starts a new run, because a continuation is wrong or not possible:

- The previous run did not end `ok` with a submit.
- Ostra moved the agent to a different executor after a harness failure.
- A harness run left no session id to resume.
- You amended the request after the previous run started.
- The conversation already holds six runs (`MAX_CONVERSATION_RUNS`). A long conversation costs more per turn
  than a new start.

A conversation never has two live runs. A continuation or a consult run waits until no other run of the same
subagent is live.

### Limits

Each question can start a run, so Ostra limits the questions:

- A run can start at most three helpers (`MAX_HELPERS_PER_RUN`).
- A session can have at most 24 questions (`MAX_SESSION_ASKS`).
- A helper cannot start helpers.
- A consult run cannot ask.
- A run that owes an answer must reply before it asks.

Helpers and consult runs go through the slot limiter and the budget guard, the same as each other spawn. A
session does not complete if a question is open.

### What the log records

Each step is an event:

- `AgentAsked`: a run asks.
- `AgentReplied`: a run answers.
- `MessageDelivered`: Ostra gives a question or an answer to a run that waits.

A helper or a consult run gets its question as its spawn, so its `ExecutionStarted` is the delivery. The fold
derives all other facts from these events, including the answer of the helper and each failure answer. Thus, a
replay builds again exactly which run waits for which answer.

### Measuring coordination

Conformance fixtures prove where the engine routes a question. But they cannot show these model behaviors:

- The model asks the correct subagent.
- The model answers from its conversation.
- The model does not ask when no question is necessary.

The coordination evals measure these behaviors. [`tests/evals/coordination.toml`](../../tests/evals/coordination.toml)
holds cases in three tiers. All cases use Ostra's own source:

1. **Answering.** A consult run explains a number that only its conversation holds. It refuses to edit the spec
   when it gets a request to change it. It reads code that it never discussed, to give a correct answer. A spec
   author that the question of its helper wakes replies from its conversation and then waits again.
2. **Deciding.** An implementer has no web tools. It needs a model id that the vendor published last week. It
   must ask an explore helper and not guess the id. A fact-checker finds the spec author among three subagents
   and asks it. Two controls must ask no subagent: a fully traceable spec and a one-file rename.
3. **Loops.** A live fact-checker fails a false claim, and its second pass continues its conversation. A live
   reviewer finds a planted bug. The live implementer fixes the bug in its own continued conversation, and the
   reviewer reviews again. A live helper confirms its scope with the live author before it starts its
   research.

[`crates/ostra-server/tests/coordination_evals.rs`](../../crates/ostra-server/tests/coordination_evals.rs) runs
each case as a real session of a real `Engine`. The session runs on a git clone of a snapshot of Ostra's working
tree. The snapshot does not include the eval, so no agent can read the expected answers. The judges are
scripted. A router executor sends each run that the case lists as live to the native loop, on the model under
test. That loop has the policy, the sandbox, a code index of the snapshot, and coordination tools that connect
to that engine. The router plays each other run from the case: what an author said, the file that it wrote,
and the question that a checker asked. Thus, each wake, consult run, continuation, and released slot goes
through the code of the engine. Only the runs under test cost tokens.

A run passes when two conditions are true:

- Each code check passes: who asked whom and what, the replies, statuses, submits, files, continuations, and
  cache reads.
- A grader model finds that the run meets the rubric. The grader reads a record of the tool calls of each run,
  each question and answer, the live submits, and the diff.

The report gives the pass rates for each tier and model, the cost, and the part of the live input tokens that
came from the prompt cache. If a live run of a session stopped because of a provider error, the session runs
again, at most two times. The report shows such a session apart from the pass rates. An offline test replays
each case with stand-ins for its live runs. It checks that each live run starts, that the session stops where
the case says, and that each expected continuation continues its conversation. Thus, a broken case fails in the
normal suite.

`harness_probe wake` checks the harness side of rule H2 live. In this check, a CLI waits with its process alive,
and Ostra types the answer into its terminal. The agent asks two times and must submit the two answers.

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
| Subagent coordination: tool inputs and limits | `crates/ostra-core/src/coord.rs` |
| Coordination in the fold: routes, deliveries, continuations | `crates/ostra-engine/src/coord.rs` |
| The coordination prompt section | `assets/coordination.md` |
| Coordination evals | `tests/evals/coordination.toml`, `crates/ostra-server/tests/coordination_evals.rs` |
| Test stage evals | `tests/evals/test_stage.toml`, `tests/evals/test_stage/`, `crates/ostra-server/tests/test_stage_evals.rs` |
| Planning evals | `tests/evals/planning.toml`, `tests/evals/planning/research/`, `tests/evals/planning/results/`, `crates/ostra-server/tests/planning_evals.rs` |
| Report file names | `crates/ostra-core/src/paths.rs` (`report`) |
| Submit handling, native | `crates/ostra-exec-native/src/lib.rs` |
| Submit handling, harness | `crates/ostra-exec-harness/src/bridge.rs`, `live.rs` |
