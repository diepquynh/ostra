# Screens

The shell (`web/src/shell`) owns the chrome, tabs, routing, dialogs and live data. Each screen fills the center
pane for one resource id and keeps the props below; `index.tsx` maps ids to screens.

| Screen | Props | Resource / URL |
| --- | --- | --- |
| `WorkspaceScreen` | `{ ws }` | `ws:overview`, `/w/:ws`. New task form and sessions table; reads and clears `useShell().taskDraft`. |
| `SessionScreen` | `{ ws, id }` | `session:<id>`, `/w/:ws/s/:id`. Session board; a `#gate-<id>` hash scrolls to that gate. |
| `ExecutionScreen` | `{ ws, id }` | `exec:<id>`, `/w/:ws/x/:id`. Usage strip, then Activity or Terminal by `ExecutionView.stream`. The paused-ask banner reads `ExecutionView.pending_gate`; tool paths are shown relative to `ExecutionView.repo_root`. |
| `ArtifactScreen` | `{ ws, path }` | `artifact:<path>`, `/w/:ws/artifact?path=`. Spec, plan, report, ledger. Scrolls itself. |
| `SettingsScreen` | `{ ws }` | `ws:settings`, `/w/:ws/settings`. Workspace settings forms; a `#setting:<dotted key>` anchor opens that field's tab and rings it. Agent defaults come from `WorkspaceDetail.agents`, stacks from `WorkspaceDetail.stacks`, and the read-only global rules from `WorkspaceDetail.global_permissions`. |
| `CostScreen` | `{ ws }` | `ws:cost`, `/w/:ws/cost`. Spend tables for this week (from `WorkspaceActivity.week_since`, the status bar's week) or all time. |
| `MemoryScreen` | `{ ws }` | `ws:memory`, `/w/:ws/memory`. Lessons per project; `?project=<key>` or a `#project:<key>` anchor preselects one, `#lesson:<key>:<id>` selects a lesson. |
| `SkillsScreen` | `{ ws }` | `ws:skills`, `/w/:ws/skills`. Every project's skills: create, edit, register, delete, and adopt harness skills from `.claude/skills/` and similar; `?project=<key>` narrows the list. |
| `ProjectScreen` | `{ ws, projectKey }` | `project:<key>`, `/w/:ws/p/:key`. Header with the branch (`ProjectView.git_branch`), then the overview (or Initialize flow) with no tabs; "Browse files" calls `useShell().browseFiles(key)`. Scrolls itself. |
| `FileScreen` | `{ ws, projectKey, path }` | `file:<key>:<path>`, `/w/:ws/f/:key/<path>`. Read-only file or Changes diff. Scrolls itself. |
| `setup/Onboarding` | `{ onFinish(workspaceId \| null) }` | Full screen at `/` on first run, and from "Run the setup guide again". The caller completes onboarding. Stacks come from `EnvironmentStatus.stacks`. |
| `setup/NewWorkspaceDialog` | `{ onClose }` | Dialog from the workspace menu, ⌘K and `/`. Navigates to `/w/<new id>` on success. |
| `setup/AddProjectDialog` | `{ ws, onClose, onAdded(detail, key) }` | Dialog from the menu, ⌘K and the sidebar's Projects +. A 422 places each issue on its field (`key`, `path`, `stack`). The shell then opens `project:<key>`. |

Screens that scroll their own content are listed in `selfScrolling` (`index.tsx`); the shell gives them the
whole pane with `overflow: hidden`. Every other screen scrolls inside the pane.

## What a screen may use

- `useNav()` (`lib/nav.ts`): `open(id, { preview?, anchor? })`, `close(id)`, `pin(id)`, `href(id)`, `activeId`,
  `tabs`, `ws`. Rows in lists open with `{ preview: true }`; explicit actions open a normal tab.
- `useShell()`: `openDock(question?)`, `closeDock()`, `taskDraft` and `setTaskDraft`, `newWorkspace()`,
  `addProject()`, `runSetup()`, `browseFiles(key)`, `theme`, `toggleTheme()`.
- `useWorkspace()`: the shell's `WorkspaceDetail` (`detail`, `reload`), so screens need not refetch it.
- Live data (`lib/live.ts`, shared per workspace, one fetch and one socket subscription each):
  `useWorkspaceTree(ws)`, `useSessionSummaries(ws)`, `useActivity(ws)`, `useFileIndex(ws, key)`,
  `useProjectChanges(ws, key)`, `useProjectFsChanges(ws, key, cb)`, `useWorkspaces()`, `useSocketState()`,
  `useSearch(ws, q)`. Older helpers stay in `lib/hooks.ts`: `useAsync`, `useChannel`, `useThrottled`.
- Resource ids and URLs: `lib/resource.ts` (`parseResource`, `resourcePath`, `resourceFromPath`, `fileId`).
- The Monaco diff (`artifact/MonacoDiff.tsx`), the file editor (`project/code/FileEditor.tsx`, both set up through
  `components/monaco.ts`), and xterm (`execution/XtermScreen.tsx`) load with `lazy()`, so they
  stay out of the main chunk. Import them only through their lazy wrappers.
