# Your first session

This page follows one task from start to end, in the order in which you see each screen. When a stage shows,
the page explains its purpose. The reason is that Ostra teaches the process, and it also runs the process.
Before you start, the server must run and you must be signed in ([Quick start](quick-start.md)).

## Create a workspace

A **workspace** is a folder that groups the projects that you work on. It has one set of settings and one event
log. A **project** is one code folder in the workspace, usually one git repository. A workspace can hold many
projects, and one task can change more than one of them.

On a machine with no workspace, `/` opens the setup guide:

1. **Machine check.** Ostra lists the provider keys that it can see, from the environment, the OS keychain, or
   the registry. It also lists the harness CLIs on your `PATH`, and whether each one is signed in. You need at
   least one working provider. You can paste a key here. Ostra seals the key in its registry, never in a config
   file.
2. **Name and folder.** The folder does not have to hold your code. Ostra writes `.ostra/workspace.toml` and
   `.ostra/workspace.db` there. It writes the files of each session under `.ostra/sessions/`.
3. **Projects.** Add existing folders, or clone a git URL. A clone goes into `<workspace>/<key>` or into a folder
   that you choose. You can skip this step and add projects later from the workspace menu.
4. **Defaults.** You set these items:
   - The permission mode: ask before edits and unlisted commands (the default), accept edits in the project, or
     read-only. You set the mode that never asks later, under Settings, Permissions.
   - The executor and the model that each agent uses.
   - Whether new sessions start in YOLO mode. Keep YOLO off for a first session, because then you see each gate.
5. **Review.** Ostra validates the full request before it creates anything. Thus Ostra finds a route that names a
   model that no provider serves at this step, not in the middle of a session.
   [Settings and routing](../internals/settings-and-routing.md) explains the routing table.

## Initialize the project

A project cannot be the target of a task until you initialize it. The reason is that each agent reads the
inventory of the project to learn its commands, conventions, and skills. Until then, the project screen shows
**Initialize**.

Initialization is a small pipeline of its own:

```
detect → scout ×N (parallel, at most 12) → propose → skill approval (gate)
       → generate-skill ×N (parallel) → generate-inventory → done
```

- **Detect** reads what the project already has:
  - Skills in `.agents/skills/`.
  - Instruction files, for example `CLAUDE.md` or `AGENTS.md`.
  - An earlier `project.toml`.

  Detect divides the code into slices to scout. It skips component types that an existing skill already teaches.
  It also detects the commands and the test types. A test type is each type of test that the project runs (unit,
  integration, end to end), with its command and its requirements.
- **Scouts** each read one slice and report the patterns that they find. For example, a scout reports how this
  codebase writes a handler, a model, or a test.
- **Propose** makes a list of skills from the reports. A skill is a short document that teaches an agent to write
  a given type of code in the style of this project.
- **Skill approval** is your first gate. It is a table with one row for each proposed skill. For each skill, you
  choose generate, regenerate, reuse, or drop. The defaults are sensible. Read the reasons and accept.
- **Generate** writes each approved skill to `.agents/skills/<name>/SKILL.md`. Then it writes the inventory to
  `.ostra/INVENTORY.md` and the profile to `.ostra/project.toml`.

When initialization finishes, review the commands on the Overview tab of the project. Agents run the build, test,
format, lint, and typecheck commands to check their own work. If the test command is wrong, each phase fails to
verify. The test types in `project.toml` are the levels at which the test stage can verify. If the detect step
missed a test type, add it there. You can edit these items there, and the next execution reads them fresh.
[Project memory](../internals/project-memory.md) covers the lessons that each project keeps in `.ostra/memory/`.

Commit `.ostra/` and `.agents/skills/` with the project. They are plain files. The next person who opens the
project in Ostra does not have to initialize it.

## Start a task

The workspace screen has a **New task** form, not a chat box. Write the request as a ticket: tell what must
change and how you will know that it works. To tag a file or folder from an initialized project, type `@`, or
drag the item from the Files tab. Add a few words about the purpose of each tagged file, because agents use that
note to decide how to read it. One request can have up to 50 tags and 20 uploads.

The form has three toggles below the text:

- **Tests** and **Docs** answer the closing gate before it opens. To get the question at the end, keep them off.
  When **Docs** is on, a picker chooses the documentation book that the session writes into. The default is the
  book with the name of the projects of the session. Ostra creates this book on the first run.
- **YOLO** lets a judge model answer the gates for you. It never answers a budget gate, and it never waives a
  security finding. Keep it off for now.

If you have more than one initialized project, you can pin projects below the text. A pinned session works only
in the pinned projects. Research, feedback, and plan phases cannot reach other projects. To let Classify choose
the scope, keep all projects unpinned.

