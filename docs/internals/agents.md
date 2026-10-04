# Agents

Ostra splits the pipeline's work among fourteen built-in agents, and a workspace or a plugin can add its own
custom agents beside them. Each one does one job, such as researching a request, writing a spec, reviewing a
change, or writing tests. Code decides which agent runs, what it is given, and what
happens with its result. The agent does the work inside its stage and then hands back a structured answer.

This page covers what the agents are, how Ostra describes them, how it briefs them before a run, and how it
reads their results. The central rule is simple: **an agent's result is the data it submits through a typed
tool call, never the text it writes at the end of its run.** The rest of this page follows from that rule.

## The roster

Every built-in agent lives in `assets/agents/<name>/` as two files: `agent.toml`, which describes it, and
`prompt.md`, its system prompt. Both are embedded in the `ostra` binary at compile time, so a running server
never reads these definitions from disk and a user cannot swap one out by editing a file.

The fourteen agents are not wired into the engine by name. They are the standard plugin `ostra`
([`ostra-default-plugin`](../../crates/ostra-default-plugin/src/lib.rs)), written with `ostra-sdk` like any other plugin
(Rule PL4). It reads each agent's `agent.toml` and `prompt.md` with the SDK's definition parser
(`ostra_sdk::definition::parse_toml`) and sets nothing else, so everything that makes the reviewer a reviewer is
data in its files, in fields any agent can declare:

```rust
pub fn agent(agent: AgentName) -> Result<PluginAgent, String> {
    let toml_path = format!("{agent}/agent.toml");
    let prompt = agent_file(&format!("{agent}/prompt.md"))?;
    ostra_sdk::definition::parse_toml(agent.as_str(), &agent_file(&toml_path)?, &prompt)
        .map_err(|e| format!("assets/agents/{toml_path}: {e}"))
}
```

The only privilege the standard plugin has is its name: no other plugin may call itself `ostra`, and its agents
keep the built-in names, which is how a run of one is named. They are also the default agent for each result
contract a built-in stage reads (Rule WF8), which a workflow can replace with any agent that returns the same
contract.

| Agent | Stage | Default tier | What it does |
| --- | --- | --- | --- |
| `explore` | Research | advanced | Researches one task and writes one research document. It is the only pipeline agent that searches the web, and every external page it relies on is cited by URL and date. It reports findings, never requirements. |
| `generate-spec` | Spec | advanced | Reads every research document for the request and writes one spec: requirements in EARS notation with Given/When/Then criteria, grouped into ordered deliverables. It states what to build, never how. A new codebase gets a new project key in it, which Ostra creates only after the plan is approved. |
| `fact-check` | Fact-check | advanced | Checks a spec or a plan for claims that would break the implementer and for external facts that no longer trace to a cited page. It runs after every spec and every plan, and Ostra refuses approval without a recorded `PASS`. |
| `plan` | Plan | advanced | Turns an approved spec into a master plan plus one file per phase. Each step names an exact path, an action, the skills to load, and a verification command. It takes every requirement from the spec and reads no research document, only the code facts Ostra extracts from them. It lists a new project the spec names in `new_projects`. |
| `implementer` | Build | balanced | Writes the code for one plan phase, one review fix, one inline change, or the fix for a stuck run the user sent it to (rule O8), and verifies each step with the project's build command. It never writes tests. |
| `code-reviewer` | Review | balanced | Reviews the unstaged changes of one review loop against the project's rule set and the phase's requirements, and runs a security scan whose BLOCKER findings no instruction can override. |
| `execution-path-analyzer` | Test | balanced | Plans how a phase is verified. It traces every path through the functions the phase changed (branches, early returns, error paths, boundaries), the system flows that reach them (from a route, a CLI command, a screen, a job, or a consumer of a changed contract), and the existing tests that cover them, and gives each check a test level from the project's test types. `write-test` turns each path and flow into one test. |
| `write-test` | Test | balanced | Verifies the phase: writes unit, integration, and end-to-end tests at the levels the analyzer assigned, following the project's test skills, then runs them and the existing suites the analyzer listed as regression. It writes only test code. |
| `documentation` | Docs | advanced | Writes one project's part of the workspace documentation book: an overview, sections of one unit of work each (purpose, boundaries, assumptions, business flow, small Mermaid diagrams, tables, separation of concerns, code references), and glossary terms, all checked against real source. It returns the part in its submit call and writes no file. It runs only when the user asks for documentation. |
| `system-architecture` | Docs | advanced | Writes how the projects of a book of two or more projects work together: components and what each owns, links with protocol and payload, failure and recovery, and scaling, with one flowchart. It returns them in its submit call and writes no file. |
| `prompt-generation` | Build | advanced | Writes or edits instruction files (system prompts, `SKILL.md` skills, agent definitions). It runs for prompt requests and when an implementer hands off prompt authoring. |
| `initializer` | Project setup | balanced | Bootstraps a project in one of six modes: detect, scout, propose, generate-skill, generate-inventory, and adopt. |
| `advisor` | Rescue | advanced (high effort) | Reads one failed or stuck step of a created project's init (rule O5), or a build or test run stuck on its environment (rule O7), from its inputs, its outputs, and the project, and submits `retry` with guidance for the step's next run or `escalate` with a reason for the user. It is read-only. |
| `quick-answer` | Side panel | balanced | Answers one question about the workspace from the code, project memory, and fetched pages. It never writes files and never changes pipeline state. |

