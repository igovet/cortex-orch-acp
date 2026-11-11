# Cortex Workspace desktop mock

In-memory desktop prototype. The owner approved the [static mockups](../mockups/README.md), and the **Workspace** in `src/workspace/ui/` is their clickable implementation and the whole app. It covers the app frame, Home, Inbox, the project board and list, the command palette, the task session, the live worker view, New task, result review, Ready to merge, conflicts, Code, Git, worktrees, sprints, Archive, machines and Settings. No native or runtime integration is qualified.

**Mock prototype - fixture data, not runtime evidence.** No ACP, service, fetch, WebSocket, Tauri commands, filesystem or Git integration. All actions change memory; refreshing or Prototype → Reset discards them. Vite's development server/HMR is tooling, not a product connection.

## Run and check

Requires Node 22.12+ (validated here with Node 24.14.1 and npm 11.11.0).

```sh
cd prototypes/cortex-workspace/desktop
npm ci
npm run typecheck
npm test
npm run build
npm run dev -- --mode a      # http://127.0.0.1:5181/
```

`--mode b` and `--mode c` still reserve strict loopback ports 5182 and 5183; every server serves the same app.

## Workspace routes

Every view is a History API route; Back/Forward work, and modified clicks open new tabs. `/` redirects to the last machine, or the local one.

| Path | View |
|---|---|
| `/m/:machine` | Home: needs you, review and merge queues, engine limits, all projects on the machine |
| `/m/:machine/inbox` | Decisions from every connected machine, including sprint planning and unmerged-work questions. Each row opens where that decision is made |
| `/m/:machine/archive` | The Archive of every machine: countdown groups, restore, delete, the unmerged-work question |
| `/m/:machine/new` (`?kind=sprint`, `?project=`) | New task or sprint |
| `/m/:machine/p/:project/board` · `list` (`?scope=standalone`) | Project board and list |
| `/m/:machine/p/:project/knowledge` · `settings` | Harvest, refresh and skills as tasks; project facts and its override |
| `/m/:machine/p/:project/code` (`?checkout=`, `?file=`, `?line=`) | Code view of one checkout |
| `/m/:machine/p/:project/git` · `worktrees` (`?checkout=`) | Git view: status on the left, History or Branches and worktrees on the right |
| `/m/:machine/p/:project/s/:sprint/board` · `list` | The same views scoped to a sprint, with its queue bar |
| `/m/:machine/p/:project/s/:sprint/planning` | Planning conversation beside Plan vN, dependency graph, run mode, target |
| `/m/:machine/p/:project/s/:sprint/summary` | Counts and one row per task: state, checks, branch |
| `/m/:machine/t/:task` | Task session: conversation, plan, worker rows, decision cards, composer, queue |
| `/m/:machine/t/:task/workers` · `workers/:worker` | Workers list with one worker's detail; `:worker` also takes `cortex` and `verification` |
| `/m/:machine/t/:task/workers/:worker/live` | Live worker: full activity stream, filters, Follow, direct messages |
| `/m/:machine/t/:task/changes` | Diff review, or Ready to merge once the task is there or in a conflict |
| `/m/:machine/t/:task/changes/diff` · `changes/conflict` | The diff review and the conflict, whatever the task's state |
| `/m/:machine/t/:task/code` (`?file=`, `?line=`) | Code view of the task's checkout, at a file and line |
| `/m/:machine/t/:task/terminal` | Opens the mock bottom panel |
| `/settings/app/appearance` | Theme: System (default), Light or Dark. This is the only place it changes |
| `/settings/app/voice` · `shortcuts` · `machines` | Voice input, shortcuts, all machines with Add remote machine |
| `/settings/machine/:id/overview` · `connections` · `worktrees` · `storage` · `diagnostics` · `remote` | Settings of one machine |
| `/settings/machine/:id/orchestration` | Overview: what needs attention, one line per page, the routing file in use |
| `/settings/machine/:id/orchestration/defaults` · `models` · `effective` · `policy` · `profiles` · `files` | Coordinator, Models, Route preview, Limits and fallback, Profiles, Files and presets |
| `/settings/machine/:id/orchestration/routing` (`?tab=roles`, `kinds`, `quota`, `rules`) | Routing |
| `/settings/machine/:id/orchestration/tools` · `tools/new` (`?tab=define`, `connect`, `access`) · `tools/:server` (`?tab=connection`, `tools`, `sandbox`) | MCP servers, Add a server, one server |

