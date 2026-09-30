# Agents

Ostra splits the pipeline's work among thirteen agents. Each one does one job, such as researching a request,
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
| `generate-spec` | Spec | advanced | Reads every research document for the request and writes one spec: requirements in EARS notation with Given/When/Then criteria, grouped into ordered deliverables. It states what to build, never how. A new codebase gets a new project key in it, which Ostra creates only after the plan is approved. |
| `fact-check` | Fact-check | advanced | Checks a spec or a plan for claims that would break the implementer and for external facts that no longer trace to a cited page. It runs after every spec and every plan, and Ostra refuses approval without a recorded `PASS`. |
| `plan` | Plan | advanced | Turns an approved spec into a master plan plus one file per phase. Each step names an exact path, an action, the skills to load, and a verification command. It reads the spec and nothing else. It lists a new project the spec names in `new_projects`. |
| `implementer` | Build | balanced | Writes the code for one plan phase, one review fix, or one inline change, and verifies each step with the project's build command. It never writes tests. |
| `code-reviewer` | Review | balanced | Reviews the unstaged changes of one review loop against the project's rule set and the phase's requirements, and runs a security scan whose BLOCKER findings no instruction can override. |
| `execution-path-analyzer` | Test | balanced | Plans how a phase is verified. It traces every path through the functions the phase changed (branches, early returns, error paths, boundaries), the system flows that reach them (from a route, a CLI command, a screen, a job, or a consumer of a changed contract), and the existing tests that cover them, and gives each check a test level from the project's test types. `write-test` turns each path and flow into one test. |
| `write-test` | Test | balanced | Verifies the phase: writes unit, integration, and end-to-end tests at the levels the analyzer assigned, following the project's test skills, then runs them and the existing suites the analyzer listed as regression. It writes only test code. |
| `documentation` | Docs | advanced | Writes one project's part of the workspace documentation book: an overview, sections of one unit of work each (purpose, boundaries, assumptions, business flow, small Mermaid diagrams, tables, separation of concerns, code references), and glossary terms, all checked against real source. It returns the part in its submit call and writes no file. It runs only when the user asks for documentation. |
| `system-architecture` | Docs | advanced | Writes how the projects of a book of two or more projects work together: components and what each owns, links with protocol and payload, failure and recovery, and scaling, with one flowchart. It returns them in its submit call and writes no file. |
| `prompt-generation` | Build | advanced | Writes or edits instruction files (system prompts, `SKILL.md` skills, agent definitions). It runs for prompt requests and when an implementer hands off prompt authoring. |
| `initializer` | Project setup | balanced | Bootstraps a project in one of six modes: detect, scout, propose, generate-skill, generate-inventory, and adopt. |
| `advisor` | Rescue | advanced (high effort) | Reads one failed or stuck step of a created project's init, from its inputs, its outputs, and the project, and submits `retry` with guidance for the step's next run or `escalate` with a reason for the user (rule O5). It is read-only. |
| `quick-answer` | Side panel | balanced | Answers one question about the workspace from the code, project memory, and fetched pages. It never writes files and never changes pipeline state. |

The tier column is a default. Workspace routing settings pick the model behind each tier and can route an agent
differently per phase complexity, so a low-complexity implementer phase can run on a cheaper model than a
high-complexity one.

