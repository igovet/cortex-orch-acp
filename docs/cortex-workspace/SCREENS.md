# Screen inventory

**Status: planned screen inventory. The local slice section below is built; the other screens are mock-only in the prototype or planned.**

Desktop: all-projects dashboard, per-project dashboard (Overview), project board and list, create task, task discussion and plan, the task Workers tab (worker list, live worker view, finished worker view and worker message composer), decisions, evidence and diff, file tree, editor and search, terminal, Git, connection, orchestration, worktree and update settings. The accepted desktop direction (Gate D, 2026-11-05) is the Paseo-style variant based on concept A, recorded in [desktop approval](design/desktop-approval.md); the static mockups in `prototypes/cortex-workspace/mockups/` are its visual reference.

A machine is a running Workspace process, not a project. One local machine exists at launch; remote machines connect via the server connector. The machine switcher shows one machine at a time, while Inbox and Archive aggregate all machines. Projects belong to machines and tasks belong to projects; sprints contain linked tasks. Each task is reached from its project's board, list, overview or the cross-project needs-you list, and the all-projects dashboard summarises projects without listing their tasks as a flat collection.

Android: Now, tasks and projects; decision detail, result/diff, discussion, connection/reconnect, file editing, Git and advanced terminal are accessible but secondary.

Each screen needs loading, empty, offline/stale, permission denied, conflict and error states where applicable. A result must distinguish unrun from passed checks; unknown status must not be styled as success. Decision detail identifies effect, scope and rejection consequences. Return from mobile preserves task identity and context.

## Local slice (built, real mode)

The local desktop slice connects these conversation controls to the real service and Cortex runtime:

- Startup shows Loading workspace… until the runtime projection and product facts (sprints, planning/archived tasks and owner decisions) can be presented together. Background rereads keep the last coherent view until that pair is ready. A required read failure keeps that view and offers Retry; it never exposes the raw runtime task placement.

