# Changelog

Every release of Ostra, written from its commit titles by `./release.sh`.

## 0.2.1 (2026-10-07)

### Features

- Make Claude Haiku 5.5 the default native fast model (689016e)

## 0.2.0 (2026-10-07)

### Features

- Add plugin SDK, programmatic workflows, and builders (687ea83)
- cli: Add ostra plugin add to register a plugin program (3aa71ee)
- sdk: Add web-dev example plugin with e2e testing (9aed217)
- **Breaking:** Let documentation writers choose topics and write free pages (ee0ff45)
- Run the docs stage as a survey, page writers, and synthesis rounds (b0b1416)
- **Breaking:** Remove the architecture agent and check the shape of docs pages (ca15cbd)
- sdk: Add typed result contracts for plugins (f9a8d48)
- Run the docs as their own book stage after closing (ae381fb)
- core: Name the work dirs of each execution (2ab303d)
- policy: Confine each run to its named work dirs (8668256)
- agents: Tell each agent the folders it works in (34315cd)
- exec: Let native tools work in each work dir of a run (f70fe73)
- engine: Name each run's work dirs and allow multi-project phases (e375c9e)
- Let a phase create only its main project (fef0dab)
- docs: Document every project of a session in one docs pipeline (ecaac79)
- plan: Make each plan phase a working feature (a18486d)

### Fixes

- engine: Allow the eight arguments of Pipeline::helper_task (f1092d1)
- engine: Keep the budget gate in the engine for every pipeline (301dffa)
- default-plugin: Box the step that the pipeline's effects give back (6f6f605)
- core: Show no project tag on session-wide docs runs (d83e1f5)
- docs: Leave the part across projects out of Book parts (a1f8994)

### Refactors

- Move the standard plugin into ostra-default-plugin (7a493e5)
- Run built-in stages as the standard plugin's pipeline (26f0eac)
- Move the docs-stage generator out of the core book (106ae2f)
- Keep the built-in stages' state in the pipeline (79ca396)
- Route built-in steps, checks, and spawn inputs through the pipeline (d824ea8)
- Split the large pipeline and runner files by stage and area (341fca2)
- Move each built-in stage's files into stages/<stage>/ (3ecdc98)
- Hand each gate, run, and judge decision to its stage (490a742)
- Keep each stage's state and helpers in its stage folder (4b69da7)
- Build each judge's input in the stage that asks it (647ab8a)
- Move each stage's board cards and artifacts into its stage folder (30c87b1)
- Answer each pipeline hook in the stage that owns it (ff8d867)
- engine: Move the session fold into state/apply.rs (ca04530)
- default-plugin: Group a track's docs state in DocsPipeline (013d209)

### Documentation

- Rewrite the plugin and workflow pages in STE (a2420ec)
- Describe the standard plugin's pipeline and the book stage (214bdc9)
- Describe the standard plugin's stage folders (91b9446)
- Describe each stage's view module (82c448f)
- Describe each stage's pipeline hooks (0679efe)

### Tests

- Wait for queued amendments in the attached-files test (3b1abc2)
- code: Find the session fold in state/apply.rs (323e138)
- browser: Remove the book architecture from the books spec (80ac259)

### Build and chores

- Update Cargo.lock for the engine's dropped dev-dependency (80343dc)

### Other

- Merge branch 'master' of github.com:diepquynh/ostra into feature/workflow (b559f64)
- Merge branch 'worktree-agent-aca0e80d25281997e' into feature/workflow (29df98c)
- Merge branch 'worktree-agent-a5797101d7fdd31b7' into feature/workflow (87352b4)
- Merge branch 'worktree-agent-a4a79b28db7200904' into feature/workflow (0519637)
- Merge pull request #7 from diepquynh/feature/workflow (60ecce9)

## 0.1.3 (2026-10-04)

### Documentation

- Rewrite the docs pages in Simplified Technical English (97d809b)
- Fix statements that disagree with the code (6df0e2c)

## 0.1.2 (2026-10-04)

### Features

- Write docs in Simplified Technical English (c58d153)

### Build and chores

- Add Dependabot config (ddd91bb)
- Add CI for pull requests (e7f78cb)

## 0.1.1 (2026-10-02)

### Fixes

- mcp: Stop a server's processes when it is turned off (73fc262)

### Build and chores

- Pass clippy and rustfmt on the pinned toolchain (12a0f17)
- Upgrade to Rust 1.99 (2b51fde)

## 0.1.0 (2026-10-02)

### Features