When you start the task, the session board opens. Its header shows your request in an **Original request**
section, with the line breaks that you typed.

## Watch the board

The board has one lane for each stage, in the order of a standard software lifecycle: Research, Requirements,
Verification, Design, Build, Review, Test, Docs, Done. Each lane has a short note that tells why the stage exists.
The cards move across the board when the engine starts and finishes work.

The first event is a **judge call**. Classify reads the request and chooses a category:

| Category | Path |
| --- | --- |
| IMPLEMENT | Research, then the light track (phases built from the research) or the full track (spec, fact-check, and plan if the stakes are not low). Then phases, review, your feedback rounds, closing, tests, and docs. |
| PLAN | Research, spec, and plan, then stop. |
| SPEC | Research and spec. |
| RESEARCH | Research only. |
| VERIFY | One implementer run of the test command. |
| TEST | Only the test stage, for code that exists: a verification plan, tests at each level that it needs, and review. |
| QUICK CHANGE | One implementer pass, with no spec or review, for an edit that the request fully describes. |
| QUICK ANSWER | Goes to the quick-question panel. |

The Decisions tab lists each judge call as "Ostra chose X because Y", with an override.
[Gates and judges](../internals/gates-and-judges.md) lists each judge and what it can decide. If Classify chose
QUICK CHANGE and you want the full path, override the decision there.

For an IMPLEMENT task, you then see these steps:

1. **Explore.** One researcher for each project or area runs in parallel and writes a research document. A
   Sufficiency judge reads each "Not covered" item. If one of the items is important, the judge asks for another
   explore run. The Track judge then chooses one of two tracks:
   - The light track goes directly to step 8.
   - The full track runs steps 2 to 7 first.

   The Track selector on the New task form skips the judge.
2. **Spec.** The spec agent writes requirements in EARS form ("When X, the system shall Y") with acceptance
   criteria. It uses all the research documents. The spec is the contract that each later stage checks against.
3. **Open questions.** If the spec has questions that only you can answer, a gate lists them. The recommended
   option is first. The options have numbers. Other takes a typed answer that can combine them, for example "1
   and 3, plus an audit log". A judge reads your answers first, and it does these things:
   - It runs research when you ask for it.
   - It keeps an answer that applies only to a later stage for that stage.
   - It drops an answer that you tell it to ignore.
   - It forgets a kept note that you take back.

   The other answers run the spec agent again. Ostra never edits the spec itself.
4. **Fact-check.** A separate agent checks each claim in the spec against the code and the sources that the spec
   cites. A FAIL sends the findings back to the spec agent. A PASS unlocks approval.
5. **Spec approval.** The approve button stays disabled until the fact-check passes. To reject the spec, you must
   write a note that tells what to change.
6. **Stakes.** A judge rates the change as low, medium, or high. Low skips the plan.
7. **Plan, fact-check, plan approval.** The plan divides the work into phases. Each phase has steps, the skills
   that it needs, and a test policy. The plan also has a dependency graph between phases. The Overview tab shows
   that graph.
8. **Phases.** Each phase runs an implementer, then a code reviewer, in a loop of at most three rounds. Phases
   that depend on no other phase run in parallel across projects. Two phases never run at the same time in one
   project. After a phase passes, Ostra stages its changed files with `git add`.
9. **Implementation review.** When all phases are finished, you try the change. If you send feedback, Ostra
   builds it as a reviewed revision phase and then asks again. When the change is correct, accept it.
10. **Format** runs the format command of the project one time, after you accept.
11. **Closing gate.** You decide whether to verify the change with tests (unit, integration, end to end, and a
    rerun of the existing tests). You also decide whether to update the docs.
12. **Tests and docs**, then the **completion report**.

[The pipeline](../internals/pipeline.md) explains each stage and the rule behind it.
[The planner](../internals/planner.md) explains how the engine decides what runs next.

Click a card to open its execution. The Activity tab streams the tool calls of the agent with their inputs,
diffs, and outputs. It also shows each policy decision next to the rule that made it. A harness execution also
has a Terminal tab with the live CLI. [Executors](../internals/executors.md) explains the difference.

## Answer the gates

A gate is a stop where the engine needs a decision that it does not make alone. Open gates show at the top of
the Overview tab and in the status bar. A first session usually shows these gates:

| Gate | What you decide |
| --- | --- |
| Open questions | Answer each question. Each question needs an answer before the spec runs again. |
| Spec approval, plan approval | Approve, or reject with a note that tells what to change. |
| Permission | An agent wants a tool call that your permissions do not allow. Allow it one time, deny it, or add the suggested rule for the workspace. |
| Review cap | The review loop of a phase got to three rounds with open findings. Allow one more fix pass, with an optional note to the fix agent, or mark the phase blocked. A blocked phase removes each phase that depends on it from the queue. Independent phases continue. |
| Stuck | An agent reported that it cannot continue, and it told what it needs. |
| Implementation review | Accept the result, or tell what to change. Ostra builds and reviews each change, and then the gate opens again. |
| Closing | Tests and docs for each project. |
| Budget reached | The session spent its budget. Increase it by an amount, or stop. |

A BLOCKER from the reviewer shows as a notice, not as a question. It is a security finding. The fix agent removes
it and the review runs again. This loop has no cap, and you cannot waive the finding.

## Add context or pause

The Add context box below the lanes accepts more text or file tags during a session:

- **Queue** lets the running work finish, and then gives the new context to the next step.
- **Send now** interrupts each running execution.

In the two cases, after Ostra classifies the request, the Route answer judge reads the context before a step
starts or runs again. The judge decides on one of these results:

- The context joins the request.
- Ostra keeps the context only as a note for later stages.
- Ostra drops the context, because you said to ignore it.

The judge also decides whether to research first, and in which project. For example, "this is for the new
project, not the backend" sends the research to the new project. The interrupted work then runs again, with the
context next to its task. If the context changes a requirement after the spec exists, the spec and the plan run
again.

Queued context waits when an execution runs. No new work starts until the running work finishes. Until then, the
header shows the context as "Queued, waits for running work" with **Withdraw**. **Withdraw** takes the context
back before a step reads it. A withdrawn addition stays in the header, struck through. You cannot withdraw
context that you sent with **Send now**, because it already restarted the running work. A queued "skip the
research on X" stops only research that did not start. To stop a running research, use Skip or Send now.

## Correct one run

When one agent is stuck on a step, type the fix in the correction box on its execution page. Do not add context
to the full session. **Send now** stops only that run and resumes it in the same conversation. Your correction is
its next message. Thus the run keeps the work that it did. Other runs continue. You cannot withdraw a correction
that you sent to a running run. On a paused session, the box queues the correction until you continue. Until
then, the page shows the correction with **Withdraw**. Ctrl+Enter or ⌘Enter sends it.

**Pause** stops new work from starting and interrupts the running work. You can still answer gates during a
pause. **Continue** resumes each interrupted execution at the point where it stopped. The execution keeps its
place on the board. Its Activity, cost, and conversation continue from before the pause. A paused session stays
paused after a server restart.

## Stop it

**Stop** on the board header ends the session. Ostra cancels each running execution and marks the session as
failed with "Stopped by the user." A stopped session does not resume. When the server is down, run
`ostra stop <session id>` to do the same thing.

**Cancel** on one execution stops only that run. Its gate opens as "You stopped ...". There you retry the run or
abandon the step. Nothing retries a run that you stopped, not even YOLO.

**Skip** is available on a running research, test analysis, docs, or architecture execution. It stops the
execution and lets the session continue without its result, with no gate to answer. If you skip the test
analysis of a phase, Ostra also skips the tests of that phase. You can also give the instruction in Add context
("skip the research on X"). Ostra then drops the research tasks that you name. A queued addition waits for the
running work first. Thus, to stop research that already runs, send the addition with **Send now**. Work, review,
spec, plan, and fact-check runs have only Cancel, because the rules of the pipeline need their results.

## Read what it wrote

Each file that a session writes is under `<workspace>/.ostra/sessions/<session id>/`:

- The research documents, the spec, and the plan are at the session root, because they can apply to more than one
  project.
- Each project gets a subfolder for its reports: `ostra-implementer-phase-<n>.md`, the review ledger
  `ostra-review-ledger-phase-<n>.md`, the test reports, and at the end `ostra-completion.md`.
- `uploads/` holds the files that you attached.
- `.state/` belongs to the engine. No agent can write there.

In the console, the Sessions tree lists these files under the session. The spec and the plan open as rendered
documents with an outline. IDs such as `R3` or `step 2.3` link to the part that defines them. The review ledger
opens as comments on a diff.

A documentation book is not a session file. The engine writes it to `<workspace>/.ostra/docs/<book>/`.
**Documentation** in the workspace menu lists all books. A book opens as pages with a sidebar, search, and a
table of contents. **Export HTML** saves the book as one file that opens without a server.

The completion report lists what changed, which stages ran, which stages Ostra skipped, and why. Ostra stages your
code changes in the git index of each project, but it does not commit them. Thus you do the last review: open the
Git tab, read the staged diff, and commit.
