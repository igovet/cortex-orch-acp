# Screen inventory

**Status: planned screen inventory. The local slice section below is built; the other screens are mock-only in the prototype or planned. The Conversation adoption section is owner-selected and planned; the built behaviour stays as described until it is implemented.**

Desktop: all-projects dashboard, per-project dashboard (Overview), project board and list, create task, task discussion and plan, the task Workers view (worker list, live worker view, finished worker view and worker message composer; Work details once Conversation is adopted), decisions, evidence and diff, file tree, editor and search, terminal, Git, connection, orchestration, worktree and update settings. The accepted desktop direction (Gate D, 2026-11-05) is the Paseo-style variant based on concept A, recorded in [desktop approval](design/desktop-approval.md); the static mockups in `prototypes/cortex-workspace/mockups/` are its visual reference.

A machine is a running Workspace process, not a project. One local machine exists at launch; remote machines connect via the server connector. The machine switcher shows one machine at a time, while Inbox and Archive aggregate all machines. Projects belong to machines and tasks belong to projects; sprints contain linked tasks. Each task is reached from its project's board, list, overview or the cross-project needs-you list, and the all-projects dashboard summarises projects without listing their tasks as a flat collection.

Android: Now, tasks and projects; decision detail, result/diff, discussion, connection/reconnect, file editing, Git and advanced terminal are accessible but secondary.

Each screen needs loading, empty, offline/stale, permission denied, conflict and error states where applicable. A result must distinguish unrun from passed checks; unknown status must not be styled as success. Decision detail identifies effect, scope and rejection consequences. Return from mobile preserves task identity and context.

## Conversation adoption

