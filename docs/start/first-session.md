# Your first session

This page walks through one task from start to finish, in the order you meet each screen. It explains what
each stage is for as it appears, because Ostra is built to teach the process as much as to run it. It assumes
the server is running and you are signed in ([Quick start](quick-start.md)).

## Create a workspace

A **workspace** is a folder that groups the projects you work on together, with one set of settings and one
event log. A **project** is one code folder inside it, usually one git repository. A workspace can hold
several projects, and one task can change more than one of them.

On a machine with no workspace, `/` opens the setup guide:

1. **Machine check.** Ostra lists the provider keys it can see (from the environment, the OS keychain, or the
   registry) and the harness CLIs on your `PATH`, with whether each one is signed in. You need at least one
   working provider. You can paste a key here; Ostra seals it in its registry, never in a config file.
2. **Name and folder.** The folder does not need to hold your code. Ostra writes `.ostra/workspace.toml` and
   `.ostra/workspace.db` there, and every session's files under `.ostra/sessions/`.
3. **Projects.** Add existing folders, or clone a git URL. A clone goes into `<workspace>/<key>` or a folder
   you choose. You can skip this and add projects later from the workspace menu.
4. **Defaults.** The permission mode (ask before edits and unlisted commands, the default; accept edits
   inside the project; or read-only), which executor and model each agent uses, and whether new sessions
   start in YOLO mode. The mode that never asks is set later, under Settings, Permissions. Leave YOLO off for a first session, so you see every gate.
5. **Review.** Ostra validates the whole request before it creates anything, so a route that names a model
   no provider serves is caught here and not in the middle of a session.
   [Settings and routing](../internals/settings-and-routing.md) explains the routing table.

## Initialize the project

A project cannot be the target of a task until it is initialized, because every agent reads the project's
inventory to know its commands, conventions, and skills. The project screen shows **Initialize** until then.

Initialization is its own small pipeline:

```
detect → scout ×N (parallel, at most 12) → propose → skill approval (gate)
       → generate-skill ×N (parallel) → generate-inventory → done
```

- **Detect** reads what the project already has: skills in `.agents/skills/`, instruction files such as
  `CLAUDE.md` or `AGENTS.md`, and an earlier `project.toml`. It splits the code into slices worth scouting and
  skips component types an existing skill already teaches.
- **Scouts** each read one slice and report the patterns they find: how a handler, a model, or a test is
  written in this codebase.
- **Propose** turns the reports into a list of skills: short documents that teach an agent to write a given
  kind of code the way this project does.
- **Skill approval** is your first gate. It is a table with one row per proposed skill. For each you choose
  generate, regenerate, reuse, or drop. The defaults are sensible; read the reasons and accept.
- **Generate** writes each approved skill to `.agents/skills/<name>/SKILL.md`, then the inventory to
  `.ostra/INVENTORY.md` and the profile to `.ostra/project.toml`.

Review the commands on the project's Overview tab when it finishes. Build, test, format, lint, and typecheck
are the commands agents run to check their own work; a wrong test command means every phase fails to verify.
You can edit them there, and the next execution reads them fresh.
[Project memory](../internals/project-memory.md) covers the lessons each project keeps in `.ostra/memory/`.

Commit `.ostra/` and `.agents/skills/` with the project. They are plain files, and the next person to open the
project in Ostra skips initialization.

## Start a task

The workspace screen has a **New task** form, not a chat box. Write the request as you would write a ticket:
what should change and how you will know it worked. Type `@` to tag a file or folder from an initialized
project, or drag one in from the Files tab; add a few words on what each tagged file is for, because agents
use that note to decide how to read it. Up to 50 tags and 20 uploads go with one request.

Three toggles sit under the text:

- **Tests** and **Docs** answer the closing gate in advance. Leave them off to be asked at the end.
- **YOLO** lets a judge model answer the gates for you. It never answers a budget gate or waives a security
  finding. Leave it off for now.

Start the task and the session board opens.

## Watch the board

The board has one lane per stage, in the order of a standard software lifecycle: Research, Requirements,
Verification, Design, Build, Review, Test, Docs, Done. Each lane carries a short note on why the stage exists.
Cards move across as the engine starts and finishes work.

The first thing that happens is a **judge call**. Classify reads the request and picks a category:

| Category | Path |
| --- | --- |
| IMPLEMENT | Research, then the light track (phases built from the research) or the full track (spec, fact-check, plan unless low-stakes), then phases, review, your feedback rounds, closing, tests, docs. |
| PLAN | Research, spec, and plan, then stop. |
| SPEC | Research and spec. |
| RESEARCH | Research only. |
| VERIFY | One implementer run of the test command. |
| UNIT TEST | Path analysis, test writing, and review for existing code. |
| QUICK CHANGE | One implementer pass, no spec or review, for an edit the request fully describes. |
| QUICK ANSWER | Sent to the quick-question panel instead. |

The Decisions tab lists every judge call as "Ostra chose X because Y", with an override.
[Gates and judges](../internals/gates-and-judges.md) lists every judge and what it may decide. If Classify picked
QUICK CHANGE and you wanted the full path, override it there.