- Maximize the execution terminal (8307199)
- Manage every project's skills from one page (71c96af)
- Save provider keys from the browser and route effort by phase complexity (ee3317b)
- Resize the docks and render content at full width (9051ffb)
- Revise the spec and plan in place instead of regenerating them (72bb451)
- Write research, spec, and plan as typed documents and render them as chapters (96dbb16)
- Fill the width in the reports view and resize table columns (19661df)
- Pin a wide table's scrollbar to the bottom of the reports view (a5bd06e)
- Add quick changes on the native executor and price harness runs from their session files (ca6c8c0)
- Cache prompts for one hour and price models from the models.dev catalog (b1fae8b)
- Add docker compose setup (655a38a)
- Clone and pull projects from git with credentials saved in the console (7d3e10a)
- Create folders while browsing, install and log in harnesses, clone from the setup wizard (a11b4f4)
- Look for existing skills before scouting, keep skills in .agents/skills, and brief agents with CLAUDE.md and AGENTS.md (98dd93a)
- Navigate code, edit files, and create files and folders in the Files view (571223c)
- Add a Git dock tab, mark changed lines in the file view, and switch tabs with Ctrl+Tab (08a0e2d)
- Add install.sh to run Ostra as a launchd or systemd user service (5c326aa)
- Delete a workspace's Ostra records from Settings (6680295)
- Answer code navigation from per-project language servers over LSP (4602267)
- Give agents code navigation tools over a dependency graph built from the index (83dff7b)
- Show a project's dependency graph, rebuild its code index, and edit its commands (d15ac39)
- Show function call references in the dependency graph and search it for any symbol or file (8e7695b)
- Link implementations in the code graph and give agents a CodeImplementations tool (2e334e8)
- Go back in the graph, preview listed definitions, and show what else a file defines (832bb3e)
- Complete code and show signature help in the file editor over the socket (9d0aeea)
- Show base classes and implementations from the file editor, with a right-click menu (defc89f)
- Browse and edit files in one Monaco editor, jump with Ctrl+click, and open tabs beside the current one (57ebcb9)
- Open library sources and jar classes that a language server points at, read-only (d23e139)
- Pin tabs and close them from a right-click menu (1ef5a15)
- Add session context files, uploads, pause, read-only harness sessions, and tab reordering (f9fa938)
- Connect agents and every harness to workspace MCP servers, with a settings tab (2976b63)
- Harden Ostra before open sourcing: sandbox, sealed credentials, and folder-file approval (8d574d1)
- Sandbox agent commands on macOS with Seatbelt (7bb4ce4)
- Check the init profile at submit, free-text stacks, and a per-workspace sandbox mode (a088d7e)
- Replace the logo with a live mark that follows the open session (c625870)
- Add the homepage and docs site on a shared design system package (1e7c866)
- Add workspace artifacts: files every agent reads, in the left dock (45c13e8)
- Filter sandbox egress, plant decoy credentials, and pause on containment signals (32f8b33)
- Filter egress on macOS, refuse other processes' environment, and add loopback settings (002040d)
- Run Ostra on Windows: Git Bash, PowerShell and Cmd tools, job-object trees, and Windows path guards (ece6c4a)
- Add install.ps1 to run Ostra as a Task Scheduler task on Windows (3fb32bc)
- Resume a paused execution in place instead of as a second run (901b0eb)
- Lock keyboard shortcuts in fullscreen so browser-reserved combos reach the console (8118dc2)
- Add the mobile console from the Claude Design templates (6f49b39)
- Add a light track, a feedback loop after the build, and judge routing evals (c6b0dab)
- Route every gate answer through a judge before any agent sees it (14ec4c5)
- Let the implementer create a project the plan needs, initialize it in the session, and advise on stuck init steps (2eb4076)
- Add the homepage intro splash and scroll reveal from Claude Design (f174147)
- Implement the motion homepage from Claude Design (d096746)
- Let subagents ask and wake each other, and measure it with coordination evals (ab59783)
- Compact long native runs near the context window instead of clearing tool results (be26b31)
- Add tool enforcement, disabled by default, so capable models may edit files by any route (330db8e)
- Show what each native model response costs, on its thinking and its tool calls (5167641)
- Make the test stage verify the whole change, and measure it with test stage evals (a4b8af5)
- Let each workspace rebind the console's shortcuts, and offer to install Ostra as an app (a817080)
- Write the documentation book per project area, and let agents search it (ff1aefe)
- Let the user skip a research, test analysis, or docs task, and stop the Sufficiency judge queuing research a later agent does anyway (29f411f)
- Let the user send one run a correction, and withdraw queued context before any step reads it (07c5d22)
- Let agents read listed credential files, and give Yarn Berry its own sandbox cache and proxy (ca53961)
- Send a run stuck on its environment to the advisor before the user, and finish a session whose test phase ends blocked (9328c95)
- Let the user send an implementer to fix what stopped a stuck run, then continue the stuck agent, and answer every stuck gate that way under YOLO (2d04b15)
- Hide every path a .*ignore file hides from a sandboxed agent's searches, in native Grep and Glob, harness search calls, and shell searches (1f738c8)
- Cut code re-reading in the planning stages (db4b8f6)
- Version releases from commit titles (3cee52c)

