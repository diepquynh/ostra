# Output Contract: INVENTORY.md and project.toml

This file defines the exact structure the initializer's **generate** mode must write, and that the
orchestrator and every subagent read. Both files live in the target repo at `.ostra/`.

**Design principle: route by inventory, not by description.** Models do not reliably route off skill
front-matter `description` fields. So the single source of truth is `INVENTORY.md`, a plain markdown file
that every agent is instructed to **Read** first. Skill discovery is a loading mechanism, not a
routing source. The inventory works the instant it is written because it is just a file.

**Design principle: every skill row carries BOTH a name and a path.** A per-project skill lives in the target
repo at `.ostra/skills/{name}/SKILL.md`. Ostra's skill tool resolves a name to that path, and every harness
executor loads a skill by reading the path, because a path is the one mechanism that works on every harness
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
| `convention`         | convention  | `.ostra/skills/convention/SKILL.md`     | Always. Auto-load for any code edit.        |
| `module-hub`         | module-hub  | `.ostra/skills/module-hub/SKILL.md`     | Locating which area/module a path belongs to.|
| `{component-skill}`  | creation    | `.ostra/skills/{component-skill}/SKILL.md` | Creating or modifying a {component type}. |

## Skill Application Mapping

| File type being changed | Skills to load          |
| ----------------------- | ----------------------- |
| {component type}        | `{skill}`, `convention` |

## Module / Area Map

| Path glob                | Area        | Reference                                   |
| ------------------------ | ----------- | ------------------------------------------- |
| `{glob}`                 | {area name} | `.ostra/skills/module-hub/references/{x}.md` or `none` |

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
test_framework = "junit5+mockito"

[stack]
language = "java"
frameworks = ["spring-boot"]
build_tool = "maven-wrapper"

[commands]
build = "./mvnw -q -T1C compile"
test = "./mvnw test"
test_one = "./mvnw test -pl {MODULE} -am -Dtest={TEST} -Dsurefire.failIfNoSpecifiedTests=false"
format = "./mvnw spotless:apply"
# lint, typecheck, and run are omitted because this repo has none. TOML has no null.

[test_types.unit]
command = "./mvnw test"
command_one = "./mvnw test -pl {MODULE} -am -Dtest={TEST}"
matches = ["src/test/java/**/*Test.java"]
note = "Needs no running services."

[[module_map]]
glob = "src/**"
area = "app"
# reference omitted: this area has no reference file yet

[[skills]]
name = "convention"
kind = "convention"
path = ".ostra/skills/convention/SKILL.md"
source = "generated"

[[skills]]
name = "entity"
kind = "creation"
path = ".ostra/skills/entity/SKILL.md"
component_type = "entity"
source = "generated"

[[skills]]
name = "deploy"
kind = "other"
path = ".ostra/skills/deploy/SKILL.md"
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
- `[commands]` values are exact shell strings. Omit a command the repo does not have: TOML has no null, and an
  empty string reads as a command that runs nothing. Use the SAME placeholder names (`{MODULE}`, `{TEST}`) as
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