For an IMPLEMENT task you then see:

1. **Explore.** One researcher per project or area runs in parallel and writes a research document. A
   Sufficiency judge reads every "Not covered" item and asks for another explore run if one of them matters.
   The Track judge then picks the light track, which goes straight to step 8, or the full track, which runs steps
   2 to 7 first. The New task form's Track selector skips the judge.
2. **Spec.** The spec agent writes requirements in EARS form ("When X, the system shall Y") with acceptance
   criteria, from every research document. The spec is the contract every later stage checks against.
3. **Open questions.** If the spec has questions only you can answer, a gate lists them with the recommended
   option first. Every answer re-runs the spec agent. Ostra never edits the spec itself.
4. **Fact-check.** A separate agent checks every claim in the spec against the code and the sources it
   cites. A FAIL sends the findings back to the spec agent; a PASS unlocks approval.
5. **Spec approval.** The approve button stays disabled until the fact-check passes. Rejecting needs a note
   saying what to change.
6. **Stakes.** A judge rates the change low, medium, or high. Low skips the plan.
7. **Plan, fact-check, plan approval.** The plan splits the work into phases, each with steps, the skills it
   needs, and a test policy, and a dependency graph between phases. The Overview tab draws that graph.
8. **Phases.** Each phase runs an implementer, then a code reviewer, in a loop of at most three rounds.
   Phases that depend on nothing run in parallel across projects, never two at once in one project. After a
   phase passes, its changed files are staged with `git add`.
9. **Implementation review.** When every phase has finished, you try the change. Send feedback and Ostra
   builds it as a reviewed revision phase, then asks again; accept when it is right.
10. **Format** runs the project's format command once, after you accept.
11. **Closing gate.** Whether to write tests and docs for what changed.
12. **Tests and docs**, then the **completion report**.

[The pipeline](../internals/pipeline.md) explains each stage and the rule behind it, and
[The planner](../internals/planner.md) how the engine decides what runs next.

Click any card to open its execution. The Activity tab streams the agent's tool calls with their inputs,
diffs, and outputs, and every policy decision beside the rule that made it. A harness execution also has a
Terminal tab with the live CLI. [Executors](../internals/executors.md) explains the difference.

## Answer the gates

A gate is the engine stopping for a decision it will not make alone. Open gates sit at the top of the
Overview tab and in the status bar. The ones a first session is likely to show:

| Gate | What you decide |
| --- | --- |
| Open questions | Answer each question. Every question needs an answer before the spec re-runs. |
| Spec approval, plan approval | Approve, or reject with what to change. |
| Permission | An agent wants a tool call your permissions do not allow. Allow once, deny, or add the suggested rule for the workspace. |
| Review cap | A phase's review loop reached three rounds with findings open. Allow one more fix pass, with an optional note to the fix agent, or mark the phase blocked. A blocked phase takes every phase that depends on it out of the queue; independent phases continue. |
| Stuck | An agent reported it cannot proceed and said what it needs. |
| Implementation review | Accept the result, or describe what to change. Each change is built and reviewed, then the gate returns. |
| Closing | Tests and docs for each project. |
| Budget reached | The session spent its budget. Raise it by an amount, or stop. |

A BLOCKER from the reviewer is shown as a notice, not a question. It is a security finding: the fix agent
removes it and the review runs again, with no cap and no way to waive it.

## Add context or pause

The Add context box under the lanes takes more text or file tags mid-session. **Queue** lets running work
finish and hands the new context to the next step. **Send now** interrupts every running execution and re-runs
each with the updated request. Either way the engine treats it as an amendment to the request, so a
requirement change re-runs the spec and the plan.

**Pause** stops starting new work and interrupts what is running. Gates can still be answered while paused.
**Continue** resumes each interrupted execution where it stopped. The execution keeps its place on the
board, and its Activity, cost, and conversation continue from before the pause. A paused session stays paused
across a server restart.

## Stop it

**Stop** on the board header ends the session: every running execution is cancelled and the session is
marked failed with "Stopped by the user." A stopped session does not resume. With the server down, the same
thing is `ostra stop <session id>`.

## Read what it wrote

Every file a session writes is under `<workspace>/.ostra/sessions/<session id>/`:

- The research documents, the spec, and the plan sit at the session root, because they can span projects.
- Each project gets a subfolder for its reports: `ostra-implementer-phase-<n>.md`, the review ledger
  `ostra-review-ledger-phase-<n>.md`, the test reports, `ostra-module-docs.md`, and at the end
  `ostra-completion.md`.
- `uploads/` holds files you attached.
- `.state/` belongs to the engine. No agent may write there.

In the console, these are listed under the session in the Sessions tree. The spec and plan open as rendered
documents with an outline, and IDs such as `R3` or `step 2.3` link to the part that defines them. The review
ledger opens as comments on a diff.

The completion report lists what changed, which stages ran and which were skipped, and why. Your code changes
are staged in each project's git index and not committed, so the last review is yours: open the Git tab,
read the staged diff, and commit.