### Fixes

- Fix Claude pricing and add refresh button (6257c76)
- Let Claude Code harness sessions load Ostra's MCP tools (c10f05d)
- Harden terminal streaming and sign-ins (26adba3)
- Stop offering MCP 2026-07-28 so Claude Code gets Ostra's tools (431554b)
- Display proper title and description in session details (3f372fd)
- Load the app on refresh of file routes whose path ends in a file name (b10a0a2)
- Scroll long menus and let small multi-line inputs grow (3f0e5d6)
- Show a running format command on the session board (d69b007)
- Fail fast on harness sign-in screens and run quick answers natively (d128402)
- Keep harness terminals at 20x5 or larger so tiny resizes cannot kill output (74ee417)
- Find the usages of the member at the clicked position, not every member with its name (0c1cbe4)
- Keep a file's scroll position across tab switches, and stop the active tab changing on hover (b2a3d07)
- Fix ostra-core unused var warning (a1ef572)
- Keep folder files from disabling the sandbox and block removals during live work (a632966)
- Restore the Seatbelt canary name for macOS builds (f42d4c8)
- Render the homepage's console shot on Windows (c3008a1)
- Find cargo and npm in install.ps1 from a terminal opened before they were installed (00a2666)
- Start npm-installed harness CLIs on Windows (f6b69cd)
- Browse Windows folders in the folder picker (3bf5c2d)
- Spawn one explore per repeated Sufficiency task (6064525)
- Send Anthropic's direct web tools instead of the code-execution ones (5ad46e7)
- Keep agents to Ostra tool calls instead of status text (b940228)
- Keep Windows-only shell code from warning on Linux and macOS (3fff8be)
- Let the initializer write skills through a linked skills dir (9b34175)
- Keep a session to the projects pinned on the New task form (3245da7)
- Warn about ⇧⌘T, ⇧⌘N, ⇧⌘W and ⇧⌘Tab on macOS when recording a shortcut (a71232f)
- Draw book diagrams at natural size with opaque edge labels (7e663ee)
- Keep Esc from closing the installed app on Android, and switch tabs with keys Chrome on Android passes through (c589670)
- Show spec, plan, their fact-checks, system architecture and quick answers as session-wide runs instead of tagging them with the primary project (7bba911)

### Refactors

- Move workspace handling into an ostra-workspace crate (d6566d6)
- Move the sandbox into an ostra-sandbox crate with one OS layer (5dd1cb8)
- Split the store's SQL into one repository per table behind the unchanged WorkspaceDb and RegistryDb API (903d771)

### Documentation

- Add provider usage guide covering API keys, harness sign-in, and provider terms (0f51b5c)
- Revamp README build and run with quick start and step-by-step setup (f9414da)
- Add deep-dive docs on Ostra internals, security, and platforms (3f012ed)
- Add console screenshots to the user interface and How Ostra works pages (169d675)
- Add the workspaces deep dive with console screenshots (59a5fc8)
- Add the sandboxing deep dive under Security (258171e)
- Open the sandboxing page with what the sandbox guarantees (f14efe4)
- Add Windows to the homepage install box and say what Ostra is for (e0f3f1f)
- Add a "When should you use Ostra" section to the README (2d49af2)
- Record that an effort change loses the prompt cache on continued runs (1105c1e)
- Add a security policy with private reporting by email and the boundaries a report can cover (73f4e49)
- Keep commit titles short (40b9655)

### Tests

- Match the session header's title and request (36451de)
- Keep the MCP test's bridge socket under the macOS path limit (9b78dd1)
- Commit planning eval results beside their cases (5d2be34)

### Build and chores

- Record the Claude Design sync target (0285feb)
- lint (adffaa5)
- Cache cargo and npm downloads and the Rust target dir across Docker builds (ee75776)
- Lint and format the frontend with Biome (b6bccdc)
- Strip debug info and symbols from release builds (180033c)

### Other

- Initial commit (580b8d9)
- Redesign the console and add the backend it needs (d7dd224)