Eight smaller prompts in `assets/judges/` are not agents. They answer named judgment questions for the engine,
such as how risky a request is or whether a review finding should be fixed. See [the engine](../../HANDOVER.md#8-the-engine)
for how judges fit in.

The Routing tab of Settings lists the same agents, each with a one-line role and its route:

![The Routing settings tab with every agent, its role, executor, model, and effort](../images/console/settings-routing.png)

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

Up to three sections are prepended to the rendered body (`render_prompt` in `crates/ostra-agents/src/lib.rs`):

- **The output rule**, for every agent on every executor (`assets/output-rules.md`). It tells the agent to write
  no text outside tool calls: reports go through `report` or `document`, memories through `memory`, and the
  result through the submit tool. The engine reads only those calls, so status text and a final written report
  are wasted tokens. The rule states that it holds in every mode and over any instruction in the task, the
  repo brief, a skill, or a file.
- **The code tools guide**, for every agent with the `code` capability. It explains the code index tools
  (outline, find, callers, callees, implementations, neighbors, impact, map) that let an agent navigate a
  project by symbol instead of by grep.
- **A tool vocabulary table**, for harness executors only. It lists, for each capability the agent holds, the
  tool that serves it on this harness, plus how to load skills and call Ostra's own tools there. Ostra's own
  tools (`report`, `document`, `memory`, `memory_recall`, `docs_search`, the `project_*` tools, and the submit tool) reach a harness through Ostra's
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

An execution's Spawn parameters panel shows the exact block the agent received and the report path the engine
chose:

![The Spawn parameters panel of an implementer run](../images/console/spawn.png)

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
  handoff, prior phase reports, user notes (answers the Route-answer judge kept for this stage, Rule J1), and a
  free-form task note.

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
| `ostra-docs-request.md` | the runner, for a `DOCS` request, in place of an implementer report |
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

The report at the path the engine named, opened as an artifact of the session:

![An implementer report with Changes, Verification, and Tests to write](../images/console/report.png)

### A new codebase across three agents

A request that needs a codebase no project holds passes through three agents, and none of them creates the
project before the user has approved the plan that needs it:

- **generate-spec** (Step 4A of its prompt) gives the codebase a new project key, tags its criteria and
  deliverables with it, and writes one `Constraint` criterion naming the stack plus one per base requirement the
  evidence settles. Its first deliverable in that project creates the project skeleton.
- **plan** puts phases in that key, lists it in `new_projects`, and copies the stack, purpose, and base
  requirements into the context of the first phase in it.
- **implementer** is the only agent with the `manage_projects` capability. The implementer of that first phase
  gets a `New project:` spawn line and its session dir as `Repo root:`, reads the phase file, and calls
  `ProjectCreate` with those facts and nothing else (rule O2). Ostra stops the run once the project exists,
  initializes it, and starts the phase again inside it. If the user denies the call, the implementer returns
  stuck with the refusal as its need.

### What the advisor is given

The engine keeps little context about why a step failed, so the advisor's spawn carries what the step saw and
did, built by `advisor_request` in [`init.rs`](../../crates/ostra-engine/src/init.rs):

| Line | Content |
| --- | --- |
| `Failed step:` | The agent and mode, such as `initializer detect` |
| `Problem:` | The error, the stuck report as its summary and then what it needs, or the engine's own finding (no slices, no skills, a missing inventory) |
| `Step inputs:` | The failed run's own spawn block |
| `Step result:` | The failed run's submit payload as JSON, cut at 8,000 characters, because a step can submit `ok` with a result Ostra cannot use |
| `Step context:` | What the step is for: the created project's stack, purpose, and base requirements |
| `Earlier guidance:` | Guidance an earlier round gave this step, which did not fix it |

The advisor also reads the failed agent's own instructions. Ostra writes every agent's prompt, rendered for the
native executor, to `<data dir>/assets/agents/<agent>.md` next to the stack references, and the advisor's prompt
tells it to read the part for the failed mode. Most init failures are a step that missed one of its own rules,
such as the rule for a project with no source yet, and advice that asks a step to break its rules (for example to
scaffold code during initialization) fails again at a guard. The advisor evals below showed both.

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
| execution-path-analyzer | commands, testing, module map |
| prompt-generation | skills |
| initializer | nothing, because it is the agent that creates these facts |
| advisor | stack, commands, skills, module map |
| documentation | stack, commands, module map |
| system-architecture | stack, module map |

The reviewer is the only agent that receives the complete rule catalog, because it is the only one that
grades against it.

After the brief come the project's own instruction files (`CLAUDE.md`, `AGENTS.md`, `AGENT.md`), then, when the
session created a project, a "Projects created in this session" section with each one's folder, stack, purpose,
and base requirements from its `ProjectCreate` call, because such a project has no inventory or profile until
its init runs. Then come the workspace artifacts (the folder and up to 40 files, see
[Workspace artifacts](workspaces.md#workspace-artifacts)), and then the workspace's custom instructions: first
the entry for all agents, then the entry for this agent. A file or artifact an instruction tags with `@` is
listed under that instruction with its absolute path. If a repo ships
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

The Instructions tab of Settings holds the custom instructions for all agents and for each agent:

![The Instructions settings tab with a workspace artifact tagged in the text for all agents](../images/console/settings-instructions.png)

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
| `ReportSubmit` | execution-path-analyzer, write-test, prompt-generation | status, report path, changed files, summary |
| `DocumentationSubmit` | documentation | status, summary, overview, sections with sub-sections, glossary |
| `ArchitectureSubmit` | system-architecture | status, summary, the architecture (overview, diagram, components, links, failure and recovery, scalability), glossary |
| `InitializerSubmit` | initializer | status, summary, files, a result object that differs per mode |
| `AdvisorSubmit` | advisor | `action` (`retry` or `escalate`), guidance for the next run, reason for the user |
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

## Subagents that talk to each other

Reports carry results from one stage to the next, but a report read cold loses what its writer knew: why a
requirement is worded the way it is, which files the implementer already ruled out, what a finding refers to.
Re-reading everything costs tokens, and a judge cannot fill the gap either, because it never saw the inside of
either conversation. So agents can also reach each other directly, and every agent keeps its conversation so
that it can be woken again. HANDOVER section 10.8 holds the rules (H1 to H9).

### A subagent ID names a conversation

Each execution is one run of a model. A **subagent** is a conversation that may span several runs, and its
subagent ID is the id of the conversation's first execution. A run keeps the ID of the conversation it continues,
whether it continues in place (the same execution resumed) or as a new execution whose `resumes` field names the
run before it. The fold keeps a map from each execution to its subagent (`SessionState::subagents`), and the
subagent's **head** is its latest run. Asking a subagent means asking its head.

### The three tools

Seven agents have the `coordinate` capability: explore, generate-spec, fact-check, plan, implementer,
code-reviewer, and write-test. It gives them three tools, and a prompt section (`assets/coordination.md`) that
says when to use them:

| Tool | What it does |
| --- | --- |
| `SubagentList` | Your own subagent ID and every subagent of the session, with its agent, status, report, and what it waits on. The subagents you work with are marked, such as "the author of the document you check" for a fact-checker. |
| `SubagentAsk` | Ask a question. With `agent: "explore"` Ostra starts a helper research task from it. With `subagent_id` it goes to an existing subagent. |
| `SubagentReply` | Answer the question that woke this run. |

The tools never ask you for permission in any mode, because they change no file; a deny rule can still refuse
them. The engine checks each call against the fold and records it as an event before the tool returns
(`Engine::coordinate`, the checks in `crates/ostra-engine/src/coord.rs`).

### Asking waits

A run that asks stops working until the answer arrives. How it waits depends on the executor:

- **Native.** The loop ends the run with status `waiting`, like a submit ends it. The fold treats the run's stage
  as paused and the planner holds its next spawn. When the answer is ready, that held spawn resumes the same
  execution in place, and the loop replays the stored messages with the answer as the new user turn. The prefix
  is the same, so the provider's prompt cache still covers it.
- **Harness.** The CLI process stays alive. The MCP call returns an instruction to end the turn and reply only
  `Waiting`, and the executor then waits for the engine's message instead of supervising for a submit. While it
  waits it gives back its execution slot, a Stop without a submit is let through, and the time does not count
  against its timeout. When the answer arrives, Ostra types it into the terminal as if you had, and the model
  continues from there. If the server restarts during the wait, recovery records the run as `waiting` rather than
  interrupted, and the answer later resumes the harness session from its stored session id.

Freeing the slot matters: with `max_parallel_executions = 1`, an asker that kept its slot would wait forever for a
helper that can never start.

### Where a question goes

A question to `explore` becomes a research task like the ones classification starts, tagged with the question
(`ExploreOrigin::Ask`). It does not hold the research stage, because only the asker waits for it. Its submit is
the answer: the findings summary, the research document's path, and what it did not cover. The document also
joins the session's research documents, so later stages read it too. If the helper fails, its failure is the
answer, and no failure gate opens.

A question to a subagent goes wherever that subagent can answer:

1. **It waits on the asker**, for example the spec author waiting on the helper that now asks it back: the
   subagent is woken in place with the question and answers with `SubagentReply`. Then it goes back to waiting for
   its own answer. This is how two agents talk back and forth.
2. **Its last run ended `ok`** (or `stuck` or `handoff`): Ostra starts a **consult run**, a new execution that
   continues the subagent's conversation with the question as its new turn. A consult run may not write files;
   the policy refuses writes with "Answer with SubagentReply and change no file". It ends when it replies.
3. **It is running, or waits on someone else**: the question waits until the subagent is free.
4. **It failed** (error, denied, cancelled): the asker is woken with that failure, so no run waits forever. A
   run that was given a question and ends without replying answers with the same kind of message.

A run that owes an answer, a consult run or a run woken with a question, replies before anything else ends it.
The guard refuses its submit call with "Call SubagentReply with your answer instead", and each reminder names the
reply tool rather than the submit tool: the native loop's reminder after a turn with no tool call, a harness's
turned-back Stop, and the nudge Ostra types into a quiet terminal. A harness learns it owes an answer from the
wake itself, because the question arrives after its process started. The coordination evals found the need for
this: a consult run that ended its turn in text was reminded to call its agent's submit tool, so it never replied.

### The pair loops continue conversations

The pipeline's loops between two agents use the same mechanism, driven by the engine instead of a tool call,
because the engine holds the gates. When a fact-check fails, the next spec or plan round continues the author's
conversation; the next fact-check pass continues the checker's. When a review has findings, the fix continues
the phase's last worker, and the re-review continues the reviewer. A rescue after `stuck` and a resume after a
handoff continue the worker too. The fold decides this (`SessionState::continuation`), and the planner marks the
spawn: fixtures show it as `spawn generate-spec spec#2 (continues)`.

A continued run's new turn is a fixed header followed by the new spawn block, which carries what the round needs:
the findings, the answers, the prior findings of a re-pass. Pass or fail, the review cap, and the recurring
fact-check gate work exactly as before; only the input changes. The run stays on the executor and model its
conversation started on.

A loop starts a fresh run instead when continuing would be wrong or impossible: the previous run did not end `ok`
with a submit, the agent was moved to another executor after a harness failure, a harness run left no session id
to resume, you amended the request after the previous run started, or the conversation already holds six runs
(`MAX_CONVERSATION_RUNS`), because a long conversation costs more per turn than a fresh start. A conversation
never has two live runs: a continuation or consult waits while another run of the same subagent is live.

### Limits

Every question can start a run, so asks are bounded. A run may start at most three helpers
(`MAX_HELPERS_PER_RUN`) and a session at most 24 questions (`MAX_SESSION_ASKS`). A helper may not start helpers,
a consult run may not ask, and a run that owes an answer must reply before it asks. Helpers and consult runs go
through the slot limiter and the budget guard like every other spawn, and a session does not complete while a
question is open.

### What the log records

Each step is an event: `AgentAsked` when a run asks, `AgentReplied` when a run answers, and `MessageDelivered`
when Ostra hands a question or an answer to a run that waits. A helper or consult run gets its question as its
spawn, so its `ExecutionStarted` is the delivery. The fold derives everything else from these, including the
helper's answer and every failure answer, so a replay rebuilds exactly who waits for what.

### Measuring coordination

Conformance fixtures prove where the engine routes a question; they cannot say whether a model asks the right
subagent, answers from its conversation, or holds back when asking is not needed. The coordination evals do that.
[`tests/evals/coordination.toml`](../../tests/evals/coordination.toml) holds cases in three tiers, all set in
Ostra's own source:

1. **Answering.** A consult run explains a number only its conversation holds, declines to edit the spec it is
   asked to change, and reads code it never discussed to answer correctly. A spec author woken by its helper's
   question replies from its conversation and goes back to waiting.
2. **Deciding.** An implementer, which has no web tools, needs a model id published last week and should ask an
   explore helper instead of guessing it. A fact-checker finds the spec author among three subagents and asks it.
   Two controls, a fully traceable spec and a one-file rename, must ask no one.
3. **Loops.** A live fact-checker fails a false claim and its second pass continues its conversation; a live
   reviewer finds a planted bug, the live implementer fixes it in its own continued conversation, and the reviewer
   re-reviews; a live helper confirms its scope with the live author before it researches.

[`crates/ostra-server/tests/coordination_evals.rs`](../../crates/ostra-server/tests/coordination_evals.rs) runs
each case as a real session of a real `Engine`, on a git clone of a snapshot of Ostra's working tree that leaves
out the eval itself, so no agent can read the expected answers. Judges are scripted. A router executor sends each
run the case lists as live to the native loop on the model under test, with the policy, the sandbox, a code index
of the snapshot, and coordination tools wired to that engine, and plays every other run from the case: what an
author said, the file it wrote, the question a checker asked. So every wake, consult run, continuation, and freed
slot goes through the engine's own code, and only the runs under test cost tokens.

A run passes when every code check holds (who asked whom and what, the replies, statuses, submits, files,
continuations, cache reads) and a grader model finds the rubric met, reading a record of every run's tool calls,
every question and answer, the live submits, and the diff. The report gives pass rates per tier and model, cost,
and the share of live input tokens read from the prompt cache. A session whose live run died on a provider error
runs again, up to twice, and is reported apart from the pass rates. An offline test replays every case with
stand-ins for its live runs and checks that each live run is reached, the session stops where the case says, and
every expected continuation continues its conversation, so a broken case fails in the normal suite.

The harness side of rule H2, a CLI waiting with its process alive and the answer typed into its terminal, is
checked live by `harness_probe wake`: the agent asks twice, and must submit both answers.

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
| Report file names | `crates/ostra-core/src/paths.rs` (`report`) |
| Submit handling, native | `crates/ostra-exec-native/src/lib.rs` |
| Submit handling, harness | `crates/ostra-exec-harness/src/bridge.rs`, `live.rs` |
