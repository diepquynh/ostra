# Output Contract: INVENTORY.md and project.toml

This file defines the exact structure the initializer's **generate** mode must write, and that the
orchestrator and every subagent read. Both files live in the target repo at `.ostra/`.

**Design principle: route by inventory, not by description.** Models do not reliably route off skill
front-matter `description` fields. So the single source of truth is `INVENTORY.md`, a plain markdown file
that every agent is instructed to **Read** first. Skill discovery is a loading mechanism, not a
routing source. The inventory works the instant it is written because it is just a file.

**Design principle: every skill row carries BOTH a name and a path.** A per-project skill lives in the target
repo at `.agents/skills/{name}/SKILL.md`, the cross-harness standard. A project initialized before that may still
keep some at `.ostra/skills/{name}/SKILL.md`; their rows keep that path. Ostra's skill tool resolves a name to
whichever of the two exists, and every harness executor loads a skill by reading the path, because a path is the one mechanism that works on every harness
(Claude Code resolves names, but Codex and Grok Build subagents need the path). The explicit `path` column is
therefore essential: it is the universal mechanism. Never drop it, and never let an agent guess a path.

---

## 1. INVENTORY.md

Path: `.ostra/INVENTORY.md`. Use this exact section order and table shape.

```markdown
# {Repo Name} Ostra Inventory

Generated: {YYYY-MM-DD} · Stack: {language}/{framework} · Machine profile: `.ostra/project.toml`

> Route work by the tables below, BY NAME. Do not route by skill descriptions.
> When a file type in a task matches a row in "Skill Application Mapping", load the listed skill(s) by
> the `Path` column below, with the skill tool or by reading the file.

## Commands

| Purpose   | Command            |
| --------- | ------------------ |
| build     | {exact string}     |
| test      | {exact string}     |
| test-one  | {exact string with {MODULE}/{TEST} placeholders} |
| format    | {exact string}     |
| lint      | {exact string}     |
| typecheck | {exact string or none}|
| run       | {exact string or none}|

## Skills Inventory

| Skill                | Kind        | Path                                     | Load when (component / file type)           |
| -------------------- | ----------- | ---------------------------------------- | ------------------------------------------- |
| `convention`         | convention  | `.agents/skills/convention/SKILL.md`     | Always. Auto-load for any code edit.        |
| `{component-skill}`  | creation    | `.agents/skills/{component-skill}/SKILL.md` | Creating or modifying a {component type}. |

## Skill Application Mapping

| File type being changed | Skills to load          |
| ----------------------- | ----------------------- |
| {component type}        | `{skill}`, `convention` |

## Module / Area Map

| Path glob                | Area        |
| ------------------------ | ----------- |
| `{glob}`                 | {area name} |

## Review Rule Set

Seeded from the stack reference. IDs are stable; the code-reviewer and orchestrator use them.

| ID  | Rule                                  | Severity | Auto-fixable |
| --- | ------------------------------------- | -------- | ------------ |
| {X1}| {rule}                                | {H/M/L}  | {yes/no}     |
```

**Rules:**
- The **Path** column is mandatory and is how every consumer loads the skill. It must match the same skill's
  `path` in `project.toml` exactly.
- Every skill in the repo's skill set, whether generated this run, reused from a prior run, or hand-authored,
  appears in **Skills Inventory**. A creation or test skill also appears in at least one **Skill Application
  Mapping** row. A bespoke reused skill with no file-type trigger appears in Skills Inventory only, with its
  trigger in the `Load when` column.
- `test-one` uses explicit placeholders so the orchestrator can substitute a module and test name.
- The Review Rule Set is copied from the stack reference's rule seeds. Keep IDs stable so downstream prompts
  can reference them.

---

## 2. project.toml

Path: `.ostra/project.toml`. Machine-readable twin of the inventory, in TOML with snake_case keys. Ostra parses
it into its project profile type, so an unknown key is ignored and a misspelled one is lost. Schema:

```toml
schema_version = 1
generated_at = "{YYYY-MM-DD}"
stack = "java-spring"
build_tool = "maven-wrapper"
test_framework = "junit5+mockito"

[commands]
build = "./mvnw -q -T1C compile"
test = "./mvnw test"
test_one = "./mvnw test -pl {MODULE} -am -Dtest={TEST} -Dsurefire.failIfNoSpecifiedTests=false"
format = "./mvnw spotless:apply"
# lint, typecheck, and run are omitted because this repo has none. TOML has no null.

# One table per kind of test the repo runs. The test stage assigns each check one of these levels.
[test_types.unit]
command = "./mvnw test"
command_one = "./mvnw test -pl {MODULE} -am -Dtest={TEST}"
matches = ["src/test/java/**/*Test.java"]
note = "Needs no running services."

[test_types.integration]
command = "./mvnw verify -DskipUnitTests"
command_one = "./mvnw verify -pl {MODULE} -am -Dit.test={TEST}"
matches = ["src/test/java/**/*IT.java"]
note = "Starts PostgreSQL through Testcontainers, so Docker must be running."

[[module_map]]
glob = "src/**"
area = "app"

[[skills]]
name = "convention"
kind = "convention"
path = ".agents/skills/convention/SKILL.md"
source = "generated"

[[skills]]
name = "entity"
kind = "creation"
path = ".agents/skills/entity/SKILL.md"
component_type = "entity"
source = "generated"

[[skills]]
name = "deploy"
kind = "other"
path = ".agents/skills/deploy/SKILL.md"
source = "reused"

[conventions]
immutability_keyword = "final"
naming = "{ComponentType} suffix classes"
notes = ["single timestamp per method"]

[[review_rules]]
id = "C1"
rule = "…"
severity = "M"
auto_fixable = true
```

**Rules:**
- TOML has no null. Omit any key whose value would be null (`component_type`, a command), and
  never write `null`, because Ostra cannot parse a file that contains it and the init fails.
- `schema_version` is the number `1`, never a string.
- `stack` is one string that names the stack in your own words: the language, then the main framework, in
  kebab-case (`java-spring`, `rust-axum`, `elixir-phoenix`, `go`). It is never a `[stack]` table and never
  `generic`, because a reader takes it as the repo's actual stack. `build_tool` and `test_framework` are
  top-level strings next to it.
- `conventions` is one table (`[conventions]`), never `[[conventions]]`, because Ostra reads one set of
  conventions per repo.
- `[commands]` values are exact shell strings. Omit a command the repo does not have, because an empty string
  reads as a command that runs nothing. Use the SAME placeholder names (`{MODULE}`, `{TEST}`) as
  in INVENTORY. The keys are `build`, `test`, `test_one`, `format`, `lint`, `typecheck`, and `run`.
- `[[skills]]` mirrors the INVENTORY Skills Inventory table 1:1.
- Each `[[skills]]` entry carries `source`: `"generated"` (written this run) or `"reused"` (an existing skill
  kept as-is and only registered). A reused skill's `kind` may be `"other"` when it maps to no scouted
  component type.
- `[[module_map]]` mirrors the INVENTORY Module/Area Map 1:1.
- `[[review_rules]]` mirrors the INVENTORY Review Rule Set 1:1. `severity` is `H`, `M`, or `L`.
  `auto_fixable` marks the rules Ostra may apply directly from a finding's exact `Change ... to ...` text.
  `SEC-BLOCK-*` and `PHASE-REQ-*` IDs never appear here: the code-reviewer defines them itself.
- **No routing.** The profile has no `models` or `harnesses` table. Which executor and which model each agent
  runs on is a workspace setting, edited in Ostra's settings screen and resolved per execution. A routing
  table written here would be ignored, and a reader would take it for the live configuration.
- Agents take exact command strings from the repo brief Ostra appends to their task, which is resolved from
  this file, and routing decisions from `INVENTORY.md`. The Commands table and the Review Rule Set are stated
  in the inventory in full for the reader who opens it directly.