Any other path, including the round-one `/a`, `/b`, `/c`, `/archive/a` and Project Desk addresses, shows Not found.

### Workspace frame

`src/workspace/ui/` holds `index.tsx` (composition, shortcuts), `router.ts`, `store.ts` (thin reactive wrapper over `WorkspacePort`), `selectors.ts` (read models), `prefs.ts` (panels), `frame.tsx` (title band, sidebar, inspector, terminal, menus), `home.tsx`, `board.tsx`, `pages.tsx` (Inbox, Not found), `palette.tsx`, `kit.tsx` (links, native popovers and modals, virtual list, follow), the task screens listed under [Task session](#task-session), the screens listed under [Review, merge, Code and Git](#review-merge-code-and-git) and those listed under [Sprints, Archive and Settings](#sprints-archive-and-settings).

- **Styles.** `mockup-styles.css` keeps the token blocks and the rules the app still uses from `../mockups/styles.css`; a test keeps the design tokens identical to it. Put overrides in `workspace.css`, and layout primitives in `layout.css`.
- **Panels.** Sidebar (`Ctrl+B`, width 220 to 360), Inspector (`Ctrl+Alt+B`, docked at 1440px and up, otherwise an overlay, and off with the reason on sprint, Archive and Settings screens) and Terminal (`Ctrl+J`, on project and task views) each have a toggle that never moves, and a palette action. Their state is stored under `cw.ws.*` and survives Reset.
- **Counters.** Every counter and empty state reads the same list from `selectors.ts`. "Needs you" always means the board's Needs you column; only Inbox adds other decisions.
- **Unknown state.** A task on an offline machine, or with an unconfirmed process, stays in its last known column, says so, and offers no actions. An unknown engine limit reads Unavailable where limits are listed, and stays off the sidebar; it is never zero.
- **No placeholders for missing data.** The owner asked for no dashes: a value with no data leaves its cell empty.
- **Shadows.** `workspace.css` overrides the shadow tokens so shadows are barely visible, as the owner asked. `mockup-styles.css` keeps the mockups' token values, and `workspace.css` overrides the shadow tokens on top of them.
- **Fixture gaps.** The domain has no engine per task, diff size or commits-behind count, so cards leave those out rather than inventing them.
- **Keyboard.** `Ctrl+K` palette (`>` lists actions), `Ctrl+N` new task, `Ctrl+Shift+N` new sprint, `Ctrl+I` Inbox, `Ctrl+,` settings, `/` filter, `?` help, Space preview in the Inspector, arrow keys between cards, `Ctrl+.` Stop, `Ctrl+\` worker pane, `1` to `8` answer the open question, `Enter` steer, `Ctrl+Enter` queue, commit or send diff comments, `Ctrl+P` go to file and `Ctrl+Shift+F` search in files in Code. Browsers reserve `Ctrl+N`, `Ctrl+Shift+N` and `Ctrl+P`, so in a browser use the buttons or palette instead.

### Task session

`session.tsx` (page, conversation, workers list), `worker.tsx` (stream, worker detail, direct messages, stop dialogs), `composer.tsx` (composer, queue, route pickers, dictation), `task-chrome.tsx` (title, branch chip, title band actions, Inspector, Close dialog), `newtask.tsx`, `session-model.ts` (read models) and `sim.ts` (simulation).

- **Fixture.** `WORKSPACE_CASES.session` carries the whole approved session. Tasks without a recorded session start from an empty one, and unknown context, usage or routes read Unavailable.
- **Steer and queue.** `Enter` steers: the message is sent, joins the conversation and cannot be edited. `Ctrl+Enter` queues: it stays unsent, can be reordered, removed or sent, and the UI warns it may be lost. Edit takes it out of the queue and back into the composer with its attachments, to be sent again. With an empty composer the send slot is Stop.
- **Attachments.** The composer, Other answer and New task take files. Only their names are kept, nothing is read from disk. A queued message shows its attachments. Other answer takes line breaks with `Shift+Enter`.
- **Workers.** Messages go to the running attempt only, through sending, pending, delivered, failed and withdrawn states. Delivered does not mean read. A finished worker cannot be messaged; Ask Cortex prefills the composer instead.
- **Stop, Resume, Close.** Stop needs no confirmation and keeps everything; open questions stay open but cannot be answered until Resume. Any message resumes a paused task. An unconfirmed stop blocks Close and Resume until Retry. Close lists its consequences in numbers and offers Close as is or Cancel task; `close_task` withdraws open questions, queued and undelivered messages.
- **Voice.** Dictation is simulated: no microphone is used. Words appear on a timer and only ever land in the draft. Insert and send is a separate click.
- **New task.** Description only. The title is simulated after a short delay and can be renamed; a failed title keeps the temporary one with Retry and Rename. Worktree is the default, with Main checkout and its warning as the alternative.
- **Mock menu.** On a task it adds Simulate the next step, Ask a question and Publish a result. It also toggles the self-playing simulation, an unconfirmed next stop, a failed next Cortex resolution, a failed next title and the two dictation faults.
- **Clock.** Stamps and elapsed figures run from the fixture clock plus real time since the app opened.

Worker reports and full command output are labelled as unavailable.

### Review, merge, Code and Git

`review.tsx` (result card, Changes tab), `diff.tsx` (unified and split diff, line comments), `merge.tsx` (Ready to merge, conflict), `dialogs.tsx` (accept, request changes, merge, Let Cortex resolve, delete worktree, branch, push, revert, amend), `code.tsx`, `git.tsx` (status, history, worktrees, checkout picker), `highlight.ts` and `review-model.ts` (read models). The rules are in `domain/git.ts` and `domain/session.ts`; the data is in `adapter/review-fixture.ts`.

- **Fixture.** Four Cedar Notes tasks walk the whole path: `WORKSPACE_CASES.review` (Rate-limit the export endpoint), `merge` (Add the usage events table), `rollups` (Rebase usage rollups onto main) and `mergedWorktree`. `session` carries the Code and Git view. Only Cedar Notes has code and a Git record; other projects say so. The older Amber Components conflict has its own small record.
- **Outcomes.** Passed, failed, not run and unverified are always all shown, and only a pass is green. A finished worker with a failed check reads as that, not as Finished.
- **Checks and acceptance.** Publishing a result automatically schedules any not-run check without a technical blocker; it cannot be accepted while running. Every unpassed check offers Rerun check in the result card and diff evidence (also in Ready to merge). Rerunning clears acceptance and keeps the last outcome visible until the simulated next step finishes. The simulation labels its outcome and runs no command. Ordinary failures and unverified claims must be checked, not waived. Only checks with recorded technical reasons can remain open at acceptance: the dialog lists those reasons and requires an explicit tick. The open counts stay on acceptance and in the merge dialog. A stale version or a result with no checks cannot be accepted.
- **Sending a result back.** Request changes, the composer in review, Ask Cortex to fix and Send to Cortex all reopen the task and drop any acceptance. Line comments go as one message tied to file and line.
- **Ready to merge.** Every merge and pull request goes through one dialog: source, target (Git default branch unless changed), squash or PR, preconditions in numbers, then an explicit confirm. A changed target must be confirmed again. A pull request leaves the task in Ready to merge, unmerged.
- **Update from target.** With a scripted conflict it lands on the conflict. Otherwise the branch changes, so the acceptance goes and the result reads as stale until Cortex publishes a new one.
- **Conflicts.** Resolve yourself chooses ours, theirs or both per block. Let Cortex resolve runs read, resolve, check, and ends in Ready to merge unaccepted, or fails and changes nothing. Merge stays off until the owner accepts the new version. Stop hands the conflict back with the owner's choices kept. Abort merge is in every state and restores the status, acceptance and result from before.
- **Code.** Tree with Git letters and Only changed, preview and pinned tabs, highlighting, search with Match case, Go to file, File, Inline diff and Side by side, and `?line=` opening at a changed line. When a task is running in the checkout a warning offers Stop the task first or Edit anyway. Typing into files is not built; Edit anyway only records the choice.
- **Git.** Status groups with stage, unstage and revert per file or hunk, commit with Generate and Amend, fetch, pull, push with confirmation, history with task badges, revert commit, create branch and copy SHA. An owner commit, revert or pull in a task's worktree invalidates its acceptance.
- **Worktrees.** The Worktrees tab is the Git view's Branches and worktrees side, as in the mockups. Rows show Active, Idle, Conflict, Merged or Orphaned. A live task with no Git record shows its figures as Unavailable and offers no delete. Delete names what goes in numbers; with unmerged work Keep is the default and discard has to be chosen by name. A running task's worktree cannot be deleted. An orphan can become a draft task.
- **Not built.** The table's Size and Last activity columns, file editing, the external-change prompt, the Result pane's editable preview, the worker Report, Edits and Activity subtabs, and commit-level diffs in Show.

### Sprints, Archive and Settings

`sprint.tsx` and `sprint-model.ts` (planning, plan table, graph, summary, queue controls), `archive.tsx` and `archive-model.ts`, `settings.tsx`, `orchestration.tsx` with its pages `orch-shared.tsx` (context bar, the shared drafts), `orch-limits.tsx` (Coordinator, Limits and fallback), `orch-models.tsx`, `orch-routing.tsx`, `orch-preview.tsx`, `orch-mcp.tsx`, `orch-profiles.tsx` (Overview, Profiles, Files and presets) and `orch-help.ts` (help text), `settings-kit.tsx` (rows, pickers, disclosure, save bar), `settings-state.ts` (drafts and save rules), `machine-dialogs.tsx` (add machine, add project, add connection, clear old tasks, unsaved changes, switch profile), `project-pages.tsx` (Knowledge and skills, project Settings). The rules are in `domain/sprints.ts`, `domain/archive.ts`, `domain/machines.ts`, `domain/config.ts`, `domain/routing-rules.ts` and `domain/mcp.ts`; the data is in `adapter/sprint-fixture.ts`, `adapter/plan-fixture.ts` and `domain/config-preset.ts`.

- **Try it first.** Cedar Notes holds Audit log export (planning, one open question, Plan v2), Usage billing (a running queue, for the summary) and Synthetic linked sprint. The Archive covers three machines. Settings opens from the sidebar foot.
- **Planning.** New sprint opens the planning tab. A mock planner writes a note, asks one question and proposes a plan, by itself or through Mock, Simulate the next step. A question also appears in the Inbox. Asking for changes earns a new version. Edit changes titles and dependencies; a self reference, missing task or cycle is refused in the row with the reason, and Done editing makes a new version. Previous plan shows earlier versions. Cancel planning keeps everything and can be resumed.
- **Create tasks.** Off while a question is open, a draft is being edited or the planner is working, with the reason on the button. Creating twice makes nothing new.
- **Manual and Automatic.** Manual: Start runs one ready task and never its neighbours, and a dependent stays locked. Automatic: Run sprint takes one task at a time in dependency order, plan order breaking ties. A running task finishes by itself in the mock, or through Simulate the next step. In Automatic mode passing checks move it straight to Ready to merge, with no owner acceptance at Review, and start the next ready task. The owner accepts and confirms integration there; nothing merges by itself. A question, a failed, not-run or unverified check, a conflict, a stop or a close pauses the queue, the reason shows on the plan, board and summary, and Continue queue says what must happen first. Pause queue starts nothing new and leaves the active task running. Mock adds Active task asks a question, fails a check and hits a conflict.
- **Summary.** Counts and rows come from real task facts. A check that did not run reads as not run, and blocked tasks name what they wait for.
- **Archive.** Groups are Cleanup within 3 days, Later and Cleanup blocked, above them anything waiting for a decision. Filters by text, machine, project and outcome. Restore returns to Done and restarts its clock; Open read-only shows the task with Restore on it. Delete shows a preview with counts and size, skips blocked tasks by name and leaves Git alone. Unmerged work asks first: Keep leaves it in Done and asks again after the machine's auto archive days, delete archives it. New files after archiving block cleanup until the owner restores or discards them; a live or unknown process cannot be waved through, and an offline machine reads Overdue.
- **Machines.** The title band switches machine; Inbox and Archive cover them all. Add remote machine has three steps and adds nothing until the fingerprints match. Mock simulates an expired code, an unreachable connector and differing fingerprints, and disconnects or reconnects a remote machine. Pairing is simulated: no connector exists here.
- **Orchestration.** Per machine, mirroring the real Cortex configuration. `domain/config-preset.ts` is `policy/model-settings-default.json` and a test keeps the two equal; only paths are synthetic. New tasks start from the machine's defaults, and running tasks keep what they started with.
- **Models and Routing.** One `routing.json` draft. Models: use in Cortex (`deny engine/*`), automatic routing (`providers`, the last engine stays), per model On, Auto, tiers and a detail pane with effort floor, max effort, preferred for, prefer when, effort ladder, also standard for, request names and the route. Routing: Roles (all 23 and `*`, role tier and work kind built in, strict pins), Work kinds (engine lean, preferred models, stretch), Quota (fixed hot band, windows, reported but unmapped labels; no 5-hour window), Rules (deny, inert auto-allow, legacy tiers, tokens that match nothing). Turning a model off leaves its stored `auto` entry and policy, and reads as not automatic.
- **Route preview.** What would run a role and work kind at a complexity and attempt, why the rest are out, and the runtime codes when nothing can. A grid gives all 23 roles. With a routing draft it can preview the draft. Quota windows and usage come from the same rules as the routes, so a draft or a project file is previewed whole. It is computed from the rules alone and labelled simulated: no such runtime operation exists.
- **Coordinator and limits.** One `defaults.json` draft. Coordinator: model, the model's observed efforts, access mode, language, checkout, what it resolves to, other parameters, or an exact route instead of the portable form. Limits: attempts, connections, data recipients, failure classes, budget, consent, routes, fallback targets; values that are stored but not applied stay read-only.
- **Project scope.** Only Models, Routing, Route preview and Limits have it. Routing: a project file replaces the machine's rules whole and starts from them. Limits: three columns, narrowing only, field by field, with Remove project ceiling.
- **Tools and MCP.** One `mcp-servers.json` draft, saved as one write. List with on, off, Check tools, Duplicate and Remove. Add a server runs Define, Connect, Tool access on a draft that exists only in the client, because a server cannot be saved with no tools. A server opens on Connection, Tools and Sandbox. Env values are never shown: Keep, Replace or Remove. Paste a command line splits a pasted command into words as a POSIX shell would, and never runs it: quoted and unquoted parts join into one word, and backslash escapes and line continuations work. Cortex starts servers without a shell, so an unclosed quote, shell syntax, a leading `~`, a leading `NAME=value` or a second line is refused with the reason, and the fields stay as they were.
- **Tool access.** Each tool sits on one rung of coordinator, read-only, writing, so a tick cascades and the lists stay subsets. New tools get no access. Always load needs a grant and is Claude only. The consultant is a fixed Never. Who gets the server and its engines are per server, because no per-tool key exists. Check as gives what one role really gets on one engine: a server that is off, has a missing path or skips that engine or role counts 0 with the reason, and the reason says how many tools the lists still grant. The ticks stay editable. Connect is simulated: no runtime operation lists a server's tools, so tools come from the fixture, Mock picks each failure on the page, and the fallback is typing names by hand. Grants per worker role and per-tool engines are open owner decisions, not settings.
- **Profiles and files.** A profile stores routing, defaults and ceiling, MCP usage policy and session limits, never project settings, MCP commands, arguments, environment or credentials. The context bar on every page reads Applied, Modified, No profile, Interrupted or Unavailable. Switching with changes offers Save and switch, Discard and switch, Cancel. Files shows what wins and why, preserved keys, environment overrides, the default preset and the migration. Mock adds faults for each of these.
- **Save rules.** One draft per file, not per page, each with its own revision, so pages of one file never conflict with each other and moving between them does not ask. Leaving the file asks Stay, Discard or Save here; browser Back goes, and the draft waits. Field errors say where and turn Save off; the command refuses the same list. A conflict writes nothing: Review changes lists the keys moved on each side and marks collisions, and only then offers Reapply mine. A timeout reads No answer and blocks a retry until Read again. Offline machines are read-only with nothing queued; writes to a remote are labelled simulated. Mock sets the next save to conflict or time out.
- **Other sections.** Engine connections (download and connect status, Enable OpenCode, rename, read-only details) with weekly Codex and Claude limits only, Worktrees (folder, default `~/.cortex-workspace/worktrees`), Storage and archive (retention, auto archive, Run cleanup now, Clear old tasks), Diagnostics, Remote access, Appearance, Voice input, Shortcuts.
- **Commands as controls.** Harvest, refresh and skills are buttons in Knowledge and skills that create a normal task. Open logs and the task Diagnostics item stay off, with the reason: there is no log folder here.
- **Not built.** Removing a task from a plan while editing, the sprint's own archive record (its tasks are archived one by one), Remove project, the Archive's undo receipt, loading skeletons and read-failure banners in settings (nothing here loads), and a connections draft: rename and session limits apply at once. The composer's model list is still a short fixed list, not the machine's enabled models.

## Earlier surfaces

The Project Desk (`src/desk/`) and the round-one concepts (`src/concepts/`) are no longer routed and do not reach the build. Their code and tests stay in the tree, and `src/desk/prefs.ts` still provides the theme and `persisted` helpers the Workspace uses. The theme choice lives in `localStorage["cw.theme"]`, is applied before first paint by the inline script in `index.html`, and survives Reset.

## Fixture and store contract

Revision **cw-fixtures-v3**, fixed seed **13014**. `src/fixtures.ts` exports `loadFixtures(concept)`, `FIXTURE_VERSION`, `SCENARIOS` and `SCENARIO_DETAILS`. Each load returns a fresh, equal baseline: 8 projects, 500 tasks, 10,000 events and exactly one last-confirmed running execution, with distinct task/execution/session identities. All names, relative file paths and dates are synthetic. No private source data was imported.

Projects contain tasks; events carry the same project ID as their task. Task counts in project order are **220 / 90 / 65 / 50 / 35 / 25 / 15 / 0**. Harbor Sandbox is empty (last activity unknown); Mosaic Studio is disconnected; Amber Components has the file/Git conflict. `selectProjectTasks`, `selectProjectAggregate` and `selectProjectAggregates` derive per-project facts; current running workers on a disconnected machine are `"unknown"`, not zero.

These counts describe the historical arrays. In the Workspace seed some of those tasks are archived, and Willow Archive sits on GPU box. Named cases: Mosaic Studio owns task 0 (last-confirmed running/offline); Cedar Notes owns tasks 1/5 (permission/complete result); Orbit Ledger owns task 2 (question); Quartz Search owns tasks 3/4 (failed check/stale result/unrun check); Amber Components owns task 6 (file/Git conflict); Indigo Maps owns task 7 (unavailable push); Willow Archive owns task 8 (long text/keyboard).

`src/store.ts` exports `createMockStore`, `MockStore` and `ConceptProps`. `createTask(title, projectId?, description?)` defaults to the first project, as before. All store actions are memory only; acceptance of stale results is blocked and decision responses never rewrite execution facts.


## Tests

`npm test` runs the fixture, store and archived-concept suites plus `src/desk/desk.test.tsx` (theme preference and persistence, dashboard and per-project views, empty-versus-counter consistency, board virtualisation, modal inertness, create task, disconnected projects, decisions and stale acceptance), `src/workspace/ui/ui.test.tsx` (routes, read models, navigation, panel restore, palette and modal inertness, virtualised lists, empty states, creation, theme, and an audit of the stylesheets for layout-shifting selected states), `src/workspace/ui/session.test.tsx` (task session, steer and queue, stop, resume and close, live worker, voice input, New task), `src/workspace/ui/review.test.tsx` (result outcomes, acceptance, Changes, merge confirmation, target choice, conflict flows, Code, Git, worktrees), `src/workspace/ui/phase4.test.tsx` (sprint planning and runs, Archive, machines, settings and their save rules, commands as controls), `src/workspace/domain/git.test.ts` (acceptance, integration, conflict and Git rules), `src/workspace/domain/phase4.test.ts` (sprint run modes, archive rules, machine settings), `src/workspace/domain/orchestration.test.ts` (the preset against the real policy file, role tiers, routing rules, route preview, limits, the tool ladder under random ticks, MCP rules, profiles, files), `src/workspace/ui/orchestration.test.tsx` (every field reachable, invented fields gone, shared drafts, conflicts, project scope, the matrix, add and edit flows, profiles) and `src/App.test.tsx` (the default route, and that earlier routes are gone). Vitest blanks CSS imports, so the audit reads the stylesheets from disk. Privacy scans forbid fixture URLs, emails, credential patterns and absolute home paths.

## Workspace domain foundation

New UI phases should use `WorkspacePort` from `src/workspace/domain/index.ts`, not the historical Solid store. No file under `src/workspace/domain/` or `src/workspace/adapter/` imports Solid; only `src/workspace/ui/` does. Rules, guards and projections live in plain TypeScript; the mock adapter commits a detached candidate only when the entire command succeeds.

```ts
import { createMockWorkspace } from "./src/workspace/adapter/mock";
import { loadFixtures } from "./src/fixtures";
import { boardPhase, targetBranch } from "./src/workspace/domain";

const port = createMockWorkspace(loadFixtures().workspace);
const view = port.snapshot(); // detached read model, not a mutable store
const task = view.tasks.find(t => t.status === "ready_to_merge")!;
const phase = boardPhase(view, task.id);
const receipt = port.dispatch({
  type: "integrate", taskId: task.id, method: "squash",
  confirmed: true, digest: task.result!.digest,
  targetBranch: targetBranch(view, task), now: Date.UTC(2030, 0, 1),
});
// receipt.mock is always true; this never runs Git or calls an engine.
```

`createMockWorkspace()` without a seed starts with one local **This machine**, no projects and 30-day retention. The rich `loadFixtures().workspace` seed additionally includes an offline remote, the connected GPU box and all eight existing projects; projects are not machines. IDs remain stable, with 500 domain tasks and the same 10,000 baseline event IDs. `WORKSPACE_CASES` and `WORKSPACE_FIXTURE_NOW` in `adapter/fixtures.ts` identify scripted states and the injected clock.

Port contract: `snapshot(): WorkspaceState`; `dispatch(WorkspaceCommand): MockReceipt` returns `{ ok, mock: true, message, entityId? }`. Failed domain commands leave state unchanged. Unknown programming commands are errors, not simulated success. Refresh creates a fresh seed; there is no persistence or network subscription. A Solid wrapper can publish a new snapshot after dispatch; it must not reproduce the rules in components.

| Area | Commands / read models |
|---|---|
| Machines and projects | `add_project`, `add_remote` (offline simulated attachment, not pairing), `pair_machine` (simulated, refuses differing fingerprints), `rename_machine`, `remove_machine`, `set_connection`, `set_retention`, `save_storage`, `set_worktree_root`, `run_diagnostics`, `set_remote_access`, `revoke_device` |
| Orchestration | `save_surface` (`routing`, `user_defaults`, `project_ceiling`, `mcp_servers`; checked against that surface's revision, optional project), `config_changed` (a simulated other writer), `set_config_fault`, `activate_profile` (`keep`: `save` or `discard`), `save_profile`, `revert_profile`, `deactivate_profile`, `create_profile`, `rename_profile`, `describe_profile`, `delete_profile`, `apply_preset`, `reset_preset`, `migrate_policy`, `record_mcp_tools`, `check_connection`, `refresh_models`, `add_connection`, `remove_connection`, `rename_connection`, `set_sessions`, `download_engine`, `enable_opencode` |
| Creation and lifecycle | `create_task` requires description and checkout, not a title; `generate_title` simulates success/failure, `rename_task` wins over late generation; `start_task`, `stop_task`, `confirm_stop`, `continue_task`, `close_task` with `as_is` or `cancel` |
| Evidence and integration | `publish_result`, `accept_result` (optional `openChecks` override), `request_changes`, `workspace_changed`, `set_target`, `integrate` with owner confirmation, current digest and exact target; `add_comment`, `edit_comment`, `remove_comment`, `send_comments` |
| Conflicts | `set_conflict`, `update_from_target`, `resolve_block`, `finish_merge`, `abort_merge`, `resolve_with_cortex`, `advance_resolution`, `stop_resolution`, `finish_resolution` |
| Git and worktrees | `git_stage`, `git_revert`, `git_draft`, `git_commit`, `git_fetch`, `git_pull`, `git_push`, `create_branch`, `revert_commit`, `delete_worktree`, `attach_worktree`; `state.repos` and `state.reviews` hold the records |
| Sprint planning | `create_sprint`, `plan_note`, `plan_question`, `answer_planning`, `plan_message`, `publish_plan`, `edit_plan`, `materialize_plan` (idempotent for the same version), `cancel_planning`, `resume_planning`, `rename_sprint`, `set_sprint_settings`; `set_run_mode`, `run_sprint`, `pause_sprint` |
| Workers and input | Worker status, activity and finished results; `worker_activity`, `finish_worker`, `stop_worker`, `stop_command`, `settle_command`; `send_message` (`send_now` or `queue`, optional worker, `resume`), `edit_queued`, `remove_queued`, `move_queued`, `send_queued`, `flush_queue`, `simulate_delivery`, `withdraw_message`, `redirect_message`; `task_issue` / `clear_issue` pause the sprint |
| Session | `set_task_settings` (model, effort, access, language, for one task); `ask_decision`, `answer_decision`; `state.sessions` holds each task's timeline, plan, decisions, context and usage |
| Archive | `archive_due` after the machine's auto archive days in Done (seven by default); `archive_task`, `answer_archive`, `restore_task`, `purge_due`, `purge_task`, `purge_many`, `run_cleanup`, `clear_purge_block`, `clear_old`; `set_safety` explicitly simulates process/workspace facts |
| Projections | `boardPhase`, `targetBranch`, `mapRuntimeState`, `weeklyUsage` (Codex/Claude only, unknown stays null), `runBlocker`, `runOrder`, `dependencyProblem`, `previewPurge`, `previewClear`, `machineDefaults`, `routingSource`, `routingProblems`, `previewRoute`, `previewGrid`, `tierFor`, `effectiveCeiling`, `ceilingProblems`, `ceilingState`, `coordinatorResolution`, `profileState`, `profileDifferences`, `configIssues`, `grantsFor`, `accessFor`, `mcpTools`, `toolWarnings`, `mcpProblems`, `mockConnect` |

Integration is separate from acceptance. Squash reaches Done; creating a PR leaves work unmerged at Ready to merge. Target inheritance is task override → sprint override → Git default branch, never the open working branch. A changed target must be confirmed again. Conflict resolution returns a fresh mock version to Ready to merge with no acceptance, so it cannot merge until the owner accepts it; it is never an automatic merge. Its checks are the fixture's mock outcomes, or **not run** when the fixture has none.

Conservative mock policies: all reported checks must pass before a plain acceptance (an empty check list is not a pass). Accepting anything less needs the explicit `openChecks` override, which records what was open; no implicit override exists, and a stale version cannot be overridden. Closing as is retains its permanent warning even after later review. Dependencies unlock on accepted, current Ready-to-merge evidence or a squash merge, not cancellation. Automatic sprints run one task at a time, wait for result acceptance, and pause on questions, failed/unknown checks, conflicts and owner stops. Concurrent writers to a shared checkout are rejected; this is a mock safety policy, not a qualified production concurrency guarantee.

Archive and purge operate only on memory. Unmerged work asks before archive; refusal keeps it in Done and postpones another automatic question by the machine's auto archive days. Discard confirmation does not mutate files. Retention counts from archive time per machine. Unmerged work, unexpected changes, live/unknown processes, an offline machine or an active dependent task block purge. Restore clears discard permission and restarts the Done clock. Sizes and freed megabytes come from the fixture and are never measured; an unknown size is left out and the preview says "at least".

The legacy `src/store.ts` and its four-phase task arrays remain separate compatibility data; they do not synchronize with `fixture.workspace`. New screens must not combine their mutable state with the new port. A real ACP adapter, durable decisions, delivery acknowledgements, actual title generation, the planning model, process checks, pairing, attachments, Git and filesystem actions remain future work. `mapRuntimeState` is a conservative status-only mapping, not a backend adapter.

## Native shell

`src-tauri/` is a minimal Tauri 2 shell without plugins, invoke handlers or granted capabilities. Bundle generation is disabled. The locally pinned CLI is available with `npm exec -- tauri --version`; on a separately prepared Rust/Tauri system use `npm exec -- tauri dev` or `npm exec -- tauri build`.

Cargo and rustc are absent in the inspected environment. Native compilation, Cargo dependency resolution/lockfile and platform prerequisites are **not qualified**; no system toolchain was installed. Web dev/build/checks require no Rust. Native distribution is not part of this mock deliverable.

## Evidence

Historical rendered checks are recorded in [the walkthrough](../../../docs/cortex-workspace/design/desktop-direction-walkthrough.md); they do not qualify the Workspace foundation or the approved rework. Current validation is tests and text only: the owner prohibits screenshots. The Workspace frame, task session, review, conflict, Code, Git, sprint planning, Archive and Settings screens were also measured as text in headless Chromium at 1280×800, some at 1920×1080, which is not a visual review. `node src/workspace/ui/orch-fit.measure.mjs` measures the orchestration pages the same way, at five window sizes, and that changing a value moves nothing. No native-shell, screen-reader or physical-device qualification is claimed.