The tier column is a default. Workspace routing settings pick the model behind each tier and can route an agent
differently per phase complexity, so a low-complexity implementer phase can run on a cheaper model than a
high-complexity one.

Ten smaller prompts in `assets/judges/` are not agents. They answer named judgment questions for the engine,
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
capabilities = ["read", "shell", "search_text", "glob", "code", "coordinate", "docs_search", "review_ledger", "security_block"]

returns = "review"
write_scope = "session"
brief = ["commands", "review", "conventions", "skills"]

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
  layer still checks every tool call the agent makes, including calls from agents that do hold `write`. Some
  capabilities are grants rather than tools (Rule CA6): `review_ledger` and `security_block` let the reviewer
  write the review ledger and the security block file, which the guards refuse to every agent without them.
- **`returns`** is the result contract the agent submits (Rule CA5), here `review`. The engine reads the result
  by this contract, never by the agent's name. See [result contracts](#result-contracts-and-grants).
- **`write_scope`** bounds where the agent writes: `read_only`, `session`, `project`, or `setup` (Rule CA2).
- **`brief`** names the sections of the repo brief the agent gets (see [the repo brief](#the-repo-brief)).
- **`helper`**, set only in `explore`'s file, lets `SendMessage` start the agent as a helper.
- **`[effort]`** sets reasoning effort per executor. The keys are `native` (Ostra's own agent loop) and one
  per harness: `claude` (Claude Code), `codex` (Codex), `grok` (Grok Build), and `agy` (Antigravity). The
  initializer is the one agent that differs: it runs at `xhigh` on Codex and Grok and at `max` on
  Antigravity. A workspace can override effort per agent or per phase complexity in its routing settings.

The standard plugin's manifest is built once and cached. A malformed `agent.toml` is caught by the crate's
tests, so it is a build defect and never a runtime error a user could hit.

## Result contracts and grants

Built-in and custom agents follow one model, and the engine never asks which agent it is dealing with. It asks
two other questions.

**What does the result look like?** Every agent declares a result contract (`returns`, Rule CA5): `research`,
`spec`, `fact-check`, `plan`, `implementation`, `review`, `path-analysis`, `tests`, `documentation`,
`architecture`, `prompt`, `setup`, `answer`, `advice`, `stage`, or a plugin's own `<plugin>:<contract>`
([`contract.rs`](../../crates/ostra-core/src/contract.rs)). The submit schema and its checks, the spawn block,
the skills the brief lists, and whether a report file must exist before the submit (`Contract::report_required`,
false only for `review`, whose ledger exists only when it found something) all follow the contract. A built-in
stage runs any agent whose contract it reads, so a team can write its own spec writer and bind it to the spec
stage (see [workflows](workflows.md)). Each run records its contract when it starts.

**What may it touch?** Every integration is a capability any agent may request (Rule CA6). None is reserved:

| Capability | Grants |
| --- | --- |
| `document_research`, `document_spec`, `document_plan` | The Document tool for that typed document, and the right to write it. |
| `review_ledger` | Writing a review loop's ledger, which the engine counts to cap the loop. |
| `security_block` | Writing the security block file of BLOCKER findings. |
| `progress_log` | Writing the implementer progress log that re-runs read. |
| `test_files` | Writing test files and directories in the repo. |
| `manage_projects` | The project tools (`ProjectList`, `ProjectCreate`). |

The standard agents hold exactly the grants their jobs need: `explore`, `generate-spec`, and `plan` hold their
document grant; the implementer holds `review_ledger`, `progress_log`, and `manage_projects` but not
`test_files`; `write-test`, `prompt-generation`, and the initializer hold `test_files`. The guards check these
grants, so a custom agent that requests one gets the same access. That is safe because every agent file and
plugin waits for the user's approval of the workspace file (Rule A1).

Every definition, whatever wrote it, becomes an `AgentDef` in one function, `catalog::from_plugin_agent`
([`catalog.rs`](../../crates/ostra-agents/src/catalog.rs)). Origin changes only the name: the standard plugin's
agents get the built-in names, and every other source names a custom agent.

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

Up to five sections are prepended to the rendered body (`render_def` in `crates/ostra-agents/src/lib.rs`),
in this order from the top: the tool vocabulary on a harness, the output rule, the messaging guide
(`assets/coordination.md`) for an agent with the `coordinate` capability, the stage guide
(`assets/custom-agent.md`) for an agent that returns `stage`, and the code tools guide. The three below need the
most explanation:

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

Code that spawns a plan agent without a spec file does not compile. In Ultracode, the plugin Ostra is ported
from, the same contract was a JSON file checked by a hook at spawn time, so a missing parameter surfaced as a
failed run. Here it surfaces as a build error.

The struct also enforces what an agent must *not* see. `PlanParams` has no field for research documents,
because Rule D4 says the plan takes its requirements from the approved spec alone. A requirement that did not make
it into the spec cannot leak into the plan, since there is nowhere to put it. What research found about the code
arrives as `code_facts`, a file the engine writes with files, symbols, patterns, and flows, and no request text
(Rule D4a). `FactCheckParams` carries research documents only on a spec target, for the same reason (Rule D5),
and the code facts file on a plan target.

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
  never a plain retry. After an implementer the user sent to fix the cause (Rule O8), the fact is that
  implementer's summary, report, and changed files.
- An implementer sent to a stuck run gets `No plan:` and an `Unblock:` line with the stuck run's diagnostic,
  need, and the user's instructions, plus the phase file under `Context files:`. Its report is
  `ostra-implementer-unblock-phase-<N>-<round>.md`, so it never overwrites the stuck run's report or progress
  log.

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

Both executors also refuse an `ok` submit while the declared report file does not exist, and tell the agent to
write it first. The `review` contract is exempt, because its ledger exists only when the review found something.

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
- **implementer** holds the `manage_projects` capability by default. Any agent that holds it and runs a phase
  in a project the plan names as new gets the same treatment (`creates_project` in `runner.rs`, the guard in
  `guards.rs`). The implementer of that first phase
  gets a `New project:` spawn line and its session dir as `Repo root:`, reads the phase file, and calls
  `ProjectCreate` with those facts and nothing else (rule O2). Ostra stops the run once the project exists,
  initializes it, and starts the phase again inside it. If the user denies the call, the implementer returns
  stuck with the refusal as its need.

### What the advisor is given

The engine keeps little context about why a step failed, so the advisor's spawn carries what the step saw and
did, built by `advisor_request` in [`init.rs`](../../crates/ostra-engine/src/init.rs):

| Line | Content |
| --- | --- |
| `Failed step:` | The agent and mode, such as `initializer detect`, or the agent alone, such as `write-test`, for a stuck build or test run |
| `Problem:` | The error, the stuck report as its summary and then what it needs, or the engine's own finding (no slices, no skills, a missing inventory) |
| `Step inputs:` | The failed run's own spawn block |
| `Step result:` | The failed run's submit payload as JSON, cut at 8,000 characters, because a step can submit `ok` with a result Ostra cannot use |
| `Step context:` | What the step is for: the created project's stack, purpose, and base requirements, or the phase, its file, and whether the run was in the build or the test loop |
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

Each agent gets the sections its definition names in `brief` (Rule CA6), so the choice is data, not code. The
standard agents ask for these:

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

The reviewer is the only standard agent that asks for the complete rule catalog (`review`), because it is the
only one that grades against it. The skills section is filtered by the agent's result contract: a `tests` agent
sees test skills and conventions, a `review` agent conventions only, and every other agent all skills.

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
`submit_implementer`. The tool's input schema comes from the agent's result contract (Rule CA5): a built-in
contract's schema is a Rust struct in `crates/ostra-core/src/submit.rs`, converted to JSON Schema; the `stage`
contract adds the agent's declared `data`; a plugin contract's schema comes from the plugin's manifest. Here is the fact-check return:

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

When an agent calls its submit tool, the native and harness executors run the same checks before accepting it
(`Run::submit` in `ostra-exec-native`, the submit branch of `HarnessBridge` in `ostra-exec-harness`). A
programmatic agent's result is checked for shape and for its report file, but not for documents:

1. **Shape.** `validate_submit_with` parses the input into the struct of the run's contract
   (`ExecContext.contract`), or checks it against a plugin contract's schema. A missing field or a wrong enum value
   comes back to the agent as a tool error with the parser's message and "Fix it and call submit again."
2. **Consistency.** Some rules cross fields. The reviewer's `security_block` must be true exactly when a
   BLOCKER finding is present, so a reviewer cannot report a BLOCKER while claiming nothing is blocked, or the
   reverse.
3. **Documents.** For the `research`, `spec`, and `plan` contracts, the referenced document is opened and
   checked (`doc::check_submit`). The
   counts the agent reported, such as the number of requirements or evidence rows, must match what the file
   contains. An agent cannot submit a spec summary that differs from the spec it wrote.
4. **Report file.** An `ok` submit from an agent with a declared report path is refused until that file
   exists, except for the `review` contract, whose ledger exists only when the review found something.

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

## Custom agents

A workspace adds its own agents beside the built-in ones, for checks or steps Ostra does not ship: a security
audit, a release-notes writer, a check of the team's own conventions. They run in workflow stages (see
[workflows](workflows.md)) and as helpers other agents start with `SendMessage`. HANDOVER section 9.4 holds the
rules (CA1 to CA6). A custom agent is defined with the same fields as a built-in one and gets the same treatment:
it may return any result contract and request any grant described [above](#result-contracts-and-grants).

### The file

A custom agent is one markdown file, `<workspace>/.ostra/agents/<name>.md`: TOML frontmatter between `+++` lines,
then the agent's instructions.

```markdown
+++
description = "Audits changed files for secrets and unsafe input handling."
default_tier = "balanced"
capabilities = ["read", "search_text", "glob", "shell", "coordinate"]
write_scope = "session"
timeout_seconds = 1200
[data_schema]
type = "object"
required = ["risk"]
[data_schema.properties.risk]
type = "string"
enum = ["low", "medium", "high"]
+++
Read every file the change touched and report each secret with {{ tool_read }} ...
```

Only `description` is required. The rest have defaults:

| Field | Default | Notes |
| --- | --- | --- |
| `name` | the file name without `.md` | Lowercase kebab-case, at most 40 characters, never a built-in agent's name, `module-documentation`, or `judge`. |
| `returns` | `stage` | The result contract (Rule CA5). A built-in contract such as `spec` lets a workflow bind the agent to the built-in stage that reads it; a plugin contract needs a plugin that defines it. |
| `default_tier` | `balanced` | |
| `capabilities` | `read`, `search_text`, `glob`, `report`, `coordinate` | Any capability, grants included (Rule CA6). None is reserved. |
| `write_scope` | `project` when the capabilities include `write` or `edit`, else `session` | `read_only`, `session`, `project`, or `setup`. `read_only` together with `write` or `edit` is refused. |
| `brief` | `stack`, `commands`, `skills`, `conventions`, `modules` | Sections of the repo brief, from those and `testing` and `review`. |
| `timeout_seconds` | 1200 | At most 7200. |
| `[effort]` | high on every executor | Keys are `native`, `claude`, `codex`, `grok`, `agy`. |
| `data_schema` | none | The shape of the submit's `data`, as a table. Only an agent that returns `stage` may declare one. |
| `helper` | false | True lets `SendMessage` start the agent as a helper. |

The file may be at most 128 KiB, because the whole body goes into every run's system prompt. The body is
rendered like a built-in prompt, with the same `{{ tool_* }}` tokens, the same code-tools and messaging guides
when the capabilities call for them, and the same harness vocabulary. An agent that returns
`stage` also gets `assets/custom-agent.md` before the body, a short guide to the stage contract and the submit
call; an agent that returns a built-in contract describes that contract's fields in its own instructions, as the
built-in prompts do. The body is rendered as a check when
the file loads, so a broken template token is a settings error, never a failure in the middle of a session, and
rendered again for each run.

### The catalog

`AgentCatalog` (`crates/ostra-agents/src/catalog.rs`) is the set of agents one workspace can run: the standard
plugin's agents, the valid files of `.ostra/agents/`, and the agents of the workspace's plugins. A markdown file
goes through `ostra_sdk::definition::parse_markdown` and then `from_plugin_agent`, like every other source.
`AgentCatalog::load` also takes the result contracts the workspace's plugins define, with their schemas: an
agent that returns one gets that schema for its submit, and one that returns a contract no plugin defines is
left out and reported (Rule PL5). Like the
settings, it is read again for every execution, so an edited agent applies to its next run without a restart.
A file that does not parse, or a second agent with a name already taken, is left out and reported as a settings
issue under `agents`, with the file it came from.

`AgentName` stays one type for both kinds: built-in agents are its variants, and a custom agent is
`AgentName::Custom`, whose name is interned so the type keeps its copy semantics. On the wire it is the bare name
either way, so events and databases did not change shape.

### Where an agent writes

`write_scope` is enforced by the write-scope guard (`check_scope` in `crates/ostra-policy/src/guards.rs`, through
`ExecContext::scope`), for built-in and custom agents alike. `read_only` writes nothing, `session` writes its
session dir and the OS temp dir, `project` writes the repo root and its session dir but not the temp dir, and
`setup` writes the project's `.ostra/` runtime and its skills dir (the initializer's scope). A spec, a plan, a
research document, the review ledger, the security block file, the progress log, and test files need their grant,
whatever the agent's scope. No agent can write `.ostra/agents`, `.ostra/workflows`, or `.ostra/transforms`, because an
agent that could would decide which agents, stages, and transforms run after it.

### What the stage contract holds

An agent that returns `stage`, the default, ends with `CustomSubmit` in `crates/ostra-core/src/submit.rs`:

| Field | Meaning |
| --- | --- |
| `verdict` | `pass`, `fail`, or `needs_user`. A workflow stage moves on, follows its `on_fail`, or asks you. |
| `summary` | What the run did and found. Later stages and you read it. Required and non-empty. |
| `findings` | Each problem, with its file and the fix. |
| `question`, `options` | For `needs_user`: the decision needed, and the answers to offer, recommended first. `question` is required then. |
| `report_path` | A report the run wrote, when it wrote one. |
| `data` | Output in the shape `data_schema` declares. Required when one is declared. |

`data` is checked against the declared schema by a small JSON Schema subset in
`crates/ostra-core/src/schema_check.rs`: `type`, `properties`, `required`, `items`, `enum`, `minItems`,
`maxItems`, and `additionalProperties: false`. Other keywords load but check nothing. The run's submit tool shows
the model the full schema, and both executors validate against the schema the run was given, so a mismatch is a
tool error the model can fix before anything is recorded.

### Routing

No agent needs a settings entry (Rule CA4). Its route is `routing.*.byAgent.<name>` when the workspace sets one,
else its `default_tier` on its executor (`resolve_route` in `crates/ostra-core/src/config.rs`). Settings
validation accepts route, effort, and MCP `agents` entries that name any agent of the catalog.

### Agents from plugins

A plugin (see [plugins](plugins.md)) defines agents in its manifest. One with a `prompt` runs on a model exactly
like a markdown agent. One without a prompt is programmatic: the plugin's own code does the work, calling tools
and the model through Ostra (Rule PL2, described in [executors](executors.md)). Its description stands in for
the prompt when one is rendered. A plugin's agent may return a contract the plugin defines, whose results the
plugin handles (Rule PL5).

### Approval

Agent files arrive with a workspace folder the same way MCP servers in `workspace.toml` do, so they wait for
your approval the same way (Rule A1). The hash you approve for the workspace file covers each file in
`.ostra/agents/`, `.ostra/workflows/`, and `.ostra/transforms/` by name and content hash (`definition_files` in
`crates/ostra-workspace/src/trust.rs`), together with `[[plugins]]`. Until you approve, the catalog loads the
built-in agents and the plugins built into the binary only, Settings lists each file as waiting, and a session
that names a workflow from those files is refused. An agent file you add or edit outside Ostra puts the
workspace file back into waiting.

### The agent screen

The console lists every agent the workspace can run and edits the workspace's own files (HANDOVER section 9.5,
Rules AG1 to AG3). Its server side is in
[`crates/ostra-workspace/src/builder.rs`](../../crates/ostra-workspace/src/builder.rs).

`WorkspaceDetail.agents` gives each agent's `AgentInfo`, which says where the definition comes from (`source`:
`ostra`, `workspace` with its file under `.ostra/agents/`, or `plugin` with the plugin's name), the contract it
`returns`, its `write_scope`, and whether it is a `helper` or `programmatic`, besides its routes.

| Request | What it does |
| --- | --- |
| `GET /api/workspaces/:ws/agents/:name` | An `AgentDetail`: the `AgentInfo`, the definition as an `AgentDoc`, whether it is `editable`, the system prompt as the native executor renders it (`prompt_preview`), its submit schema, and the workflow nodes that run it or bind it (`used_by`, as `workflow/node`). |
| `PUT /api/workspaces/:ws/agents/:name` | Save a workspace agent from an `AgentDoc`. |
| `DELETE /api/workspaces/:ws/agents/:name` | Delete a workspace agent's file. |

Only a workspace agent's file is `editable`. Ostra's and a plugin's agents are read only; their `AgentDoc` is
filled from the definition, so the console can duplicate one into a new workspace agent.

The editor builds `data_schema` as a list of fields, because most output shapes are a few named values and
hand-written JSON Schema is easy to get wrong. Each field has a name, a type (`string`, `number`, `integer`,
`boolean`, `object`, or `array`), a required flag, and a description; a string can list its allowed values
(`enum`), an object holds its own fields and can refuse others (`additionalProperties: false`), and an array
picks its item type, which nests the same way, and its `minItems` and `maxItems`. These are the keywords
`schema_check` enforces, so everything the builder writes is checked. The editor refuses to save while a field
has no name or two fields of one object share a name. A JSON tab edits the same schema as text. A schema that
uses anything else, such as a list of types or a `title`, opens in the JSON tab and stays there, so the builder
never drops a keyword it cannot show. An empty builder saves no schema.

A save writes the file a user would write (`agent_markdown`): TOML frontmatter between `+++` lines, then the
prompt. Defaults are left out (`returns = "stage"`, an empty capability list, `helper = false`), and the tables
(`effort`, `data_schema`) come last. Before anything is written, the text goes through the same
`parse_markdown` a load uses (Rule CA1), so a built-in agent's name or a broken template token is refused. A
save is also refused when a plugin already holds the name (`AgentCatalog::with_workspace_def`), when `returns`
names a plugin contract no running plugin defines, and when the changed agent would stop a workflow that runs
now from running, for example by changing the contract a workflow binds it to. A delete is refused while a
workflow uses the agent. Both write through `trust::save_definitions`, so an approved workspace stays approved
(Rule A1).

An agent whose file waits for approval is out of the catalog, so no session runs it, but its page still reads
the file and shows it with `waiting_approval`.

## Subagents that talk to each other

Reports carry results from one stage to the next, but a report read cold loses what its writer knew: why a
requirement is worded the way it is, which files the implementer already ruled out, what a finding refers to.
Re-reading everything costs tokens, and a judge cannot fill the gap either, because it never saw the inside of
either conversation. So agents can also message each other directly, and every agent keeps its conversation so
that it can be woken again. HANDOVER section 10.8 holds the rules (SM1 to SM9 for messages, H5 to H7 for pair
loops).

### A subagent ID names a conversation

Each execution is one run of a model. A **subagent** is a conversation that may span several runs, and its
subagent ID is the id of the conversation's first execution. A run keeps the ID of the conversation it continues,
whether it continues in place (the same execution resumed) or as a new execution whose `resumes` field names the
run before it. The fold keeps a map from each execution to its subagent (`SessionState::subagents`), and the
subagent's **head** is its latest run. A message to a subagent goes to its head. A run cannot message its own
subagent.

### The three tools

Agents with the `coordinate` capability get three tools, and a prompt section (`assets/coordination.md`) that says
when to use them. Among the built-in agents these are explore, generate-spec, fact-check, plan, implementer,
code-reviewer, and write-test; every custom agent has the capability unless its definition leaves it out.

| Tool | What it does |
| --- | --- |
| `ListAgents` | Your own subagent ID and every subagent of the session, with its agent, status, report, and what it waits on, then the helper agents you can start. The subagents you work with are marked, such as "the author of the document you check" or "waits for your reply". |
| `SendMessage` | Queue a message. With `to` it goes to an existing subagent; with `agent` Ostra starts a new helper with the message as its task. With `wait: true` your run pauses after sending. |
| `WaitForMessage` | Pause your run until a message arrives for you. |

The tools never ask you for permission in any mode, because they change no file; a deny rule can still refuse
them. The engine checks each call against the fold and records it as an event before the tool returns
(`Engine::coordinate` in `crates/ostra-engine/src/runner.rs`, the checks in `crates/ostra-engine/src/coord.rs`).

### Messages are queued

A message never lands in the middle of a model request, because text spliced into a request in flight would
break the receiver's turn and its prompt cache. Ostra keeps it in the fold and hands it over at the receiver's
next turn boundary (Rule SM2):

- **A running native run** reads its messages after its current turn's tool results, in the same user turn and
  after the cached prefix, or in place of the reminder after a turn without tool calls.
- **A running harness run** reads them once its turn ends: the Stop is let through, the process stays up, and
  Ostra types the messages into its terminal.
- **A programmatic agent** reads them after its current tool call, appended to the call's output.
- **A waiting run** is woken with them.
- **A subagent whose run ended** `ok`, `stuck`, or `handoff` is continued for them (below).

[Executors](executors.md) describes how each executor does this.

### Pausing for a message

A run pauses with `wait: true` on `SendMessage`, or with `WaitForMessage` (Rule SM3). How it waits depends on the
executor:

- **Native.** The loop ends the run with status `waiting`, as a submit ends it. The fold treats the run's stage as
  paused, and the planner holds the stage's next spawn. When a message is ready, the planner resumes the same
  execution in place, and the loop replays the stored messages with the new messages as the next user turn. The
  prefix is the same, so the provider's prompt cache still covers it.
- **Harness and programmatic.** The process stays alive. A harness is told to end its turn and reply only
  `Waiting`; its Stop is let through. While it waits it gives back its execution slot, and the time does not count
  against its timeout. When a message arrives, Ostra types it into the terminal, or returns it from the
  programmatic agent's tool call. If the server restarts during a harness wait, recovery records the run as
  `waiting` rather than interrupted, and the message later resumes the harness session from its session id.

Freeing the slot matters: with `max_parallel_executions = 1`, a waiting run that kept its slot would wait forever
for a run that can never start.

No run waits forever. A run that waits on a subagent hears when that subagent ends its run without messaging it,
unless a message already queued for that subagent will continue it. A run that waits for any message hears when
no other subagent of the session is running or waiting and no message is queued for one. Either way the run is
woken with a notice saying so, and continues without a message.

### Where a message goes

A message to a helper whose contract is `research`, such as `explore`, becomes a research task like the ones
classification starts, run by that helper and tagged with the message (`ExploreOrigin::Ask`). The `explore` name
matters only for a log written before contracts, whose helper targets carry none. It does not hold the research stage, because only the sender waits for it. Any
other agent whose definition sets `helper = true` is started as a helper run (purpose `Helper`) with the message
as its task. Either way, the helper's submit comes back to the sender as a result message: for explore the
findings summary, the research document, and what it did not cover; for a custom agent its verdict, summary,
findings, report, and data. An explore helper that errors is retried once before its failure becomes the result.
A result for a sender whose run already ended is dropped, because nothing waits for it.

A message with `to` goes to the subagent's head:

1. **It is running**: the message waits for its next turn boundary.
2. **It waits**: it is woken with the message.
3. **Its last run ended `ok`, `stuck`, or `handoff`**: Ostra continues its conversation in a new run (purpose
   `Message`) with the messages as its new turn (Rule SM4). The run keeps the subagent's agent, tools, executor,
   model, report path, and stage, so an implementer continued this way can edit files to make the fix it was
   asked for. It ends with its submit call, which the pipeline records but does not read again.
4. **It failed, or its conversation already holds six runs**: it takes no message, and `SendMessage` refuses with
   the reason, so nothing is sent that no one reads.

The reviewer and implementer show how this fits together. A reviewer finds a problem, sends the implementer
"Fix X, then tell me" with `wait: true`, and pauses. The implementer's run had ended, so Ostra continues it with
the message. It fixes the file and sends the reviewer a reply, which wakes the reviewer in place, and the
reviewer checks again and submits its review.

### Replies are owed

A sender that waits for a reply is owed one (Rule SM6). When a run reads a message whose sender still waits on
its subagent, it may not submit until it sends that sender a message. Both executors ask the engine before they
accept a submit (`submit_blocked`), and every reminder names `SendMessage` and the sender's ID: the native loop's
reminder after a turn with no tool call, a harness's turned-back Stop, and the nudge Ostra types into a quiet
terminal. The coordination evals found the need for this: a run that answered in text was reminded only to
submit, so its sender never heard back.

### The pair loops continue conversations

The pipeline's loops between two agents use the same conversations, driven by the engine instead of a tool call,
because the engine holds the gates. When a fact-check fails, the next spec or plan round continues the author's
conversation; the next fact-check pass continues the checker's. When a review has findings, the fix continues
the phase's last worker, and the re-review continues the reviewer. A rescue after `stuck`, a resume after a
handoff, and the next round of a workflow stage continue their worker too. The fold decides this
(`SessionState::continuation`), and the planner marks the spawn: fixtures show it as
`spawn generate-spec spec#2 (continues)`.

A continued run's new turn is a fixed header followed by the new spawn block, which carries what the round needs:
the findings, the answers, the prior findings of a re-pass. Pass or fail, the review cap, and the recurring
fact-check gate work exactly as before; only the input changes. The run stays on the executor and model its
conversation started on.

A loop starts a fresh run instead when continuing would be wrong or impossible: the previous run did not end `ok`
with a submit, the agent was moved to another executor after a harness failure, a harness run left no session id
to resume, you amended the request after the previous run started, or the conversation already holds six runs
(`MAX_CONVERSATION_RUNS`), because a long conversation costs more per turn than a fresh start. A conversation
never has two live runs: a continuation waits while another run of the same subagent is live (Rule H7).

### Limits

Every message can start or wake a run, so messages are bounded (Rule SM5). A session's agents may send at most 48
messages (`MAX_SESSION_MESSAGES`), and a run may start at most three helpers (`MAX_HELPERS_PER_RUN`). A helper may
not start helpers. Helpers and continued runs go through the slot limiter and the budget guard like every other
spawn, and a session does not complete while a message waits for a receiver that can still take it, or a subagent
waits (Rule SM9).

### What the log records

Each step is an event (Rule SM8): `MessageSent` when a run sends, `AgentWaiting` when it waits without sending,
and `MessagesDelivered` when Ostra hands messages to a run, with the notice when no message will come. A helper's
start is its message's delivery. The fold derives everything else, including each helper's result message, so a
replay rebuilds exactly who waits for what. Logs written before messaging still fold: see
[the event log](event-log.md).

### Measuring messaging

Conformance fixtures prove where the engine routes a message; they cannot say whether a model messages the right
subagent, answers from its conversation, or holds back when a message is not needed. The coordination evals do
that. [`tests/evals/coordination.toml`](../../tests/evals/coordination.toml) holds cases in three tiers, all set in
Ostra's own source. A scripted `ask` sends with `wait: true`, and a scripted `reply` sends to the subagent that
waits:

1. **Answering.** A message run (an ended subagent continued for a message) explains a number only its
   conversation holds, declines to change the spec to a number it can justify even though it has the tools to
   edit it, and reads code it never discussed to answer correctly. A spec author woken by its helper's question
   replies from its conversation and goes back to waiting.
2. **Deciding.** An implementer, which has no web tools, needs a model id published last week and should start an
   explore helper instead of guessing it. A fact-checker finds the spec author among three subagents with
   `ListAgents` and messages it. Two controls, a fully traceable spec and a one-file rename, must message no one.
3. **Loops.** A live fact-checker fails a false claim and its second pass continues its conversation; a live
   reviewer finds a planted bug, the live implementer fixes it in its own continued conversation, and the reviewer
   re-reviews; a live helper confirms its scope with the live author before it researches.

[`crates/ostra-server/tests/coordination_evals.rs`](../../crates/ostra-server/tests/coordination_evals.rs) runs
each case as a real session of a real `Engine`, on a git clone of a snapshot of Ostra's working tree that leaves
out the eval itself, so no agent can read the expected answers. Judges are scripted. A router executor sends each
run the case lists as live to the native loop on the model under test, with the policy, the sandbox, a code index
of the snapshot, and messaging tools wired to that engine, and plays every other run from the case: what an
author said, the file it wrote, the message a checker sent. So every wake, message run, continuation, and freed
slot goes through the engine's own code, and only the runs under test cost tokens. In grading, a message that
pauses its sender or starts a helper counts as a question, and any other message counts as an answer.

A run passes when every code check holds (who messaged whom and what, the replies, statuses, submits, files,
continuations, cache reads) and a grader model finds the rubric met, reading a record of every run's tool calls,
every question and answer, the live submits, and the diff. The report gives pass rates per tier and model, cost,
and the share of live input tokens read from the prompt cache. A session whose live run died on a provider error
runs again, up to twice, and is reported apart from the pass rates. An offline test replays every case with
stand-ins for its live runs and checks that each live run is reached, the session stops where the case says, and
every expected continuation continues its conversation, so a broken case fails in the normal suite.

The harness side of rule SM3, a CLI waiting with its process alive and the message typed into its terminal, is
checked live by `harness_probe wake`: the agent starts a helper twice with `wait: true`, and must submit both
results.

## Where to look in the code

| What | Where |
| --- | --- |
| Agent definitions and prompts | `assets/agents/<name>/` |
| Tool names per executor | `assets/tool-mapping.toml` |
| Loading definitions and rendering prompts | `crates/ostra-agents/src/lib.rs`, `mapping.rs` |
| The standard plugin of Ostra's own agents and default workflows | `crates/ostra-default-plugin/src/lib.rs` |
| Definition files (markdown and `agent.toml`) | `crates/ostra-sdk/src/definition.rs` |
| Custom agents and the catalog | `crates/ostra-agents/src/catalog.rs`, `assets/custom-agent.md` |
| The agent screen's reads and saves | `crates/ostra-workspace/src/builder.rs` |
| Result contracts | `crates/ostra-core/src/contract.rs` |
| The custom submit and its `data` check | `crates/ostra-core/src/submit.rs` (`CustomSubmit`), `crates/ostra-core/src/schema_check.rs` |
| Spawn structs and the block parser, by contract | `crates/ostra-agents/src/spawn.rs` |
| The repo brief | `crates/ostra-agents/src/brief.rs` |
| Planner inputs to spawn structs | `crates/ostra-engine/src/factory.rs` |
| Submit schemas and `validate_submit_with`, by contract | `crates/ostra-core/src/submit.rs` |
| Submit document checks | `crates/ostra-core/src/doc/check.rs` |
| Messages: tool inputs and limits | `crates/ostra-core/src/coord.rs` |
| Messages in the fold: queues, waits, deliveries, continuations | `crates/ostra-engine/src/coord.rs` |
| The messaging prompt section | `assets/coordination.md` |
| Coordination evals | `tests/evals/coordination.toml`, `crates/ostra-server/tests/coordination_evals.rs` |
| Test stage evals | `tests/evals/test_stage.toml`, `tests/evals/test_stage/`, `crates/ostra-server/tests/test_stage_evals.rs` |
| Planning evals | `tests/evals/planning.toml`, `tests/evals/planning/research/`, `tests/evals/planning/results/`, `crates/ostra-server/tests/planning_evals.rs` |
| Report file names | `crates/ostra-core/src/paths.rs` (`report`) |
| Submit handling, native | `crates/ostra-exec-native/src/lib.rs` |
| Submit handling, harness | `crates/ostra-exec-harness/src/bridge.rs`, `live.rs` |