- Project registration and explicit activation. Registering a project does not activate Cortex or start work. Adding a project is described under [Add project](#add-project).
- A project whose Cortex is not working names the reason on its own page, with Try again. The machine stays Connected and every other project keeps working. Offline means only that the service itself cannot be reached.
- A description-only draft with per-task model/effort/access/language and main/worktree choices. Ctrl+Enter saves it; saving starts nothing. Automatic title generation is explicitly unavailable; rename is connected.
- Task-owned Changes, guarded merge/PR and conflict controls, real archived-row purge, project overrides, service-owned automatic queues and project removal. The project menu, worktree menu and Code open VS Code or the default application when one exists, and say what to install when not. Code edits checkout and worktree files with tabs, a dirty marker, Save, Autosave and conflict choices; see [editing files](local-task-execution.md#editing-files). See [task execution](local-task-execution.md) for safety refusals and incomplete surfaces.
- Explicit start and observed progress. Live data carries no freshness stamp; "Last confirmed" with its age appears only while the service is away.
- Identified single and batch questions. Several open questions, of a task or of sprint planning, are numbered tabs of one card: the owner sees how many there are, reads any of them, answers in any order and can change an answer until the last one goes. Multiline answers are kept unchanged; a batch submits atomically. Paused task-level questions remain visible and require Resume before answering. A settled question reads as the question with its answer under it, and the Inbox says how many questions a planning run has open.
- Durable owner/coordinator conversation, plans and tool activity, in recorded order. Streamed text reads as the message it was, one turn's tool calls fold into one line with its steps, and machine identifiers and display updates are not shown. First result is `Result v1`, independent of its runtime entity version.
- A check that did not pass is listed in the Inspector by name, with its recorded reason or what that outcome means.
- A sprint plan lists task titles, one line each. Read more opens a right sidebar with the title, dependencies and full description, which scrolls on its own and floats over the plan in a narrow window. Escape closes it.
- All work names sprints on the board and the list: each column and status group shows standalone tasks, then each sprint under its name, and a card repeats the sprint's name in its footer. The heading leads to that sprint.
- A task waiting for the owner, in Needs you or Review, is tinted on the board and the list and marked in the sidebar. Same-branch and non-Git results use Finish in the sidebar; the inspector describes committing or closing, not merging. Review the diff opens the result-bound Accept control before the separate commit or merge confirmation.
- A card that changes status crosses to its new column instead of vanishing, and a folded column opens around it. Reduced motion moves it at once. A manual drag uses the card’s measured size. Start stays on the pressed card through background reads and shows a refusal there, with a retry path; sprint queue mode does not silently disable an otherwise startable draft.
- Every action shows its loader on the control that was pressed, after a short delay so a quick action shows nothing, and that control takes no second press until the action ends. Enter in a composer counts as pressing its send button, and an action from a menu shows on the control that opened the menu.
- A machine-persisted queue. Edit removes an unsent item and restores the composer; Send now captures steering even during a running turn. Captured text is not replayed automatically after a lost response.
- Live and finished workers from retained runtime rows, including prose retained at tool boundaries, result summaries, the whole public report when it is opened (retrieved in bounded pages, never shown in part) and a byte-bounded tail of up to 200 combined task/command/guidance rows with Load older activity by attempt, including older guidance attachments and delivery status.
- Direct worker messages recorded for that channel and delivered with its next successful Cortex tool response. Delivered does not mean read or acted on. Immediate interruption and withdrawal remain unavailable.
- Stop worker through runtime cancellation; Stop command for an exact running gateway command with a required reason. Native actions have no command-stop receipt. Task Stop is a durable resumable pause, separate from those cancellation controls; task-level and pending worker questions/access waits survive it and service restart through a recorded new-worker continuation. Individually cancelling a worker still ends its waits.
- Persisted review comments returned to Cortex, and result-bound requests to rerun a check. These request work; they never mark a check passed themselves.

Task creation, task and direct-worker composers and Other answer support picked/dropped files and clipboard images without previews; sprint attachments remain unavailable. Other areas are enabled only where their service capability is present, and otherwise disabled or guarded in real mode; the labelled demo prototype keeps its mock screens. The theme is chosen only in application settings. Reopening the window restores history and queue from the machine. Restarting the service preserves a recorded task pause and allows explicit Resume through the runtime's recovery path. It does not revive lost engine processes: pending worker decisions continue through an explicitly recorded replacement, with access revalidated. Cancelled worker-local waits do not revive; unavailable controls refuse rather than replaying work.

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

The sprint dependency graph uses its intrinsic layout size, shrinking only when the panel is narrower. A one-task plan never scales its node to fill the panel.

## Task workers (desktop)

The task view has the tabs Dialog, Workers, Changes, Code and Terminal. A plan strip stays above the tabs. It shows the stage sequence (Planning, Execution, Review, Verification, Ready to merge), with the current stage marked by a text or icon marker and never by bold text, so no layout shifts ([CW-R34](requirements.md)), the plan revision and a "What changed" link, and the context and task token totals. Selecting a stage filters the worker list to that stage.

The Dialog keeps a compact row for each worker in the turn: role, objective, elapsed time and status. A row opens that worker directly in the full Workers view at every width. There is no intermediate side-pane mode or Full view switch.

The Workers tab keeps the worker list on the left and the full activity stream on the right. The stream reads like Conversation: the worker's messages in order with each run of tool steps folded into one collapsed block, and a finished worker's result, checks and report at the end of the stream, immediately visible. The Coordinator entry shows the coordinator's own recorded activity in the same form and refreshes while the task runs. Narrow windows use a worker picker instead of the side-by-side list.

Worker list:

- Groups: Coordinator (one row showing planning, idle, waiting for you or writing the result, with its route and context); Execution, Review and Verification in plan order; and Finished. Finished is collapsed while the task runs. A worker moves there only after its stage has ended, so rows do not jump mid-stage.
- Each row shows status, role, objective, elapsed time (frozen at a terminal state), the model and effort the runtime confirmed, the current activity, context occupancy and tokens, each shown only when the runtime has a value. An absent value is not shown, and it is never shown as zero. Up to two badges appear: unread activity, a waiting question, a pending message count, and an attempt count after a retry or route fallback.
- Sort by plan order (default), needs-you first, or recently active. Filter by role or objective text.
- States: planning with no plan yet ("Planning. Workers appear when the plan is accepted."); a direct answer with no workers; and activity that could not be loaded. The last case keeps the rows and shows "Activity could not be loaded · Retry". It is never shown as "no workers".

Live worker view:

- Header: role, objective, attempt number, status, elapsed time, route and route choice, context meter, tokens, a "Stop a command" control (only while gateway commands are running) and a menu with the report, a copy-id action and the instructions sent to the worker.
- Stream: tool steps fold by turn into a collapsed block whose line says what happened, such as "Edited 2 files, ran 3 commands, and read 5 files". Opening it lists the steps the same way in Conversation and the worker view: one row per recorded call identity, merging input, status and output updates, with an icon, a plain name and its target on one line (Read and a path, Edit and a path, Shell and a command, or the tool's own readable name). A step with more detail opens on click: reads link to the file in Code; searches name the query; edits and writes expose the file and recorded before/after text; commands show their full command line and output. There is no verbosity setting. Stored Markdown escapes are removed in prose, results and finished worker-list previews, while inline and fenced code retain their real backslashes. Worker notes span the content columns; internal message prefixes are not displayed. Tool labels, icons and Worked summaries reuse the Paseo integration’s presentation functions, including namespaced native/MCP calls. Internal bookkeeping, thoughts and result-submission tool rows stay hidden before grouping and counting; the published worker result remains visible. Native commands never invent an exit code or offer gateway Stop.
- Live activity: a quiet Working… indicator remains while the task or worker runs, with Reading…, Editing… or Running command… only when the recorded activity establishes it. It stops for terminal or waiting states and respects reduced motion. The stream arrives at tool boundaries; missing output and clipped previews are explicit.
- Follow: on when a running worker is opened. Scrolling up turns it off and shows a pill "↓ 7 new · Follow". End or the pill jumps to the latest item and turns Follow on. Both Conversation and Worker feeds keep the bottom pinned as streamed text or expanded Markdown grows. With Follow off, appended content keeps the scroll offset and visible content still; a row above the viewport changing height preserves the visible reading anchor. Returning to the bottom resumes following. Content updates do not override a held scrollbar drag.
- Search covers loaded pages, with a match count. "Search older history" loads earlier pages in bounded steps. Refreshing the same worker attempt preserves loaded pages and in-flight page reads; switching attempts discards them.
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

Worker view states: loading (skeleton rows and header, never "empty"); just dispatched ("Waiting for first activity"); partial history ("Showing last 200 events · Load earlier"); history read failed (retained content with "Possibly out of date · Retry"); offline machine ("Offline · last update 13:47", with the composer disabled and the reason shown); interrupted ("Worker interrupted · Cortex will retry or re-plan", and pending messages become "Not delivered"); narrow windows below 1280 px hide the inspector; below 900 px the full stream uses a worker picker, with horizontally scrollable tabs.

## Worker messages (desktop)

A composer at the bottom of a running worker's view reads "Message <role>…". It has an attach control, a send-mode chip, a voice control and Send. Files can be dropped into it, and a copied screenshot can be pasted into it ([CW-R39](requirements.md)). The worker's model, effort and access mode are shown read-only, because the route belongs to the runtime. The same composer opens from the worker row menu, and a Dialog worker row opens the full worker stream.

- Send mode "At next step" is the default. The message is appended to the worker's next Cortex tool result, or to the next prompt of the same session if the turn ends first. A worker that uses only native engine tools receives it at its next Cortex call, and the UI says so: "Delivered when the worker next calls a Cortex tool".
- "Interrupt now" is shown disabled, with the reason "Not available for this engine yet", until the engine is qualified for it.
- Message states, shown as words as well as icons: Sending…; Pending · next step (Withdraw is available until delivery); Delivered at a time (recorded in the worker's activity, which is not proof that the model acted on it); Withdrawn; Not delivered · worker finished (Send to Cortex instead, or Discard); Not delivered · stopped, cancelled or replaced (Send to new attempt if one is running, Send to Cortex, or Discard); Failed · not sent (text stays in the composer, with Retry).
- Several pending messages are delivered together, in order, at one delivery point. A pending badge appears on the worker row.
- The owner's text reaches the worker verbatim, framed as guidance that changes no scope, permission, review or acceptance criteria.

## Task send modes

- Enter sends in the current mode. Ctrl/Cmd+Enter queues. Shift+Enter inserts a newline. The mode chip on Send shows the current mode, and its default is set per machine. Key bindings are proposals until the owner decides them.
- Steer: captured at once, and the acknowledgement says it will be applied after the current step. It is applied after the current workers, review and verification, and the task is re-planned. Steer cannot be edited or removed after capture; a correction is a new message. Its states are Sending…, Received · applies after current step, Applied · plan revision, or Failed · not sent (text restored, with Retry). A refused capture means nothing reached Cortex, and running work continues.
- Queue: held by the client and not yet sent. Queued items sit above the composer. Each item shows its text and its attachments, and offers Remove, Send now (which sends it as Steer) and reorder (drag or Alt+Up/Down). Edit removes the item from the queue and returns its text and attachments to the normal composer, where it is sent again; the queue never edits an item in place. The queue is as wide as the chat column. Its background is transparent at its edges and in the gap between the queue and the composer, so the messages behind it stay visible, and its content is not clipped. Queue is sent as an ordinary message when Cortex is idle (waiting for you, or finished). While the queue is not durable, its label reads "Queue is kept until the app closes".
- Edit over a non-empty composer draft asks first: Replace draft, Keep both or Cancel. The composer never overwrites a draft silently.
- When Cortex is idle the two modes are identical, so the mode chip is hidden.
- While a coordinator question is open, the composer shows "Replying to: <question>", and text sent there is the answer. Its answer field supports line breaks and attachments, as in the main composer, including drag and drop and paste ([CW-R39](requirements.md)).
- The Send button turns into Stop only when the composer is empty. Stop opens the stop dialog, which offers Stop and Continue, Close as is, and Cancel task; see [CW-U08](backlog/CW-U08.md).

## Voice input

**Built local subset:** Ctrl+D or the mic toggles local Russian capture in task,
worker, New task and Other answer composers. A live, wrapping transcript preview
appears inside the composer with loading/download progress. Stop retains the
preview; Cancel discards it; Insert appends it to the editable draft. Insert and
send is a separate button where that composer supports sending. The default
PulseAudio source and sherpa-onnx small Russian model are used with no cloud
provider or audio storage. New-task dictation never saves or starts a task.
See [local voice behavior and limits](local-task-execution.md#local-voice-input).
The broader controls below (provider/language selection, level meter, insertion at
cursor, push-to-talk and Esc/Enter bindings) remain planned, not built.

- A microphone button sits at the bottom right of every composer, left of Send: task, worker, sprint planning, Request changes, diff comments and New task description.
- Start: a click toggles dictation. A press-and-hold push-to-talk mode is proposed, pending the owner's decision. Ctrl/Cmd+D toggles it in a focused composer, and it is not bound while the terminal has focus.
- While recording, a recording bar replaces the composer toolbar at the same height. It shows a red indicator with elapsed time, a level meter (static under reduced motion), a language choice (Auto, Русский, English or an installed model), Cancel (Esc), Insert (Enter), and Insert and send (button only).
- Live transcript: partial text appears in grey italics at the cursor in the draft. It becomes normal text on Insert. "Processing…" appears between stop and the final text.
- Dictation never sends by itself. Insert and send uses the composer's current send mode.
- Errors appear inline, and text input stays available: "Microphone access is blocked by the system." (with a link to system settings); "No microphone found."; "Dictation needs a speech model that is not downloaded." (with Download and Choose another); "Speech-to-text key is not set for this machine."; "This machine's voice service is offline."; "Microphone interrupted. Waiting to resume…"; "Transcription took too long." (with Retry and Discard).
- Provider: a local model is the recommended default, and an OpenAI-compatible service is the alternative. The owner selects the provider in the machine's voice settings ([CW-U07](backlog/CW-U07.md), [CW-U15](backlog/CW-U15.md)). The default and the machine that transcribes a remote task are open decisions.
- Privacy: audio is streamed only while recording and is not stored by default. A recording indicator is visible for as long as audio is captured. Dictation starts only from a user gesture.

## Keyboard and visual rules

- Esc never stops a task. In a recording, Esc cancels the dictation. This deliberately differs from Paseo and is an open decision.
- Proposed keys: Ctrl+1 to Ctrl+5 switch task tabs; Ctrl+Shift+P toggles the plan graph; Ctrl+Shift+F filters workers; End jumps to the latest activity; Ctrl+F searches the worker view; Ctrl+. opens the command-stop picker (never an immediate stop); 1 to 8 choose a question option. Each key is checked for conflicts on the target platform before implementation.
- No focus or press highlight. A click, a press, a route change or a dialog opening leaves no outline, ring, border change, fill or accent on any control, and a text field shows only its caret. Selection that means something (the current page, the open task) stays: it uses a neutral fill, a visible text or icon marker and `aria-selected`; a fill alone does not meet the contrast needed for selection. Selected labels never change font weight or size, and selection never shifts layout ([CW-R34](requirements.md)). There are no coloured side stripes. Only the running marker pulses, and it is static under reduced motion.
- Keyboard position: moving with Tab or an arrow key draws one quiet 1 px neutral line around the focused control (`--focus-line`), and a text field shows only its caret. This is the only focus mark, and the next pointer press clears it. A row that fills its scroller or list draws the line just inside itself, so nothing cuts it. The app records the last way of moving in `data-keyboard` on the root element, because the engine's `:focus-visible` also matches after a click that opens a dialog. Checkboxes and radios are drawn by the stylesheet, so no native focus ring can appear on them.
- Panel toggles: each icon that opens a panel (sidebar, worker pane, terminal, inspector) shows that panel's real state by colour alone: lighter while the panel is open (`--panel-on`), darker while it is closed (`--panel-off`), in both themes. Hover and click add no fill or outline, the icon never moves, and a closed panel always reopens from the same icon. On a screen with no such panel the toggle is not shown and its place stays empty, so the band never shifts. The worker pane toggle is shown only where it controls the pane: the Dialog tab at 1800 px and wider.
- Hover titles: no element uses a native title, so nothing pops up under the pointer. A control that cannot be used says why in visible text next to it. A control that nothing could ever use on this screen is not offered at all.
- Window edges: the frameless window keeps the outer 5 logical pixels of every edge (multiplied by the display scale) for edge resizing. Scrollers at the right and bottom edge stop short of that zone, so a scrollbar can always be grabbed and dragged and the window can still be resized from its edges.
- Shadows: none, or so faint that they are barely visible. No card, popover, queue or composer uses a visible shadow.
- Interface words: a label never names the program, because the whole program is Cortex: "Use", not "Use in Cortex". The agent you talk to reads as Coordinator where a name is needed. Labels and hints are English only.
- Task state: a same-branch review points to Ready to finish, not Ready to merge. The inspector shows the completed outcome once, without repeating the same status as a second fact.
- Identifiers: task, assignment, report, route and connection ids and build hashes never appear in the interface. Diagnostics is the one exception, and it shows the build and connection identifiers for support. A version elsewhere reads as `1.0.6`. Two connections offering one model are told apart by connection name.
- Counts: nothing to count shows nothing, never "All 0". What differs from a profile is listed in words, for example "Claude: 4 sessions at most, was 2".
- Titles and names are edited by clicking them. There is no separate Rename or Edit control.
- Selection: text can be selected and copied everywhere, with Ctrl+C. Buttons, links, the title band, the sidebar and draggable cards are excluded.
- One piece of text, such as a path, is cut with an ellipsis and never broken across lines. Anything cut shows whole in its tooltip.
- Absent values: a value Cortex does not have is not rendered. No dash (—) or placeholder glyph stands in for it, and an unknown value is never shown as zero or as success. When a whole panel has no data, the panel carries one state line with its reason, not a placeholder in each field.
- Diff colours are always paired with `+` and `−` text. Every icon has a text label.
- A polite live region announces real transitions only, such as "Implementer finished with 1 issue". Polling never announces.

## Theme (desktop)

- The theme is System, Light or Dark, chosen only in application settings. The default is System. The sidebar and window chrome have no theme switch ([CW-R33](requirements.md)).

## Worktree settings (desktop)

- Settings › Worktrees is a per-machine page. The real service reports both the current folder and its absolute default. The default is `$XDG_DATA_HOME/cortex-workspace/worktrees` in the service environment (fallback `~/.local/share/cortex-workspace/worktrees`); the dev service isolates XDG data under `<channel-state>/data`, so its default is `<channel-state>/data/cortex-workspace/worktrees`. The field, "Use default" tooltip/action and example all use that reported path; reset is unavailable until it is known. New worktrees use `<root>/<project-key>/<task-id>`, where project-key is the 16-character hash of the common Git directory. The folder is typed in the built page. Saving changes only future worktrees and never moves existing checkouts.
- A new path is checked before it is saved. It must be absolute, writable and outside every registered project checkout. A failed check is shown beside the field with its reason, and the saved path does not change. An offline, stale or unsupported save shows "Not saved", as in machine settings.
- Task creation offers two checkout choices: the current checkout with its open branch, or a dedicated worktree under the saved root. If the saved root is unavailable when a task starts, the task does not start in another folder; it shows the reason.
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
- A sprint's board, Planning and Summary say why the sprint is not moving, with an Open task link to the task that holds it: what Run sprint would refuse in an automatic sprint, or the dependency the first unstarted task waits for in a manual one.
- A sprint whose recorded tasks could not all be loaded says how many are unavailable and offers Retry, which only reads again. It is never shown as empty, and Archive stays off until every recorded task is loaded and done. An empty created sprint, and a sprint's own board, offer New task with that sprint already selected; Ctrl+N from a sprint's pages does the same.

## Ready to merge and conflicts (desktop)

- Every finished task, including tasks of an automatic sprint, reaches Ready to merge with its target branch. The target defaults to the repository's Git default branch and can be changed for a task or sprint. A local squash merge or a PR runs only after owner confirmation; there is no automatic merge ([CW-G07](backlog/CW-G07.md)). In an automatic sprint, a task that passes its checks moves to Ready to merge by itself, with no separate Accept step; the owner confirms there.
- Ready to merge always has one primary next action. A task that is not accepted yet offers "Review and merge…": its dialog shows the exact result version, checks and target, and its one confirmation records acceptance of that version first and then merges or creates the PR. A refused merge keeps the dialog open and the acceptance on record; nothing is resent by itself. An accepted task offers "Merge…" and "Create PR…".
- A project folder without Git has nothing to merge. Ready to merge offers "Close as is…" and "Set up Git…" instead of Merge and PR. Closing a result that is already accepted, or promoted after passed checks, keeps it on record as checked, without the Not verified warning. Setting up Git opens the project's Git page, where creating an empty repository asks first. New task offers no worktree or target branch for such a folder.
- Loading covers the whole action: a multi-step action such as staging every change and then committing, or accepting and then merging, keeps the pressed button loading and closed to a second click or shortcut until its last step ends. The label stays readable and a thin bar runs along the button's lower edge.
- Before Ready to merge, a check the system skipped runs automatically. A failed or not-run check offers a forced re-run. A result with an unpassed check can be accepted only when that check cannot complete for a technical reason, and only with an explicit confirmation ([CW-R40](requirements.md), [CW-U04](backlog/CW-U04.md)).
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