**Status: owner-selected on 2026-11-10. Built in the shared UI: the task conversation and worker messages below; in the page table, Home, the Inbox, the Board and list, New task and New sprint, Sprint planning, the knowledge and skill requests, and the shared row sizes; and the phone-width rules marked built under Responsive layout. The other page-table rows remain planned.** The visual anatomy, attention priorities and composer rules are in [DESIGN.md](DESIGN.md#conversation-model). This section replaces the earlier Dialog and Workers tabs, the plan strip and the Send button that turned into Stop.

### Saved task editing and sprint closure

Built: a never-started task has **Edit task…** in the task title band and its board/list menu; a narrow title band keeps it only in that menu. The dialog uses the shared composer (Attach, Voice, model/effort/settings and the plane labelled Save changes), plus title and checkout. Saved attachments must load first; a failed read has Retry. A stale save retains the draft and explains the refusal. Starting, pausing or failing a task never makes its original request editable again.

A created sprint has **New task** and **Close sprint…** in its title band; a narrow title band moves both into the sprint's actions menu, so the queue control and the window's own controls stay in view. New task adds to the current sprint without starting work. Closing confirms that the sprint moves to Archive and its queue stops, while unfinished tasks stay in the project and running tasks continue; nothing is accepted, merged, cancelled or deleted. Archive has a Closed sprints section with retained summaries. See [the behavior and error contract](local-task-execution.md#editing-saved-tasks-and-closing-sprints).

### Task Conversation

- The task opens on its Conversation column. A settled question reads as the question with its answer beneath it. Recorded order is kept, and one turn's activity folds into one compact line.
- Work details holds the stage sequence, the plan revision with "What changed", the worker list, and the usage and context totals. Selecting a stage filters the worker list there.
- Selecting an option only prepares the choice; the Send answer plane submits it. The free-text answer uses the same composer in answer mode, inside the question card. The general composer folds to "Send a separate message"; when it is expanded it is labelled "To Coordinator — not an answer to the question". Both drafts are preserved.
- Batch questions: answers are prepared locally, and the final submission sends the whole batch atomically. Until the service acknowledges it, the card reads "1 of 3 ready; sent together". Earlier answers stay editable until the final submission succeeds. A changed decision identity clears held choices that no longer apply, and says why.
- Paused task: Resume task comes first. A draft may be kept, and sending says that it resumes the task.
- Permission and access waits keep their own explicit Allow and Reject decision. They are not chat.
- Result: one current result, inline, with check outcomes (Passed, Failed, Not run, Unverified) and their provenance. Accept vN accepts only that exact version, and the outcome reads "Accepted vN; not committed or merged". Commit, Merge, Create PR or Close is a separate confirmation with its own press. The same decision component serves Changes and Inspector; no second acceptance flow exists.
- Stop is a task action in the title band or the lifecycle item. The stop dialog keeps its options: Stop and Continue, Close as is, and Cancel task.
- Lost reply: the draft stays, delivery is marked unconfirmed, and history is refreshed. Nothing is replayed automatically and no duplicate queued item is created.
- Queue: Edit restores the text and files only after the queue removal is acknowledged; a failed removal leaves the queue as it was. Send now keeps the original message identity.
- Enter sends in the visible delivery mode and never changes it. Shift+Enter inserts a newline. IME composition is never treated as a send.

### Worker messages

- The worker composer uses the [shared composer anatomy](DESIGN.md#unified-message-composer). Its states are those listed under Worker messages above, with "Coordinator" as the name where one is needed.

### Page adoption

| Area | Adoption |
| --- | --- |
| Startup, title band, sidebar, menus | One attention selector. One recovery message per incident. One state and one next action in the title band; repeated phase and resource strips are removed. |
| Inspector and detail panels | Closed by default in a task Conversation, behind one Details entry. Duplicate Accept and progress summaries are removed while the current item is visible. Details dock beside the conversation or replace it; they never overlay it. |
| Home | Built: one "Needs you" list in which each task waiting on the owner appears once, first its question, permission or conflict, then a result to review (with its check counts), then accepted work to finish; each line opens the task. The title band counts the same list. Projects follow. Engine limits are in the sidebar and the machine's Connections, no longer on Home. |
| Inbox | Built: Home's Needs you list for every connected machine, in the same order: a question, permission or conflict, and each sprint planning question; then a result to review, with its check counts; then accepted work to finish; then unmerged work holding a task out of the Archive. Each task appears once. A question, permission or result opens the task's conversation, where it is answered or accepted inline; a conflict opens its resolution; a planning question its sprint's planning; unmerged work the Archive. No separate answering interface. The sidebar's Inbox count is this list. |
| Board and list | Built: each card or row shows its title, its column as the state, one concise fact and, on a card, one next action. The fact is the question itself, the result's check counts, what was accepted and where it goes, or Working without a worker count; a failed or cancelled run keeps its outcome. The action is Answer, Review (a permission), Resolve, Start, Resume, Open (running work), Review (a result, which opens the conversation) or the finishing step. Stop is no longer a card action: it stays in the card's menu and on the task. Scopes, sprint grouping and filters are unchanged. |
| New task and New sprint | The shared composer, with the description first. Save draft is the default and starts nothing. Start now stays an explicit secondary action; a sprint uses Start planning. |
| Sprint planning | Built: a sprint has four tabs, Chat, Plan, Board and Summary, so the planning chat and the plan each have the whole width at every window size. The proposed plan is a line in the chat ("Plan vN proposed: N tasks") whose Open plan opens the Plan tab; on the Plan tab a chip says Question waiting, Working on the plan or Unsent message and opens the Chat tab, and Request changes opens the Chat tab with the cursor in its message box. Once the tasks exist the chat's composer stays in place, smaller, and says once "Tasks are created. Edit them on the Board." Create tasks stays explicit. Queue semantics are unchanged. |
| Knowledge and skills | Built: Collect knowledge, Refresh knowledge and New skill are written in the shared composer, with Voice, Attach disabled because the request is text only, and the machine's default route shown read-only. The plane, named "Create the task", creates an ordinary task, which does not start. |
| Changes, ready to finish, merge and conflicts | An accepted result appears in the conversation; integration stays a separate confirmation. Conflict progress is a factual inline update that links to its detail. Same-branch results use Commit, non-Git projects use Close, and a pull request is not a merge. |
| Archive | Read-only outcome rows, with details on open. Restore or an explicit cleanup is the next action. No composer. |
| Code, files, search, Git, worktrees, terminal | Reached from the secondary task or project entry. One Save action in the editor. Commit, Push, Pull and task acceptance remain different intents. The terminal is a secondary tool and never becomes a message composer. |
| Settings and orchestration | One next action per page, with detail on request. Settings stay forms that use the common row sizes. Expert pages (Files, Profiles) remain available. |
| Dialogs and menus | Each confirmation names the affected object and the consequence, and offers Cancel and one specific verb. The shared composer replaces the Request changes and conflict-guidance text fields only; commit, branch and key fields are not composers. |
| Palette, Help, project picker, not found | Search and Help are not chat. Obsolete task-tab commands are removed. Not found keeps one plain problem and Back to Home. Adding a project still does not activate it. |

### Responsive layout

| Window | Default composition | Detail |
| --- | --- | --- |
| 960 × 540 minimum | Sidebar collapsed by default fit. One task toolbar of about 44 px. Conversation at most 760 px, with gutters of at least 16 px. The composer is bounded so the reading area stays usable. No default Inspector or terminal. | Replaces the conversation, with Back to task. |
| 1280 × 800 | Sidebar at its current width, 260 px. Conversation at most 760 px, with outer gutters of at least 24 px. | A docked detail needs 640 px of conversation, 320 px of detail, a 24 px gap and 40 px of gutters (1024 px). If that does not fit, the sidebar hides first; if it still does not fit, the detail replaces the conversation. |
| 1920 × 1080 | Sidebar stays. The reading column stays at most 760 px and centred. | One 320–400 px detail may dock beside it. |
| About 820 px | Sidebar collapsed; one reading column. | Replaces the conversation, with a clear Back. |
| About 390 px | The same conversation, decision and result logic at full width. 16 px gutters, full-width question choices, and the desktop's compact controls (28 px, 32 px in a composer or dialog) with 8 px gaps: nothing is enlarged for touch. No sideways page scroll. | Full-width replacement view, or a true modal for confirmations. Back and Escape return to the exact task or item. |

- Fit is measured from the pane width. A space-driven collapse never overwrites the remembered panel preference, and the panel the owner opened last wins.
- Long check commands and diffs scroll inside their own box; prose wraps. On a phone, tables become labelled rows (built for the settings and orchestration tables: below 560 px of table width each row stacks its cells, each under its column's name, and the header row goes; the tool-access matrix keeps its columns and scrolls in its own box).
- Labels: a control's, tab's, chip's, badge's or table head's label stays on one line and its capitals sit in the middle of its box within 1 px. A bar whose left group and actions do not fit on a phone puts the group on its own line. The title band's counts are shown whole or not at all, least needed first.
- On a phone the composer footer has two rows: Model, Effort and Settings, then Attach, Voice, delivery where offered, and the plane. Every control in both rows keeps the composer's 32 px, as on the desktop. The model label truncates with its full accessible label. A composer that cannot send says why once, in its writing area; its disabled tools point to that note instead of repeating it under the row.
- With the virtual keyboard open, the focused text and the plane stay visible. Pages avoid fixed 100vh and respect safe-area insets.
- Built for every page at about 390 px: no page scrolls sideways; Home's Needs you lines read in three lines (kind and age, the question, where) and its projects table keeps the project, Working, Needs you and Review; the Archive keeps the task, its cleanup and its menu, and its filters scroll in their own strip; a list row keeps its icon, title and state; a toolbar's middle zone scrolls instead of cutting its last tabs; the project toolbar keeps the scope, Board and List (or the open area when another is open) and a More project areas menu that holds the rest of Board, List, Code, Git, Worktrees, Knowledge and skills and Settings; a setting puts its control under its description; New task's two places to work stand one above the other; the sprint summary's counts stand two by two; settings and orchestration tables read as labelled rows.
- A phone layout in the shared UI is not native phone qualification.

### Accessibility

- Landmarks, a skip link to the conversation, labelled fields and named icon controls. Logical focus order matches the reading order.
- Choosing an option and submitting it are separate actions. Tabs use roving focus. Inactive question panels are not reachable by keyboard or assistive technology.
- The quiet focus line stays visible in both themes and is not hidden under sticky bands.
- Dialogs trap focus only while they are modal; Escape cancels and returns focus to its trigger.
- The full message, recipient, outcome and disabled reason are available without hover. Colour is never the only status signal.
- Announcements are bounded ("New question", "Message sent") and never re-read history. Transcript updates during dictation are throttled.
- Reduced motion removes auto-scroll animation, continuous repaint and any pulse used as the only activity signal.
- The layout must hold at 200% zoom and at 320 CSS px without losing a required action. This is a requirement to test, not a claimed result.

### Voice in the shared composer

- Voice sits in the tools row of every message composer where dictation applies. Dictated text is inserted, never sent, except through an explicit Insert and send where the parent action truly submits, using the same delivery and validation as the plane. It is not offered in stop or automatic-resolution confirmations.
- Errors keep the specific wording in [Voice input](#voice-input) and say that nothing was sent.

## Local slice (built, real mode)

The local desktop slice connects these conversation controls to the real service and Cortex runtime:

- Startup shows only the Cortex logo on the app's theme background, with nothing else, until the runtime projection and product facts (sprints, planning/archived tasks and owner decisions) can be presented together. After 2 seconds the slot under the logo says "Loading workspace…"; after 15 seconds it says "Still waiting for the service. The first start can take longer." A first read that fails shows "The workspace could not be loaded.", its reason and Try again under the logo, and the logo does not move. A service that cannot be reached is not a startup failure: an offline relaunch opens the offline workspace, with its Offline notice and Retry. Once the workspace has opened, its frame stays, so a retry never drops back to the logo. Background rereads keep the last coherent view until that pair is ready. A required read failure keeps that view and offers Retry; it never exposes the raw runtime task placement.

- Project registration and explicit activation. Registering a project does not activate Cortex or start work. Adding a project is described under [Add project](#add-project).
- A project whose Cortex is not working names the reason on its own page, with Try again. The machine stays Connected and every other project keeps working. Offline means only that the service itself cannot be reached.
- A description-only draft with per-task model/effort/access/language and main/worktree choices. Ctrl+Enter saves it; saving starts nothing. Automatic title generation is explicitly unavailable; rename is connected.
- Task-owned Changes, guarded merge/PR and conflict controls, real archived-row purge, project overrides, service-owned automatic queues and project removal. The project menu, worktree menu and Code open VS Code or the default application when one exists, and say what to install when not. Code edits checkout and worktree files with tabs, a dirty marker, Save, Autosave and conflict choices; see [editing files](local-task-execution.md#editing-files). See [task execution](local-task-execution.md) for safety refusals and incomplete surfaces.
- Explicit start and observed progress. Live data carries no freshness stamp; "Last confirmed" with its age appears only while the service is away.
- Unconfirmed task state. When the machine is connected but Cortex cannot confirm whether a task is running or stopped, the task's title shows one status, Not confirmed, and one alert notice explains it: "Task state not confirmed." with the reason that the machine is connected but cannot confirm whether the task is running or stopped, and the action Re-check state. A reply that is still unconfirmed says so and links to Machine settings → Diagnostics; a failed read keeps the notice and asks to try again. Re-check rereads the existing task state only: it resumes, stops and replays nothing. A confirmed read removes the notice and restores the composer and task controls. While the state is unconfirmed, sending and worker messages stay off and the disabled composer names Re-check state. A disconnected machine is a different case, with its own notice, "Machine disconnected.", the same Re-check state action and the Diagnostics link. Board cards and list rows show Process state unknown on a connected machine and Last known status on a disconnected one.
- Identified single and batch questions. Several open questions, of a task or of sprint planning, are numbered tabs of one card: the owner sees how many there are, reads any of them, answers in any order and can change an answer until the last one goes. Multiline answers are kept unchanged; a batch submits atomically. Paused task-level questions remain visible and require Resume before answering. A settled question reads as the question with its answer under it, and the Inbox says how many questions a planning run has open.
- Durable owner/coordinator conversation, plans and tool activity, in recorded order. Streamed text reads as the message it was, one turn's tool calls fold into one line with its steps, and machine identifiers and display updates are not shown. First result is `Result v1`, independent of its runtime entity version.
- A check that did not pass is listed in the Inspector by name, with its recorded reason or what that outcome means.
- A sprint plan lists task titles, one line each. Read more opens a right sidebar with the title, dependencies and full description, which scrolls on its own and is always docked beside the plan, never over it. Panels give way by one rule ([sprint plan](#sprint-plan-list-graph-and-full-view-desktop)), and on a window with no room for both, Details takes the plan's place with a way back. Escape closes it.
- All work names sprints on the board and the list: each column and status group shows standalone tasks, then each sprint under its name, and a card repeats the sprint's name in its footer. The heading leads to that sprint.
- A task waiting for the owner, in Needs you or Review, is tinted on the board and the list and marked in the sidebar. Same-branch and non-Git results use Finish in the sidebar; the inspector describes committing or closing, not merging. Review the diff opens the result-bound Accept control before the separate commit or merge confirmation.
- A card that changes status crosses to its new column instead of vanishing, and a folded column opens around it. Reduced motion moves it at once. A manual drag uses the card’s measured size. Start stays on the pressed card through background reads and shows a refusal there, with a retry path; sprint queue mode does not silently disable an otherwise startable draft.
- Every action shows its loader on the control that was pressed, after a short delay so a quick action shows nothing, and that control takes no second press until the action ends. Enter in a composer counts as pressing its send button, and an action from a menu shows on the control that opened the menu.
- A machine-persisted queue. Edit removes an unsent item and restores the composer; Send now captures steering even during a running turn. Captured text is not replayed automatically after a lost response. Each queued row is one line: its text shortens and opens whole on a press or Enter, and Edit, Send now and remove keep their own box. The row shows no attachment chips; the message keeps its files, which show again in Edit and open from the conversation once sent.
- Live and finished workers from retained runtime rows, including prose retained at tool boundaries, result summaries, the whole public report when it is opened (retrieved in bounded pages, never shown in part) and all available combined task/command/guidance history automatically loaded by attempt using byte-bounded continuation pages, including older guidance attachments and delivery status. There is no Load older control; a failed read retains history and offers Retry. Follow and the reader's scroll anchor stay unchanged while pages arrive. Checkout locations say Project folder or Worktree separately from the branch; the project title chip shows the current open branch, not the Git default.
- Direct worker messages recorded for that channel and delivered with its next successful Cortex tool response. Delivered does not mean read or acted on. Immediate interruption and withdrawal remain unavailable.
- Stop worker through runtime cancellation; Stop command for an exact running gateway command with a required reason. Native actions have no command-stop receipt. Task Stop is a durable resumable pause, separate from those cancellation controls; task-level and pending worker questions/access waits survive it and service restart through a recorded new-worker continuation. Individually cancelling a worker still ends its waits.
- Persisted review comments returned to Cortex, and result-bound requests to rerun a check. These request work; they never mark a check passed themselves.

Every form for a message or new work, including sprint creation, planning messages, Request changes, diff comments and conflict guidance, supports picked/dropped files and clipboard images with removable chips and image thumbnails; saved files open from their task, sprint or message. Other areas are enabled only where their service capability is present, and otherwise disabled or guarded in real mode; the labelled demo prototype keeps its mock screens. The theme is chosen only in application settings. Reopening the window restores history and queue from the machine. Restarting the service preserves a recorded task pause and allows explicit Resume through the runtime's recovery path. It does not revive lost engine processes: pending worker decisions continue through an explicitly recorded replacement, with access revalidated. Cancelled worker-local waits do not revive; unavailable controls refuse rather than replaying work.

## Core session controls (desktop)

These controls keep the working principles of the current Cortex session configuration (see [requirements](requirements.md) CW-R26). Each control sends an intent; the value shown as current is the one the runtime confirms. Each control needs unavailable, stale and permission-denied states, and an unsupported value is never silently replaced.

Task composer (task discussion; the create-task form reuses the same controls for optional settings):

- Model: provider and model, from the runtime's current route catalog. A model the route does not support is shown as unavailable with its reason.
- Effort: the effort list for the selected model. Changing the model resets effort, and the reset is shown.
- Access mode: Development, Guarded or Auto, each with its own description. An unknown stored value is shown as Guarded, never as Auto.
- Language: Auto, English or Russian. It applies to Cortex's own status, question and stop text, and the control says so.
- Send mode: Steer or Queue, described under Task send modes below.
- Voice input: a microphone control that places dictated text in the composer for review, described under Voice input below. Local Russian dictation is built in the Linux desktop slice; Paseo's composer dictation is its reference ([CW-R31](requirements.md), [CW-U11](backlog/CW-U11.md)). Dictated text is never sent automatically.

Task header and inspector:

- Read-back line: the effective provider, model, effort, access mode and language as confirmed by the runtime, with the configuration revision they come from.
- Context: the last reported context occupancy and its age. With no snapshot the context value is not shown; it is never shown as zero or estimated from cumulative tokens.
- Usage: token counts, each labelled observed, engine-reported, estimated or unknown. Plan usage is labelled with its basis (at route selection, or a provider window), and a missing value stays explicit.
- Engine limits: for each connected Codex and Claude engine, the percentage of the weekly limit used, the window and the reset time, with the time of the snapshot. An unknown value is never shown as a number or as zero. When an engine has no limit snapshot, its row carries one state line, "Limit usage unavailable", and no placeholder in each field. OpenCode is not shown, because it spans multiple providers ([CW-R36](requirements.md), [CW-U10](backlog/CW-U10.md)).

Machine settings:

- Default model, effort, access mode and language for new tasks on the selected machine, saved through the machine's orchestration settings ([CW-U07](backlog/CW-U07.md)). Offline, stale and unsupported saves are shown as not saved.
- Engines and connections: connection status, and the engine limit usage described under Task header and inspector for connected Codex and Claude engines.
- Orchestration: routing, model, limit, tool and profile settings for this machine, specified under [Orchestration settings (desktop)](#orchestration-settings-desktop).

The small sprint dependency graph under the plan's list uses its intrinsic layout size, shrinking only when the panel is narrower, and only while its labels keep at least 11 px on screen (a scale of 0.92 or more, at most 320 px tall). A one-task plan never scales its node to fill the panel. A plan too large for that shows one row instead, such as `Dependency graph · 57 tasks in 9 layers`, with Open graph.

## Sprint plan: List, Graph and Full view (desktop)

The plan has its own tab beside the planning chat (Chat, Plan, Board, Summary). The plan header holds a chip for what waits in the chat, when anything does, the plan's name and count, a `List | Graph` switch and the plan's own actions. List is the default. The owner's choice is remembered and never changes by itself with the plan's size; the switch is hidden while there is no plan. Edit opens the editor in List, and the Graph then previews the draft.

The Graph view stacks a toolbar, the canvas and a status band under the plan header; the minimap is the only thing drawn on the canvas. Full view (the Full view button or E; Exit full view or Esc to leave) fills the window under the title band and tab strip. It hides the sidebar, the conversation, the sprint header and tabs, info strips and the plan header, and keeps warning strips, the stopped-planning strip with its reason and Retry planning or Resume planning, and Details. It writes no preference, so leaving it restores every panel as it was. It is not kept across a restart and is not the system's fullscreen.

Panels never float over the plan. The plan keeps at least 640 px in Graph and 420 px in List; with less, the sidebar gives way, and on a window under 940 px Details takes the plan's place with Back to graph. Details is always docked at 300 px. Giving way writes no preference, and space returning brings the panel back. The panel opened last wins: showing the sidebar (Ctrl+B) where there is no room closes Details and keeps the selection, and opening Details lets the sidebar give way. Ctrl+\\ moves between the Chat and Plan tabs. The chat stays mounted while the Plan tab is shown, so an unsent message survives, and the plan header's chip says Question waiting (or 3 questions waiting), Working on the plan or Unsent message and opens the Chat tab. Below 800 px of window height the Graph view keeps only warning strips and the stopped-planning strip. Request changes opens the Chat tab, leaving Full view, and puts the cursor in its message box, keeping the selection and the view, at every window size. Menus opened from the graph toolbar, Layers from the More menu included, stay whole inside the window.

Navigation. Zoom runs from the lower of Fit and 50% up to 150%, in steps of 1.25, and the percent opens Fit (F), Fit matches (Shift+F), 100% (0), 75% and 50%. Fit shows the whole graph with 24 px margins and never above 100%. A plan opens at Fit; zoom, position and collapsed layers stay for the tab, per sprint and plan version. Wheel pans, Shift+wheel pans sideways, Ctrl+wheel zooms at the pointer, and a drag of 4 px or more pans. Nothing in the canvas animates. The minimap, 176 by 112 px in the bottom right corner of the canvas, shows only while part of the graph is off screen. It is off by default on a canvas under 760 px wide, where the first press of M or its toggle turns it on; a click or drag on it moves the view, and arrow keys pan while the focus stays on it. Under 760 px of toolbar Layers and the minimap move into a More menu, under 640 px Status too.

Finding. From 8 tasks the toolbar has search, which matches the task number (`17` or `#17` puts task 17 first) and the shown title, case-insensitive, never the description. The count beside it reads `2 of 7`, `4 of 57` for a status filter alone, or `No matches`. Enter and Shift+Enter step through matches, a step into a collapsed layer opens it, and Esc clears the search, then returns to the canvas. The Status filter (`Status · 2` while active) appears once the plan's tasks exist and never for a past version or a draft; its checkbox rows count the tasks in each state and the popover stays open. Search and filter combine; tasks that do not match dim in place and nothing moves. Dimmed text uses the quieter text colour, never transparency, and keeps 4.5:1 against the canvas in both themes. When nothing matches, the band says `No task matches "zzz".` with Clear search and Clear status.

Selection. A click or Space on a task lights only its links: Direct by default, or Whole chain (T). What it needs is drawn solid, what it unblocks dashed, and the rest dims. Only a task the owner picked dims anything. The band then reads `Task 17 · Needs 3, 5 · Unblocks 20, 21` with the title, Start when the task can start, Open task and Details. A double click or Enter opens Details; a click on the empty canvas clears the selection.

Layers. Tasks group by dependency layer. From 13 tasks in 2 layers a heading such as `Layer 3 · 12` sits above each column and a click on it collapses the layer; the Layers popover lists every layer, with Collapse all, Expand all and, once tasks exist, Collapse finished layers. A collapsed layer is one band with its task count, up to three states worst first, its matches and its linked tasks; edges into it merge, counted `x3`.

Compact display. Titles stay 12 px on screen at every zoom up to 100%. At Detail (80% and up) a task shows its number, title and state; at Title (50 to 80%) its number and title; at Number (30 to 50%) its number, with its state icon from 42%; at Overview only a box marked by its state. At Number and Overview the picked task, the current match, the task with keyboard focus and the matches carry a pin with number and title, at most 12. A task that is not loaded is a dashed box that reads Not loaded; no plan state is drawn green. Past versions read `Past version · read only` and drafts `Draft · not saved`, both without task states; a draft's band says `Change titles and dependencies in List.` with Open list.

Keys, with the canvas focused and never while typing: the arrows move to the nearest dependent or dependency and up or down a layer, Shift+Left and Shift+Right cycle the same task's dependencies or dependents, Home and End go to the first and last task, Space selects, Enter opens Details or expands a collapsed layer, C collapses or expands the task's layer, + or =, - and 0 zoom, F and Shift+F fit, Ctrl+arrows pan, / or Ctrl+F focus search, M toggles the minimap, T the links and E Full view. Esc steps back one thing at a time: search text, then Details, then the selection, then Full view. The canvas is one Tab stop with roving focus, drawn with the quiet focus line under keyboard use. Each task is a button named with its number, title, state, needs and unblocks, and one polite status region announces counts and moves. Help and Settings → Shortcuts list these keys, and the palette offers Show graph, Fit graph and Full view.

## Task workers (desktop)

The task view opens on the conversation ([Conversation adoption](#conversation-adoption)). There is no plan strip or tab bar: a task bar states progress in words and keeps Work details, Changes, Code and Terminal one activation away, with their routes, and folds them into one Details menu on a narrow pane. The stage sequence (Planning, Execution, Review, Verification, Ready to merge) keeps its marker, which is a text or icon and never bold text, so no layout shifts ([CW-R34](requirements.md)); the plan revision with its "What changed" link and the context and task token totals move there too. Selecting a stage filters the worker list in Work details to that stage.

The Dialog keeps a compact row for each worker in the turn: role, objective, elapsed time and status. A row opens that worker directly in the full Workers view at every width. There is no intermediate side-pane mode or Full view switch. Planned: the same worker rows appear in the Conversation column as compact activity and open the worker in Work details.

The Workers view keeps the worker list on the left and the full activity stream on the right. The stream reads like Conversation: the worker's messages in order with each run of tool steps folded into one collapsed block, and a finished worker's result, checks and report at the end of the stream, immediately visible. The Coordinator entry shows the coordinator's own recorded activity in the same form and refreshes while the task runs. Narrow windows use a worker picker instead of the side-by-side list.

Worker list:

- Groups: Coordinator (one row showing planning, idle, waiting for you or writing the result, with its route and context); Execution, Review and Verification in plan order; and Finished. Finished is collapsed while the task runs. A worker moves there only after its stage has ended, so rows do not jump mid-stage.
- Each row shows status, role, objective, elapsed time (frozen at a terminal state), the model and effort the runtime confirmed, the current activity, context occupancy and tokens, each shown only when the runtime has a value. An absent value is not shown, and it is never shown as zero. Up to two badges appear: unread activity, a waiting question, a pending message count, and an attempt count after a retry or route fallback.
- Sort by plan order (default), needs-you first, or recently active. Filter by role or objective text.
- A worker card opens its live view directly.
- States: planning with no plan yet ("Planning. Workers appear when the plan is accepted."); a direct answer with no workers; and activity that could not be loaded. The last case keeps the rows and shows "Activity could not be loaded · Retry". It is never shown as "no workers".

Live worker view:

- Header: role, objective, attempt number, status, elapsed time, route and route choice, context meter, tokens, a "Stop a command" control (only while gateway commands are running) and a menu with the report, a copy-id action and the instructions sent to the worker.
- Stream: tool steps fold by turn into a collapsed block whose line says what happened, such as "Edited 2 files, ran 3 commands, and read 5 files". Opening it lists the steps the same way in Conversation and the worker view: one row per recorded call identity, merging input, status and output updates, with an icon, a plain name and its target on one line (Read and a path, Edit and a path, Shell and a command, or the tool's own readable name). A step opens on click only when there is something to see beyond its row: a command with its full command line and output, or an edit with its file (linked to Code) and recorded before/after text. A step with nothing more to show, such as a read of a named file, is a plain line with no disclosure. Opened blocks and steps stay open, and the reading position stays put, through every live update, including a block growing, its first steps leaving the bounded live window and the turn finishing. There is no verbosity setting. Stored Markdown escapes are removed in prose, results and finished worker-list previews, while inline and fenced code retain their real backslashes. Worker notes span the content columns; internal message prefixes are not displayed. Tool labels, icons, Worked summaries and which calls appear reuse the Paseo integration’s presentation functions, so both clients show the same rows; an engine's raw tool name never reaches the screen, and an unknown tool gets Paseo's humanized name. Thoughts, context figures and other engine status updates stay hidden before grouping and counting; report bodies are never shown, and the published worker result remains visible. Native commands never invent an exit code or offer gateway Stop.
- Recorded output: the output of an opened step, a command's output tail and an edit's diff each sit in one labelled box that keeps lines exactly as written. A long line scrolls sideways inside its box, and the box scrolls vertically as well, up to 280 px (320 px for a diff). Nothing is cut off at the column's edge, and the column and the page do not scroll sideways for it. The box is a named stop in the Tab order; once focused, arrow keys, Shift+wheel and a trackpad move it. Its name is the label, such as Command output, Output, Diff or Edit.
- Live activity: a quiet Working… indicator remains while the task or worker runs, with Reading…, Editing… or Running command… only when the recorded activity establishes it. It stops for terminal or waiting states and respects reduced motion. The stream arrives at tool boundaries; missing output and clipped previews are explicit.
- Follow: on when a running worker is opened. Scrolling up turns it off and shows a pill "↓ 7 new · Follow". End or the pill jumps to the latest item and turns Follow on. Both Conversation and Worker feeds keep the bottom pinned as streamed text or expanded Markdown grows. With Follow off, appended content keeps the scroll offset and visible content still; a row above the viewport changing height preserves the visible reading anchor. Returning to the bottom resumes following. Content updates do not override a held scrollbar drag.
- Find in this activity filters the rows loaded so far by their text and shows "No activity matches this search." when none match; while a search is active, collapsed tool blocks open. Automatic paging loads the rest of the history, so the search reaches all of it once loading finishes. Refreshing the same worker attempt preserves loaded pages and in-flight page reads; switching attempts discards them.
- Stop a command: pick the running command, enter a required reason, then confirm. States are sending, requested, stopped, missed (the command had already finished) and refused. The worker receives the reason and continues. This is the only control that interrupts one action of a worker.
- Waiting for you: the question card is pinned above the worker composer, with options as buttons, "Other…" and Stop the task. "Other…" opens a free-text answer field with line breaks (Shift+Enter) and attachments, as in the main composer. Files can be dropped into the field, and a copied screenshot can be pasted into it ([CW-R39](requirements.md)). The composer placeholder becomes "Answer the worker's question…", and text sent there is an answer.

Finished worker view:

- Header: status (Finished, Finished with issues, Failed, Cancelled or Interrupted), duration, route, final context, tokens and attempts.
- Result: a card at the end of the stream with the worker's state, "Open report" and the attempt's own summary, clipped at 16,000 characters with a visible note.
- Checks: one row each. Outcomes are passed, failed, not run, unverified, closed by rerun, retracted and superseded. Not run is never shown as green. The blocker or evidence gap is shown for not-run and unverified rows. A missing record shows "Check details unavailable", and an empty list shows "No checks reported".
- Report: "Open report" retrieves all bounded public pages before showing the complete Markdown document, without Next page. Headings, lists, tables, quotes and code are formatted; code keeps literal backslashes. Supplied HTML stays inert. Relative, HTTP(S) and mailto links are allowed; executable/unknown schemes remain text. An open report survives background refreshes. Very large reports still require initial rendering time; this is not a guarantee of frame timing on every machine.
- Changes by this worker: the file list with counts and per-file diffs, labelled "From recorded edits (may omit commands' side effects)". "Open task Changes" shows the real branch diff.
- Activity: the full history, read-only, with Follow off. Re-opening a finished worker always restores it.
- A finished worker accepts no messages. Its composer is replaced by "This worker has finished. Ask about this worker…", which pre-fills the task composer with a worker reference.
- Review stage: reviewer rows show the verdict (Approve, Changes requested or Block). The reviewer view lists findings with severity, file and line, each linked to the cited activity.
- Verification lane: each runtime command with its argv, elapsed time, exit code and output tail. A failed verification shows a readable reason followed by its code.

Worker view states: loading (skeleton rows and header, never "empty"); just dispatched ("Waiting for first activity"); history still loading ("Loading recorded activity…" while the remaining pages load automatically, with the rows loaded so far kept); history read failed (loaded rows kept, with the error and Retry); offline machine ("Offline · last update 13:47", with the composer disabled and the reason shown); interrupted ("Worker interrupted · Coordinator will retry or re-plan", and pending messages become "Not delivered"); narrow windows below 1280 px hide the inspector; below 900 px the full stream uses a worker picker, with horizontally scrollable tabs.

## Worker messages (desktop)

A composer at the bottom of a running worker's view reads "Message <role>…". It has an attach control, a send-mode chip, a voice control and Send. Files can be dropped into it, and a copied screenshot can be pasted into it ([CW-R39](requirements.md)). The worker's model, effort and access mode are shown read-only, because the route belongs to the runtime. The same composer opens from the worker row menu, and a Dialog worker row opens the full worker stream.

- Send mode "At next step" is the default. The message is appended to the worker's next Cortex tool result, or to the next prompt of the same session if the turn ends first. A worker that uses only native engine tools receives it at its next Cortex call, and the UI says so: "Delivered when the worker next calls a runtime tool".
- "Interrupt now" is shown disabled, with the reason "Not available for this engine yet", until the engine is qualified for it.
- Message states, shown as words as well as icons: Sending…; Pending · next step (Withdraw is available until delivery); Delivered at a time (recorded in the worker's activity, which is not proof that the model acted on it); Withdrawn; Not delivered · worker finished (Send to Coordinator instead, or Discard); Not delivered · stopped, cancelled or replaced (Send to new attempt if one is running, Send to Coordinator, or Discard); Failed · not sent (text stays in the composer, with Retry).
- Several pending messages are delivered together, in order, at one delivery point. A pending badge appears on the worker row.
- The owner's text reaches the worker verbatim, framed as guidance that changes no scope, permission, review or acceptance criteria.

## Task send modes

- Enter sends in the delivery mode shown beside the plane, Send now or Queue, and only that visible choice changes the mode; Ctrl/Cmd+Enter does not queue. Shift+Enter inserts a newline. Help and Settings → Shortcuts say the same. Key bindings are proposals until the owner decides them ([open choices](DESIGN.md#open-choices)).
- Steer: captured at once, and the acknowledgement says it will be applied after the current step. It is applied after the current workers, review and verification, and the task is re-planned. Steer cannot be edited or removed after capture; a correction is a new message. Its states are Sending…, Received · applies after current step, Applied · plan revision, or Failed · not sent (text restored, with Retry). A refused capture means nothing reached Cortex, and running work continues.
- Queue: held by the client and not yet sent. Queued items sit above the composer. Each item shows its text on one line and offers Edit, Send now (which sends it as Steer), remove and reorder (drag or Alt+Up/Down); its files are not listed in the row. Edit removes the item from the queue and returns its text and files to the normal composer, where they can be removed before it is sent again; the queue never edits an item in place. Send now sends the files with the message, and they then open from the conversation. The queue is as wide as the chat column. Its background is transparent at its edges and in the gap between the queue and the composer, so the messages behind it stay visible. Its controls are never clipped; long text shortens with an ellipsis and opens whole on a press or Enter. Queue is sent as an ordinary message when Cortex is idle (waiting for you, or finished). While the queue is not durable, its label reads "Queue is kept until the app closes".
- Edit over a non-empty composer draft asks first: Replace draft, Keep both or Cancel. The composer never overwrites a draft silently.
- When Cortex is idle the two modes are identical, so the mode chip is hidden.
- While a coordinator question is open, the answer composer sits inside the question card, and the general composer folds to "Send a separate message". Its answer field supports line breaks and attachments, as in the main composer, including drag and drop and paste ([CW-R39](requirements.md)).
- The plane never becomes Stop: Stop is a task action in the title band. Stop opens the stop dialog, which offers Stop and Continue, Close as is, and Cancel task; see [CW-U08](backlog/CW-U08.md).

## Voice input

**Built local subset:** Ctrl+D or the mic toggles local Russian capture in task,
worker, New task, planner and question-answer composers, and every other shared composer. A live, wrapping transcript preview
appears inside the composer with loading/download progress. Stop retains the
preview; Cancel discards it; Insert appends it to the editable draft. Insert and
send is a separate button where that composer supports sending. The default
PulseAudio source and sherpa-onnx small Russian model are used with no cloud
provider or audio storage. New-task dictation never saves or starts a task.
See [local voice behavior and limits](local-task-execution.md#local-voice-input). Recording on an installed desktop package has not been qualified; the source path is wired.
The broader controls below (provider/language selection, level meter, insertion at
cursor, push-to-talk and Esc/Enter bindings) remain planned, not built.

- The Voice control sits in the tools row of every message composer where dictation applies (see the [composer anatomy](DESIGN.md#unified-message-composer)): task, worker, sprint planning, Request changes, diff comments and New task description.
- Start: a click toggles dictation. A press-and-hold push-to-talk mode is proposed, pending the owner's decision. Ctrl/Cmd+D toggles it in a focused composer, and it is not bound while the terminal has focus.
- While recording, a recording bar replaces the composer toolbar at the same height. It shows a red indicator with elapsed time, a level meter (static under reduced motion), a language choice (Auto, Русский, English or an installed model), Cancel (Esc), Insert (Enter), and Insert and send (button only).
- Live transcript: partial text appears in grey italics at the cursor in the draft. It becomes normal text on Insert. "Processing…" appears between stop and the final text.
- Dictation never sends by itself. Insert and send uses the composer's current send mode.
- Errors appear inline, and text input stays available: "Microphone access is blocked by the system." (with a link to system settings); "No microphone found."; "Dictation needs a speech model that is not downloaded." (with Download and Choose another); "Speech-to-text key is not set for this machine."; "This machine's voice service is offline."; "Microphone interrupted. Waiting to resume…"; "Transcription took too long." (with Retry and Discard).
- Provider: a local model is the recommended default, and an OpenAI-compatible service is the alternative. The owner selects the provider in the machine's voice settings ([CW-U07](backlog/CW-U07.md), [CW-U15](backlog/CW-U15.md)). The default and the machine that transcribes a remote task are open decisions.
- Privacy: audio is streamed only while recording and is not stored by default. A recording indicator is visible for as long as audio is captured. Dictation starts only from a user gesture.

## Tabs (desktop)

A strip above the content area holds browser-style tabs. Each tab shows a title from its screen (task title, project name, New task, Settings) and a close control; + opens a new tab on Home. Each tab is a separate workspace: anything from the sidebar, a link, the palette or a deep link changes only the active tab. The window's address follows the active tab, and each tab has its own Back and Forward.

- Switching tabs keeps everything in the tab as it was left: typed text and attachments, scroll position, expanded rows, selected worker, and code and terminal state. A hidden tab stays mounted and costs nothing: its timers, polls, animations, terminal and editor updates stop, and it reads the machine again when it is back on view. A recording stops when its tab is left, and what was heard stays ready to insert.
- Create, close, switch, and drag to reorder. Ctrl+T new, Ctrl+W close, Ctrl+Tab and Ctrl+Shift+Tab cycle, Ctrl+1 to Ctrl+8 jump to that tab and Ctrl+9 to the last, a middle click closes. While typing in a terminal, Ctrl+T and Ctrl+W stay with the shell. In a plain browser the browser itself keeps Ctrl+T and Ctrl+W.
- Closing the last tab leaves one fresh Home tab, never an empty window. Closing a tab with unsent text asks first.
- The tab list, each tab's route and the active tab survive a restart. A restored tab is opened only when it is first shown, and Back and Forward start over. Unsent text does not survive a restart.

## Keyboard and visual rules

- Esc never stops a task. In a recording, Esc cancels the dictation. This deliberately differs from Paseo and is an open decision.
- Ctrl+1 to Ctrl+9 switch workspace tabs, so a task's own tabs have no number keys.
- Proposed keys: Ctrl+Shift+P toggles the plan graph; Ctrl+Shift+F filters workers; End jumps to the latest activity; Ctrl+F searches the worker view; Ctrl+. opens the command-stop picker (never an immediate stop); 1 to 8 choose a question option. Each key is checked for conflicts on the target platform before implementation.
- No focus or press highlight. A click, a press, a route change or a dialog opening leaves no outline, ring, border change, fill or accent on any control, and a text field shows only its caret. Selection that means something (the current page, the open task) stays: it uses a neutral fill, a visible text or icon marker and `aria-selected`; a fill alone does not meet the contrast needed for selection. Selected labels never change font weight or size, and selection never shifts layout ([CW-R34](requirements.md)). There are no coloured side stripes. Only the running marker pulses, and it is static under reduced motion.
- Keyboard position: moving with Tab or an arrow key draws one quiet 1 px neutral line around the focused control (`--focus-line`), and a text field shows only its caret. The only focus mark is that line, and the next pointer press clears it. A control that a dialog or panel focused as it opened shows the same line, where the keys act, until the next press. A row that fills its scroller or list draws the line just inside itself, so nothing cuts it. The app records the last way of moving in `data-keyboard` on the root element, because the engine's `:focus-visible` also matches after a click that opens a dialog. Checkboxes and radios are drawn by the stylesheet, so no native focus ring can appear on them.
- Panel toggles: each icon that opens a panel (sidebar, worker pane, terminal, inspector) shows that panel's real state by colour alone: lighter while the panel is open (`--panel-on`), darker while it is closed (`--panel-off`), in both themes. Hover and click add no fill or outline, the icon never moves, and a closed panel always reopens from the same icon. On a screen with no such panel the toggle is not shown and its place stays empty, so the band never shifts. The worker pane toggle is shown only where it controls the pane: the Dialog tab at 1800 px and wider.
- Hover titles: no element uses a native title, so nothing pops up under the pointer. A control that cannot be used says why in visible text: in its bar's reason slot beside the controls or, where the bar has no slot, in a box that a press or keyboard focus opens beside the control. The reason is never drawn inside the control and never opens on hover. A control that nothing could ever use on this screen is not offered at all.
- Window edges: the frameless window keeps the outer 5 logical pixels of every edge (multiplied by the display scale) for edge resizing. Scrollers at the right and bottom edge stop short of that zone, so a scrollbar can always be grabbed and dragged and the window can still be resized from its edges.
- Shadows: none, or so faint that they are barely visible. No card, popover, queue or composer uses a visible shadow.
- Interface words: a label never names the program, because the whole program is Cortex: "Use", not "Use in Cortex". The agent you talk to reads as Coordinator where a name is needed. Labels and hints are English only.
- Task state: a same-branch review points to Ready to finish, not Ready to merge. The inspector shows the completed outcome once, without repeating the same status as a second fact.
- Identifiers: task, assignment, report, route and connection ids and build hashes never appear in the interface. Diagnostics is the one exception, and it shows the build and connection identifiers for support. A version elsewhere reads as `1.0.6`. Two connections offering one model are told apart by connection name.
- Counts: nothing to count shows nothing, never "All 0". What differs from a profile is listed in words, for example "Claude: 4 sessions at most, was 2".
- Titles and names are edited by clicking them. There is no separate Rename or Edit control.
- Selection: text can be selected and copied everywhere, with Ctrl+C. Buttons, links, the title band, the sidebar and draggable cards are excluded.
- One piece of text, such as a path, is cut with an ellipsis and never broken across lines. Anything cut shows whole in a box beside it when it is pressed, or when keyboard focus reaches it and Enter or Space opens it. It never opens on hover, and no native tooltip is used.
- Absent values: a value Cortex does not have is not rendered. No dash (—) or placeholder glyph stands in for it, and an unknown value is never shown as zero or as success. When a whole panel has no data, the panel carries one state line with its reason, not a placeholder in each field.
- Diff colours are always paired with `+` and `−` text. Every icon has a text label.
- A polite live region announces real transitions only, such as "Implementer finished with 1 issue". Polling never announces.

## Theme (desktop)

- The theme is System, Light or Dark, chosen only in application settings. The default is System. The sidebar and window chrome have no theme switch ([CW-R33](requirements.md)).

## Worktree settings (desktop)

- Settings › Worktrees is a per-machine page. The real service reports both the current folder and its absolute default. The default is `$XDG_DATA_HOME/cortex-workspace/worktrees` in the service environment (fallback `~/.local/share/cortex-workspace/worktrees`); the dev service isolates XDG data under `<channel-state>/data`, so its default is `<channel-state>/data/cortex-workspace/worktrees`. The field, "Use default" tooltip/action and example all use that reported path; reset is unavailable until it is known. New worktrees use `<root>/<project-key>/<task-id>`, where project-key is the 16-character hash of the common Git directory. The folder is typed in the built page. Saving changes only future worktrees and never moves existing checkouts.
- A new path is checked before it is saved. It must be absolute, writable and outside every registered project checkout. A failed check is shown beside the field with its reason, and the saved path does not change. An offline, stale or unsupported save shows "Not saved", as in machine settings.
- Task creation offers two checkout choices: the project folder on its open branch, or a dedicated worktree under the saved root. If the saved root is unavailable when a task starts, the task does not start in another folder; it shows the reason.
- Project worktree options (branch prefix, setup script, files copied into new worktrees, removal after merge) stay in the project's Worktree settings.

## Orchestration settings (desktop)

Per-machine orchestration settings reach field-level parity with the Cortex configuration that Paseo exposes, in a more convenient form ([CW-R41](requirements.md), [CW-R23](requirements.md)). The field checklist and acceptance criteria are in [CW-U07](backlog/CW-U07.md). This section fixes the pages and the shared rules. No owner approval of these pages is recorded yet, and the current prototype pages do not yet meet this inventory.

Each page is scoped to one machine and shows the same context bar: machine name, connection state, runtime version, active profile and its state. Project scope appears only where it applies.

| Page | Purpose | Project scope |
|---|---|---|
| Overview | Items that need attention, each linking to its field; summary rows; the routing source in use | Shows the source |
| Coordinator | Model, effort, access mode, language and checkout that new tasks start with; exact default route under Advanced | No |
| Models | Which models may run, their tiers, effort limits and preferences; engine switches; per-connection model refresh | Yes (routing) |
| Routing | Roles and pins, work kinds and engine lean, quota windows, rules under Advanced | Yes (routing) |
| Route preview | Read-only answer to what would run for a role, work kind, complexity and attempt, from the saved or the draft rules | Yes (routing) |
| Limits and fallback | Attempts, allowed connections and data recipients, fallback failure classes, budget and consent | Yes (ceiling, narrow only) |
| Tools and MCP | Server list, add, edit and remove; tool access per role class; sandbox | No |
| Profiles | Saved snapshots of routing, defaults and ceiling, MCP usage policy and connection session limits | No |
| Files and presets | Source precedence, preserved keys, environment overrides, default preset, model-policy migration | Read-only |

Engine connections stay on the machine's Connections page, which gains the real fields listed in CW-U07.

Shared rules:

- Drafts are per surface, not per page: routing (Models and Routing share one draft and one save), user defaults (Coordinator and Limits share one draft and one save), project ceiling, MCP servers and connections. The save bar names what it covers, for example "4 unsaved changes in routing rules".
- A save over a surface that changed elsewhere is refused with a conflict state. The owner reviews the changed keys and then reapplies; nothing is overwritten blind.
- Offline, every write is disabled with its reason, last-known values stay visible, and a dirty draft is kept. Nothing is queued.
- Built-in values (role tiers, complexity shift, fallback order, effort by tier, hot band, work-kind table) are read-only, marked with a lock and "Built in". They are never controls.
- A value that is not set is omitted; no dash or "None" stands in for it. Selecting anything changes no font weight, size or layout.
- Advanced sections are closed by default and open by themselves when they hold a non-default value or an error.

Routing preview: the route preview shows the tier path, the effort, the ranked routes with the decision that placed each, and the excluded routes with their reasons. A blocked result states its reason in words. The page states that live availability, retries and plan hints are known only at dispatch.

Tools and MCP:

- The server list shows each server with its tool counts by access level, its engines, its network state, the time of its last tool check and an enabled switch. Invalid, path-missing, unmanaged-settings and disabled rows carry their reason.
- Adding a server takes three steps: Define, Connect and list tools, Tool access. Connecting starts the server in its sandbox, lists its tools and calls none. The draft stays in the client until it is saved, because a server with no granted tool cannot be saved, even when disabled.
- The tool access matrix has one row per tool and the columns Coordinator, Read-only, Writing, Consultant (fixed to Never) and Always load (Claude only). The levels cascade: Coordinator implies Read-only, which implies Writing. Removing Writing clears the higher levels, so every saved list is a subset of the list above it.
- Each tool row shows one state: Granted; New (listed by the server and never granted by default); Not offered (granted but no longer listed; kept until removed); or Not checked (typed by hand).
- Bulk actions set the selected rows to Off, Writing only, Read-only too or Coordinator too. Suggest from server applies the server's own hints as a labelled preview; it is never automatic.
- Check as, for a chosen role class and engine, shows the exact access result the runtime would use.
- Stored environment values are never shown after saving; they can be kept, replaced or removed. Redacted arguments keep their stored value until the whole part is replaced.

Open owner decisions, not in the current configuration schema: per-tool engine access (today set by duplicating a server per engine) and access per worker role. The prototype simulates tool listing, the route preview and the new runtime operations, and labels each as simulated; none is runtime behaviour.

## Application updates (desktop)

- Settings › Updates shows the installed version, the time of the last check, Check now and, when a newer release exists, Install version <x>…. Release notes appear as plain text in a scrollable area.
- Each state is shown in words: Up to date; Checking…; Update available (version and release date); Downloading (bytes received); Verifying signature and checksum; Verification failed · nothing installed; Waiting for administrator approval; Installing; Restart required; Install failed · previous version still installed; Rolled back to version <x>; Check failed · GitHub unreachable, last check <time>.
- Install does not start while a task on this machine has a running worker. The dialog names the blocking task and offers Stop first or Cancel. The update never stops a worker by itself.
- Install needs owner confirmation and administrator approval. Cancelling either leaves the installed version unchanged.
- Each machine shows and updates its own installation. The local app does not install on a remote machine.

## Project board and card actions (desktop)

- Card actions (Open, Preview, the column's primary action and the more menu) appear on hover or keyboard focus in a strip outside the card content. They never cover the card's title, branch or fact line ([CW-R37](requirements.md), [CW-U01](backlog/CW-U01.md)).
- In an automatic sprint, the queue's rule ("Passed checks leave each task ready for your final confirmation and start the next ready task.") sits behind a `?` button on the board, closed by default, and is not a line above it. A manual sprint shows no such line.
- A sprint's board, Planning and Summary say why the sprint is not moving, with an Open task link to the task that holds it: what Run sprint would refuse in an automatic sprint, or the dependency the first unstarted task waits for in a manual one.
- A sprint whose recorded tasks could not all be loaded says how many are unavailable and offers Retry, which only reads again. It is never shown as empty, and Archive stays off until every recorded task is loaded and done. An empty created sprint, and a sprint's own board, offer New task with that sprint already selected; Ctrl+N from a sprint's pages does the same.

## Ready to merge and conflicts (desktop)

- Every finished task, including tasks of an automatic sprint, reaches Ready to merge with its target branch. The target defaults to the repository's Git default branch and can be changed for a task or sprint. A local squash merge or a PR runs only after owner confirmation; there is no automatic merge ([CW-G07](backlog/CW-G07.md)). In an automatic sprint, a task that passes its checks moves to Ready to merge by itself, with no separate Accept step; the owner confirms there.
- Ready to merge always has one primary next action. A task that is not accepted yet offers "Review and merge…": its dialog shows the exact result version, checks and target, and its one confirmation records acceptance of that version first and then merges or creates the PR. A refused merge keeps the dialog open and the acceptance on record; nothing is resent by itself. An accepted task offers "Merge…" and "Create PR…".
- A project folder without Git has nothing to merge. Ready to merge offers "Close as is…" and "Set up Git…" instead of Merge and PR. Closing a result that is already accepted, or promoted after passed checks, keeps it on record as checked, without the Not verified warning. Setting up Git opens the project's Git page, where creating an empty repository asks first. New task offers no worktree or target branch for such a folder.
- Loading covers the whole action: a multi-step action such as staging every change and then committing, or accepting and then merging, keeps the pressed button loading and closed to a second click or shortcut until its last step ends. The label stays readable and a thin bar runs along the button's lower edge.
- Before Ready to merge, a check the system skipped runs automatically. A failed or not-run check offers a forced re-run. A result with an unpassed check can be accepted only when that check cannot complete for a technical reason, and only with an explicit confirmation ([CW-R40](requirements.md), [CW-U04](backlog/CW-U04.md)).
- The review's comment bar appears only in a project with Git. A comment needs a line of a diff, so a project without Git hides the bar rather than showing it disabled.
- A conflict shows "Conflict with <branch>" in words and an icon on the card and the project row. The Changes tab lists conflicts first.
- "Resolve automatically" hands the conflict to the task. The result returns as a normal diff and goes through Ready to merge for owner review; Cortex never merges it itself ([CW-R35](requirements.md), [CW-G07](backlog/CW-G07.md)).
- "Abort merge" stays visible in every conflict state. No automatic stash or reset is performed.

## Add project

The empty state, the sidebar and the palette open one flow, modelled on Paseo's. It works on the selected machine: the dialog names it, and the machine does the searching.

- **Ways in.** Search for folder, Browse and New folder. Browse is the system folder chooser and appears only for the machine this app runs on. Cloning from GitHub is not offered.
- **Search field.** A fragment such as `cedar` searches the home, the mounted disks under `/mnt`, `/media` and `/srv`, and the folders that hold registered projects. A typed path such as `~/code/ce` or `/mnt/data/ce` searches below its parent. A trailing slash lists that folder, with the folder itself first. `~` is the owner's home on that machine.
- **Results.** They follow the text after a 250 ms pause. Rows that still fit stay on screen until the machine answers, so the list narrows without blinking. Registered projects on the machine are the recent folders: they come first, and alone when the field is empty. A folder with a `.git` is marked Git repository. A typed path the search did not report is offered last, as itself, with Open this path, so a folder under a parent that cannot be listed can still be named.
- **Ranking.** An exact folder name, then a name starting with the text, then one containing it, then a match across the path, then letters in order. Shallower folders win ties.
- **Keyboard.** Up and Down move and wrap, Enter chooses, Esc steps back a page and then closes. Pressing outside closes from any page. The choice stays on the same folder when more rows arrive.
- **Choosing a folder adds it.** The project is named after the folder and can be renamed in its settings. Registering does not activate Cortex or start work. The app then opens the project.
- **New folder.** Choose a parent the same way, then type a name. The row shows where it will go, and Enter creates the folder and adds it. A name already taken is refused and the folder is left alone.
- **Refusals.** The machine's reason appears in the list, the field keeps the text, and nothing is resent. A folder that does not exist offers Create this folder.
- **A failed search.** It shows its reason in the list and the machine stays Connected. A limit that ended the search early says so in the footer. A folder the search cannot read is skipped without a word. A typed path still works on a machine with no search.
- **Layout.** Every page and state has one height, so typing, results and page changes move nothing. The selected row has the same background as the palette's, with no outline or shadow.
