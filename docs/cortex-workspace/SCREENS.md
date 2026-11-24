# Screen inventory

**Status: screen inventory. The desktop screens run on real data in the application and are checked by no-paid automated checks and container journeys, not on a real Ubuntu session. A statement marked Planned in a section is not built. The Remote access settings page and the Android phone shell are built in the shared interface and covered by interface tests only; they are not checked in the native desktop window or on a phone. The desktop's own Add remote machine control (desktop to desktop) stays disabled.**

Desktop: all-projects dashboard, machine Board, per-project dashboard (Overview), project board and list, create task, task discussion and plan, the task Workers view (worker list, live worker view, finished worker view and worker message composer; Work details once Conversation is adopted), decisions, evidence and diff, file tree, editor and search, terminal, Git, connection, orchestration, worktree and update settings. The accepted desktop direction (Gate D, 2026-11-05) is the Paseo-style variant based on concept A, recorded in [desktop approval](design/desktop-approval.md); the static mockups in `prototypes/cortex-workspace/mockups/` are its visual reference.

A machine is a running Workspace process, not a project. One local machine exists at launch; remote machines are the computers a phone is paired with ([remote access](remote-access.md)); the desktop's own Add remote machine control is disabled. The machine switcher shows one machine at a time, while Inbox and Archive aggregate all machines. Projects belong to machines and tasks belong to projects; sprints contain linked tasks. Tasks can be opened from the machine Board, their project's board, list or overview, or the cross-project needs-you list. Home summarises projects; Board shows work across them.

Android: Now, tasks and projects; decision detail, result/diff, discussion, connection/reconnect, file editing, Git and advanced terminal are accessible but secondary.

Each screen needs loading, empty, offline/stale, permission denied, conflict and error states where applicable. A result must distinguish unrun from passed checks; unknown status must not be styled as success. Decision detail identifies effect, scope and rejection consequences. Return from mobile preserves task identity and context.

## Conversation adoption

**Status: owner-selected on 2026-11-10. Built in the shared UI: the task conversation and worker messages below; in the page table, Home, the Inbox, the Board and list, New task and New sprint, Sprint planning, the knowledge and skill requests, and the shared row sizes; and the phone-width rules marked built under Responsive layout. The other page-table rows remain planned.** The visual anatomy, attention priorities and composer rules are in [DESIGN.md](DESIGN.md#conversation-model). This section replaces the earlier Dialog and Workers tabs, the plan strip and the Send button that turned into Stop.

### Saved task editing and sprint closure

Built: a never-started task has **Edit task…** in its task menu ("…" in the title band) and its board/list menu. The dialog uses the shared composer (Attach, Voice, model/effort/settings and the plane labelled Save changes), plus title and checkout. Saved attachments must load first; a failed read has Retry. A stale save retains the draft and explains the refusal. Starting, pausing or failing a task never makes its original request editable again.

A created sprint has **New task** and **Close sprint…** in its title band; a narrow title band moves both into the sprint's actions menu, so the queue control and the window's own controls stay in view. New task adds to the current sprint without starting work. Closing confirms that the sprint and its tasks move to Archive, including drafts, and that the queue stops without discarding anything. The Close control names started members that are not Done, members with disputed completion and open rework. Archive has a Closed sprints section with retained summaries; archived drafts are labelled Not started and can be restored without a sprint. Until it is closed, a sprint stays in the sidebar under its project; once its queue is complete it reads Done there, while its tasks may still wait to be finished. Running and paused queues also appear in Active work, with their Working, Review and Ready to merge tasks indented under the sprint. The project list also indents tasks under their sprint and shows at most five task and sprint rows combined, before the All link. See [the behavior and error contract](local-task-execution.md#editing-saved-tasks-and-closing-sprints).

### Task Conversation

- The task opens on its Conversation column. A settled question reads as the question with its answer
  beneath it. Recorded order is kept, and one turn's activity folds into one compact line. The
  Conversation shows only the coordinator's own messages, which are in the task's language, with the
  compact work rows, worker summaries, questions and results. A worker's or reviewer's narration (written
  in English by design) is not a paragraph there: it is read on that worker's page under Work details, as
  text rows in order with its tool steps (the newest 60 messages, each up to 4000 characters, under a
  shared size bound; a cut marks the page partial). A worker's tool steps are on its page too, not in the
  Conversation's "Worked …" lines. The runtime leaves a retained message or tool call out of the
  Conversation only when its identity names an attempt recorded for the task's run under a role other than
  the orchestrator, and the worker page reads the same rows by that attempt; an older history is read the
  same way, and a row with no recorded attempt stays in the Conversation. A question a worker asks needs
  the owner, so it stays in the Conversation, open or answered, headed "Asked by" and the worker's role,
  and is answered there.
- A context compaction the runtime observed in the coordinator's engine session reads as one divider
  line, "Context compacted", at its place in the Conversation. A coordinator message that carries the
  runtime's retained-state recovery capsule is the model repeating its own prompt, not a reply: it reads
  as one line saying it was hidden, and its retained text stays in the store. The same holds for a
  message that opens with an engine reminder block (`<system-reminder>`) or carries two of them; a
  reply that only mentions the tag or the capsule once inside its own prose stays visible.
- Work details holds the stage sequence, the plan revision with "What changed", the worker list, and the usage and context totals. Selecting a stage filters the worker list there. Below the workers, Write denials lists the owner's `deny-write` paths in force for the task, each with when it was set and Remove, which lifts that one denial only.
- Selecting an option only prepares the choice; Send answer submits it. The owner may choose an option,
  add their own words and attach files: writing does not clear the option and choosing does not clear the
  words, and all of it is the one answer (the option's label, a blank line, the words, with the files);
  the settled card shows that answer with its file chips, also after a restart. Only the runtime's stop
  question and a skill proposal, whose options act by themselves, keep an option and written words apart
  (writing clears the option, choosing clears the words). A question with options keeps one answer line
  above its box ("Answer: not chosen yet", then the chosen option, cut to one line), so choosing fills
  that line and the page does not move. The free-text answer uses the same composer in answer mode, inside
  the question card, without a model or effort control: the answer goes into the task's running session,
  whose model and effort are already set. The general composer folds to "Send a separate message"; when it
  is expanded it is labelled "To Coordinator — not an answer to the question". Both drafts are preserved.
- Batch questions: questions asked together are one form. Every question is on view under "Question N of M" with its own options and answer box, and one Send answers button sends every answer at once as the one answer of their batch; it stays unavailable, with "Answer every question first: N of M answered", until each question has an answer. Once the service records the answers the form reads as answered and never offers the same questions again; a refused send keeps every chosen and typed answer and shows the reason, so the owner can send again. The answers read as given from the press, with "Sending the answers…", while their fields stay behind them; a refusal brings the fields back as they were, with the reason and Retry.
- Long conversations open on their latest part. The Conversation opens at its end with the newest rows,
  read in at most four requests however long the task ran, and new rows keep arriving below as before.
  If the boundary below the task description is on screen or within one screen above it, earlier
  content loads automatically, even while following. This repeats as needed on opening, resizing or
  scrolling up. **Show earlier** remains a fallback. "Loading earlier…" stays until there is more
  conversation to show or the beginning is reached. The earlier content goes above the held rows
  without shifting the reader's position. A failed read shows "Earlier conversation could not be
  read:" with its reason and **Retry**; it does not keep retrying by itself. Once the first row is read the line is gone and the
  conversation reads exactly as a complete read shows it. Until then a message or a "Worked …" line that
  may have begun before the rows read is not shown. A settled question from before them is not shown at
  their top, and the follow pill does not count rows read above as new. A short conversation reads in one
  request and looks exactly as before.
- Sent messages read at once. A message, a steer, a request for changes, a message to the planner and a direct message to a worker leave the composer and read in their place the moment they are sent, marked "Sending…", then "Sent"; a queued message stands at the end of the queue as "Queuing…". The service's own record takes its place without a second copy or a move: a saved, queued or worker message by the request id it was saved under, a history line by its text among the lines that arrived after the send, each record answering one send in order. A send that did not arrive stays on its line, text and files, with "Not sent:" and the reason, Retry (in the same place) and Edit (back into the composer). A confirmed message whose record does not arrive within a minute stops being shown on its own.
- Suggestions while writing. The task's message field reads "Message the agent, tag @files, or use
  /commands and /skills". Typing `@` (at the start or after a space) opens a list of the project's files
  under the field, filtered as the owner types once typing rests for about 120 ms, at most 20, best match
  first (a file name that starts with the words, then one that holds them, then a path, then letters in
  order; case and script do not matter, so Cyrillic names match as typed). Before anything is typed after
  `@` it shows only files picked earlier in this window, or "Type to search project files". Typing `/`
  where a line begins opens Commands (the screen's own Stop or Resume, Queue and Send now, Work details,
  Changes, Code and New task) and Project skills. Up and Down move, Enter or Tab picks, a press picks,
  Escape closes the list and leaves the field and the draft as they are, and removing the `@` or `/`
  closes it. While the list has options Enter picks instead of sending; with only a note in it ("No
  matching files", "Searching project files…") Enter sends as usual. A picked file is written as `@path`,
  a skill as a sentence naming it and its path, and a command runs and leaves nothing in the field. The
  list sits in the top layer against the field and never moves the layout. The same `@` list is offered in
  every message field that knows its project: answers to questions (which offer no `/` list), messages to
  a worker, the planner, New task and New sprint, Edit task and Request changes; `/` skills are offered
  where the text goes to the coordinator (the task's field, New task, Edit task, Request changes) and
  commands only in the task's field. Fields with no project, and the stop reason, comment and
  conflict-guidance fields, keep a plain hint.
- Paused task: Resume task comes first. A draft may be kept, and sending says that it resumes the task.
- When a task is **Not confirmed** because its records disagree, the conversation offers **Open
  Diagnostics** and **Close as is…**. The dialog confirms just the close, not cancellation or verification
  of the run. The closed task has no active decision and retains a warning with a link to Diagnostics. The
  Board and List put it under **Closed, not verified**, not **Done**.
- Permission and access waits keep their own explicit Allow and Reject decision. They are not chat. A worker engine's request to go beyond the access mode (read or write outside the project, credential or key files, Git control files or engine settings, a deletion, a web fetch) is a Permission card (a command run in the sandbox reads outside the project without one) in the task's Conversation, "Asked by <role>": one line saying who wants to read, write, delete, run or fetch what, the exact paths (past three, the rest fold behind "N more"), why it needs the owner, the access mode, and Deny and Allow once; no text field and no model or effort choice. While it waits the task needs the owner and the desktop notification for a waiting question covers it. Answered, it reads as the request with "Allowed once" or "Denied" and the time; withdrawn (the task paused or stopped, the worker's session ended, the service restarted), as "Withdrawn:" and the reason.
- Result: one current result, inline, with check outcomes (Passed, Failed, Not run, Unverified) and their provenance. Accept vN accepts only that exact version, and the outcome reads "Accepted vN; not committed or merged". Commit, Merge, Create PR or Close is a separate confirmation with its own press. The same decision component serves Changes and Inspector; no second acceptance flow exists.
- Dock: the workers bar, queued rows and the composer at the bottom of a Conversation, the planning conversation and a worker's page stand in the pane below the feed, never over it: the feed and its scrollbar end at the dock's top edge, so nothing of the feed shows beside, between or under the dock's rows, and at the end the scrollbar's thumb reaches the feed's visible bottom. The feed's last entry ends above the dock with the usual gap. The Latest pill floats over the feed's bottom edge with no band or background of its own, short of the feed's scrollbar, and never changes the feed's size.
- Copy: every full message, the owner's and a received one alike, has its own Copy button: the task and sprint brief, the owner's messages and steers, the coordinator's and planner's replies, a worker's messages and the owner's messages to it, and the task and worker result cards. It copies the message's Markdown source exactly as recorded (headings, lists, fenced code, links and blank lines), never the rendered text or a clipped preview, and reads "Copied" only after the clipboard took it, or "Not copied" when it refused. The button appears when the pointer is over its message or when it takes keyboard focus, and stays visible while it reads "Copied" or "Not copied"; its caption line under received text stays reserved while the button is hidden, so it never overlaps the next entry and the message text does not move. Tool rows, status lines and settled question cards have no Copy.
- Stop is the task composer's action button while the task runs and the field is empty, and an entry of the task menu. The stop dialog keeps its options: Stop and Continue, Close as is, and Cancel task.
- Lost reply: the message stays on its line in the conversation with its text and files, marked not sent with the reason, and history is refreshed; Edit takes it back into the composer. Nothing is replayed automatically and no duplicate queued item is created.
- Queue: Edit restores the text and files only after the queue removal is acknowledged; a failed removal leaves the queue as it was. Send now keeps the original message identity.
- Enter sends in the visible delivery mode and never changes it. Shift+Enter inserts a newline. IME composition is never treated as a send.

### Worker messages

- The worker composer uses the [shared composer anatomy](DESIGN.md#unified-message-composer). Its states are those listed under Worker messages above, with "Coordinator" as the name where one is needed.

### Page adoption

| Area | Adoption |
| --- | --- |
| Startup, title band, sidebar, menus | One attention selector. Separate from it, Active work is a status list above Pinned and Projects, with no row limit. It shows this machine’s Working, Review and Ready to merge tasks across projects, grouped by status or under their in-progress sprint, never twice within the block, and a Finished group for tasks and sprints that finished since the owner last opened them. It is absent when there are no rows. One recovery message per incident. One next action in the title band; the state reads in the strip under the tabs, the sidebar and the Inspector, and the run is controlled from the composer. Repeated phase and resource strips are removed. |
| Inspector and detail panels | Closed by default in a task Conversation, behind one Details entry. Duplicate Accept and progress summaries are removed while the current item is visible. Details dock beside the conversation or replace it; they never overlay it. |
| Home | Built: one "Needs you" list in which each task waiting on the owner appears once, first its question, permission or conflict, then a result to review (with its check counts), then accepted work to finish; each line opens the task. The title band counts the same list. Projects follow. Engine limits are in the sidebar and the machine's Connections, no longer on Home. |
| Machine Board | Built: **Board**, immediately after Home in the sidebar and phone drawer, shows tasks across projects on the selected machine. It uses the project board’s live status columns and card facts, with the project name on each card. If a project on the selected machine is showing saved data, a Saved view notice distinguishes it from live state and clears when the matching live data arrives. Select a card to open its task. Done and unverified closures keep their existing columns; archived work remains in Archive. At narrow widths, nonempty columns stack in status order. |
| Inbox | Built: Home's Needs you list for every connected machine, in the same order: a question, permission or conflict, and each sprint planning question; then a result to review, with its check counts; then accepted work to finish. Each task appears once. A question, permission or result opens the task's conversation, where it is answered or accepted inline; a conflict opens its resolution; a planning question its sprint's planning. No separate answering interface. The sidebar's Inbox count is this list. |
| Board and list | Built: each card or row shows its title, its column as the state, one concise fact and, on a card, one next action. The fact is the question itself, the result's check counts, what was accepted and where it goes, or Working without a worker count; a failed or cancelled run keeps its outcome. The action is Answer, Review (a permission), Resolve, Start, Resume, Open (running work), Review (a result, which opens the conversation) or the finishing step. Stop is no longer a card action: it stays in the card's menu and on the task. Scopes, sprint grouping and filters are unchanged. |
| New task and New sprint | The shared composer, with the description first. Save draft is the default and starts nothing. Start now stays an explicit secondary action; a sprint uses Start planning. |
| Sprint planning | Built: a sprint has four tabs, Chat, Plan, Board and Summary, so the planning chat and the plan each have the whole width at every window size. The proposed plan is a line in the chat ("Plan vN proposed: N tasks") whose Open plan opens the Plan tab; on the Plan tab a chip says Question waiting, Working on the plan or Unsent message and opens the Chat tab, and Request changes opens the Chat tab with the cursor in its message box. Once the tasks exist the chat's composer stays in place, smaller, and says once "Tasks are created. Edit them on the Board." Create tasks stays explicit. Queue semantics are unchanged. See [read-only sprint planning](#read-only-sprint-planning). |
| Knowledge and skills | Built: Collect knowledge, Refresh knowledge and New skill are written in the shared composer, with Voice, Attach disabled because the request is text only, and the machine's default route shown read-only. The plane, named "Create the task", creates an ordinary task, which does not start. |
| Changes, ready to finish, merge and conflicts | An accepted result appears in the conversation; integration stays a separate confirmation. Conflict progress is a factual inline update that links to its detail. Same-branch results use Commit, non-Git projects use Close, and a pull request is not a merge. |
| Archive | Read-only outcome rows, with details on open. A task reaches it with one press of Archive (or automatically after the auto-archive days), with no dialog and no question; its work and checkout stay as they were and Restore brings it back. A row whose cleanup is blocked by unmerged work offers Decide…, where deleting that work at cleanup is a separate explicit choice. No composer. |
| Code, files, search, Git, worktrees, terminal | Reached from the secondary task or project entry. Project tabs opened from a sprint keep its context in the address, including Code, Git, Worktrees, Settings and Knowledge and skills. Reloading or sharing the link keeps that sprint selected. One Save action in the editor. Commit, Push, Pull and task acceptance remain different intents. The terminal is a secondary tool and never becomes a message composer. |
| Settings and orchestration | One next action per page, with detail on request. Settings stay forms that use the common row sizes. Expert pages (Files, Profiles) remain available. |
| Dialogs and menus | Each confirmation names the affected object and the consequence, and offers Cancel and one specific verb. A dialog's accessible name is its visible title (for example "Stop backend-dev?"); only a dialog without a title is named by a fixed label. The shared composer replaces the Request changes and conflict-guidance text fields only; commit, branch and key fields are not composers. |
| Palette, Help, project picker, not found | Search and Help are not chat. Obsolete task-tab commands are removed. Not found keeps one plain problem and Back to Home. Adding a project still does not activate it. |

#### Read-only sprint planning

A sprint planned by its own read-only planning task has five tabs instead: Conversation, Workers, Plan,
Board and Summary. Conversation is that task's conversation drawn exactly as a task's (the coordinator's
messages, the "Worked …" step folds, question and permission cards, the workers bar and the task composer
with its Stop and Resume), headed by the sprint's own description rather than the planner's instructions,
with the plan line and any planning failure note after it. Workers is the task's Work details. A message
asks for changes to the plan and continues the same planning task; an answer, Stop, Resume and the model
choice go through the sprint's planning. The plan is still approved on the Plan tab and with Create tasks.
The planning task itself has no row, card, count, page, search result or notification of its own: its
questions make the sprint read Needs you, and any address of it opens its sprint. An older sprint planned
without such a task keeps the four tabs and its planning chat.

If planning fails, the strip above the conversation names the step and what went wrong. **Retry planning**
asks planning to resume without a new message. **Change request** opens the existing composer and
focuses its draft; sending the changes asks the planner to continue with them. The draft, previous answers and proposed plan stay in place.
An owner stop instead shows the neutral cancellation message and **Resume planning**.
Repeated service problems show a non-blocking warning while planning continues. Inbox links to the
sprint with **Planning failed** or **Planning problem** on connected machines. An open planning question
takes the warning's Inbox place, so the sprint is never listed twice. The diagnostic row disappears
when the service clears the diagnostic, and the warning strip disappears on recovery.

### Responsive layout

| Window | Default composition | Detail |
| --- | --- | --- |
| 960 × 540 minimum | Sidebar collapsed by default fit. One task toolbar of about 44 px. Conversation at most 760 px, with gutters of at least 16 px. The composer is bounded so the reading area stays usable. No default Inspector or terminal. | Replaces the conversation, with Back to task. |
| 1280 × 800 | Sidebar at its current width, 260 px. Conversation at most 760 px, with outer gutters of at least 24 px. | A docked detail needs 640 px of conversation, 320 px of detail, a 24 px gap and 40 px of gutters (1024 px). If that does not fit, the sidebar hides first; if it still does not fit, the detail replaces the conversation. |
| 1920 × 1080 | Sidebar stays. The reading column stays at most 760 px and centred. | One 320–400 px detail may dock beside it. |
| About 820 px | Sidebar collapsed; one reading column. | Replaces the conversation, with a clear Back. |
| About 390 px | The same conversation, decision and result logic at full width. 16 px gutters, full-width question choices, and the desktop's compact controls (28 px, 32 px in a composer or dialog) with 8 px gaps: a narrow desktop window is not enlarged for touch. Larger touch controls apply only in the phone shell (see Phone shell below). No sideways page scroll. | Full-width replacement view, or a true modal for confirmations. Back and Escape return to the exact task or item. |

- Active work fits the 260 px sidebar and uses the same layout in the phone drawer. A long title and the project name on its own secondary line truncate independently; the short status stays beside them.
- Unseen changes: a sidebar row whose task or sprint reached a state the owner has not opened since reads as new, with a semibold title, a small accent mark after its status word and "new update" in its accessible name. That state is a question, permission, result, acceptance, conflict or queue hold waiting on the owner; a run that ended (Done, closed, failed or cancelled), whoever ended it and wherever, in another window or client too, and whichever column it sits in; a sprint's planning question, ready plan (its row reads Plan ready) or planning failure; or a finished sprint. Another question, permission or hold in place of the one opened is a new change even when its words repeat. Opening the task or sprint clears the mark; its next such change brings it back. Whatever is open on screen is seen as it changes. A finished task stays listed in its project and in a Finished group of Active work until it is opened, and while it is open; a finished sprint listed there holds its tasks under it, so none of them is listed again on its own. A sprint row also carries the mark for its tasks whose rows are not listed, and a project row for the changes in it that are not on view, all of them while it is folded. Only a change the app observed reads as new: on the first run and for a project the app has not taken in before, the present state counts as seen. Once a project has been taken in, a task or sprint that appears in it later reads as new when it waits on the owner or has finished, also one created and finished or waiting while the app was closed, and so does a row that changed while the app was closed. What was seen is kept in this app's local storage across reloads as row and project ids with fixed-size state codes, never task text, for at most 1000 rows (those changed longest ago go first) and 200 projects. A row that bound drops leaves a short fixed-size code of its id with its project (at most 10000 codes in all), so after a reload it still counts as seen rather than new; a project whose codes do not fit, or one the project bound drops, takes its unremembered rows as seen once more, so the bounds never flood the sidebar. A task or sprint gone from a project whose read confirmed what it holds, an empty one too, takes its record along; an unreadable project keeps them.
- Fit is measured from the pane width. A space-driven collapse never overwrites the remembered panel preference, and the panel the owner opened last wins.
- Long check commands and diffs scroll inside their own box; prose wraps. On a phone, tables become labelled rows (built for the settings and orchestration tables: below 560 px of table width each row stacks its cells, each under its column's name, and the header row goes; the tool-access matrix keeps its columns and scrolls in its own box).
- Labels: a control's, tab's, chip's, badge's or table head's label stays on one line and its capitals sit in the middle of its box within 1 px. A bar whose left group and actions do not fit on a phone puts the group on its own line. The title band's counts are shown whole or not at all, least needed first.
- On a phone the composer footer has two rows: Model, Effort and Settings, then Attach, Voice, delivery where offered, and the plane. Every control in both rows keeps the composer's 32 px, as on the desktop. The model label truncates with its full accessible label. A composer that cannot send says why once, in its writing area; its disabled tools point to that note instead of repeating it under the row. In the phone shell the footer is one row instead: Model, Effort, Settings, Attach and Voice scroll sideways under a fading edge, while Queue and the plane stay fixed at the right; controls there take the phone sizes.
- With the virtual keyboard open, the focused text and the plane stay visible. Pages avoid fixed 100vh and respect safe-area insets.
- Built for every page at about 390 px: no page scrolls sideways; Home's Needs you lines read in three lines (kind and age, the question, where) and its projects table keeps the project, Working, Needs you and Review; the Archive keeps the task, its cleanup and its menu, and its filters scroll in their own strip; a list row keeps its icon, title and state; a toolbar's middle zone scrolls instead of cutting its last tabs; the project toolbar keeps the scope, Board and List (or the open area when another is open) and a More project areas menu that holds the rest of Board, List, Code, Git, Worktrees, Knowledge and skills and Settings; a setting puts its control under its description; New task's two places to work stand one above the other; the sprint summary's counts stand two by two; settings and orchestration tables read as labelled rows.
- A phone layout in the shared UI is not native phone qualification.

### Phone shell

**Status: built in the shared interface (`packages/workspace-ui/src/phone.tsx`, `phone-shell.ts`, `phone.css`) and used by the Android app; covered by Vitest only, not seen on a device. The rendered layout gates for the phone markup have not run.**

The Android app mounts the same Workspace interface with a phone shell. It differs from the desktop in these ways only:

- The phone has no machine of its own. It pairs with computers and keeps one active; switching machines tears the workspace down and starts the next at its Home. A connection-lost band shows when the machine cannot be reached.
- No tab strip and no window buttons. A title band holds the menu or Back, the title and one primary action. The sidebar is a drawer over the page with a scrim that closes it.
- Larger touch controls, applied only under `html[data-shell="phone"]`, which only a phone shell sets: for example buttons 40 px, icon buttons, navigation rows, menu items and inputs 44 px. The desktop's sizes, including a desktop window narrowed to phone width, are unchanged.
- Android Back closes the top-most sheet, menu, dialog or drawer, then the pairing step, then the phone's own page, then goes back in the workspace; when nothing is left the app may close.
- Code is read-only, there is no terminal, no file editing or system folder chooser, and no keyboard shortcut hints.
- Phone-only screens: connect (start), scan the QR code with the camera, paste a pairing link, a check-and-wait step that shows the fingerprint until the computer allows or denies, the machine sheet (rename, forget), and phone settings (appearance and About). The interface has App lock and relay pages that appear only when the native shell offers them; the Android app offers neither, because app lock (to be optional and off by default) and a phone-side relay override are not built.

The desktop's Settings, Remote access page holds the switch, the pairing QR code with a copy-link control and its time left, the pending request with Allow and Deny, the paired devices with Revoke, and the relay setting. It also holds the "Passwords and tokens from phones" switch ([sudo password prompt](#sudo-password-prompt), [owner action card](#owner-action-card)). There is no short pairing code.

### Accessibility

- Landmarks, a skip link to the conversation, labelled fields and named icon controls. Logical focus order matches the reading order.
- Choosing an option and submitting it are separate actions. Each question of a batch is a labelled group with its own radio group of options and its own answer box, and the form has one Send answers button. Number keys choose an option only when a single question is open.
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

- Startup shows only the Cortex logo on the app's theme background, with nothing else, until the runtime
  projection and product facts (sprints, planning/archived tasks and owner decisions) can be presented
  together, and the machine's own settings (the models and efforts the composers offer), can be presented
  together. The settings are requested as soon as the service answers, beside the task reads, and the logo
  stays until both are in, so the interface never opens with its pickers empty. These startup settings are
  `config.read` (models and options, engine connections, configuration, engine usage and the model-list
  fetch state) and each activated project's `project.config.read` overrides; startup never reads the
  machine overview. The overview (machine name, version, disk use and storage breakdown, archive retention
  and auto-archive days, worktree folder and its default, GitHub CLI availability, space reclamation
  failures, last cleanup and last diagnostics) is read only when a screen that shows it is on view:
  Settings (Machines, the machine overview, Worktrees, Storage and archive, Diagnostics), the Archive, an
  archived task's page and the unmerged-work question, the inspector's Machine panel while the inspector
  is shown, a project's settings, and the Create PR controls and Finish dialog of a task whose project has
  a remote. It is read once, however many of those screens ask, and again when such a screen opens and the
  last read is older than a minute; while such a screen is on view it is read again with every machine
  refresh after an action and at most every 5 minutes with the machine poll, and with none on view it is
  not read again. Until it arrives those places say "Reading this machine's details…" (on a failed read,
  "This machine's details could not be read yet. Retrying automatically.", retried every 3 seconds)
  instead of a stand-in value; a Finish dialog with the pull request chosen keeps its confirm off with
  "Checking for GitHub CLI (gh)…" rather than calling GitHub CLI missing. Other screens show the default
  machine name, This machine, until a screen has read the overview. Waiting for the settings is bounded at
  15 seconds: after that the interface opens, the pickers say "Loading…" rather than "Unavailable", a
  strip says the settings could not be read, and the read is retried every 3 seconds until it succeeds. A
  picker says "Unavailable" only once the settings have been read and the machine really offers nothing.
  When a connection has no saved model list, the service fetches it on its own (see Models below) and the
  same read reports that it is doing so: the logo then stays, with "Loading models…" and a note that the
  first time can take a minute, until a model is offered or the fetch has ended without one, bounded at 90
  seconds (the 15-second bound above does not end this wait). A fetch that ended without a list opens the
  interface at once with a strip saying the engines could not report their models and pointing to Refresh
  models; a fetch still running at 90 seconds opens it with a strip saying the lists are still being
  fetched and will fill in by themselves. While the fetch runs the app reads the settings again every 1.5
  seconds. A saved list is never waited for: it is served at once, however old, and a list older than a
  day is fetched again behind it without any notice. After 2 seconds the slot under the logo says "Loading
  workspace…"; after 15 seconds it says "Still waiting for the service. The first start can take longer."
  A first read that fails shows "The workspace could not be loaded.", its reason and Try again under the
  logo, and the logo does not move. A service that cannot be reached is not a startup failure: an offline
  relaunch opens the offline workspace, with its Offline notice and Retry. Once the workspace has opened,
  its frame stays, so a retry never drops back to the logo. Background rereads keep the last coherent view
  until that pair is ready. A required read failure keeps that view and offers Retry; it never exposes the
  raw runtime task placement.

- Project registration and explicit activation. Registering a project does not activate Cortex or start work. Adding a project is described under [Add project](#add-project).
- A project whose Cortex is not working names the reason on its own page, with Try again. The machine stays Connected and every other project keeps working. Offline means only that the service itself cannot be reached.
- A description-only draft with per-task model/effort/access/language and main/worktree choices. Ctrl+Enter saves it; saving starts nothing. Automatic title generation is explicitly unavailable; rename is connected.
- Task-owned Changes, guarded merge/PR and conflict controls, real archived-row purge, project overrides,
  service-owned automatic queues and project removal. The project menu, worktree menu and Code open VS
  Code or the default application when one exists, and say what to install when not. Code edits checkout
  and worktree files with tabs, a dirty marker, Save, Autosave and conflict choices; see
  [editing files](local-task-execution.md#editing-files). It opens Markdown, CSV, TSV and SVG formatted
  with a Preview and Source switch, previews HTML and JSON on request, shows pictures and plays video and
  sound inline, and shows a placeholder with Show as text for PDF and other binary files; see
  [viewing files](local-task-execution.md#viewing-files). Its file tree shows every file and folder in the
  project folder, dot-files such as `.env` and files Git ignores included and dimmed, with Refresh files
  and Collapse all; a folder Git ignores starts closed, opens with one read of that folder (at most 500
  entries and a "N more not shown" row) and never reads on a timer; `.git` and Cortex's private stores are
  never shown; see [the file tree](local-task-execution.md#the-file-tree). See
  [task execution](local-task-execution.md) for safety refusals and incomplete surfaces.
- Explicit start and observed progress. Live data carries no freshness stamp; "Last confirmed" with its age appears only while the service is away.
- Unconfirmed task state. When the machine is connected but Cortex cannot confirm whether a task is
  running or stopped, the task's title shows one status, Not confirmed, and one alert notice explains it:
  "Task state not confirmed." with the reason that the machine is connected but cannot confirm whether the
  task is running or stopped, and the action Re-check state. A reply that is still unconfirmed says so and
  links to Machine settings → Diagnostics; a failed read keeps the notice and asks to try again. Re-check
  rereads the existing task state only: it resumes, stops and replays nothing. A confirmed read removes
  the notice and restores the composer and task controls. While the state is unconfirmed, sending and
  worker messages stay off and the disabled composer names Re-check state. A disconnected machine is a
  different case, with its own notice, "Machine disconnected.", the same Re-check state action and the
  Diagnostics link. Board cards and list rows show Process state unknown on a connected machine and Last
  known status on a disconnected one. When the runtime says why the state is unconfirmed, the conversation
  also pins a card above the message box: a session still being stopped reads as one quiet line that
  continues by itself (when a replan waits for a worker the coordinator cancelled, it names that worker:
  "Waiting for qa-engineer to stop. The coordinator cancelled it to change the plan.", the title reads
  Changing the plan, the sidebar Changing plan, and the worker's row Stopping… until its session is
  confirmed gone; such a task never reads Working); a process that did not confirm its exit gets a decision card with Re-check state,
  Open workers, Open Diagnostics and Close as is…; records that disagree get a card with Re-check state,
  Open Diagnostics and the explicit Close as is… confirmation. Ordinary task changes are refused while the
  records disagree. The task header's See what is needed brings the card into view. It is offered only
  while its target is on the page: the pinned card, or the open question or permission that replaces the
  card, and not while Answer already leads there.
- A review that cannot be completed. When the reviewer of an Automatic task reports the same item
  unreachable with its tools in two dead-end reviews, the conversation pins "This review cannot be
  completed": "The reviewer cannot read <item> with its tools, so this review cannot be completed. Close as
  is, or change the file so it can be read.", and Close as is…. At that point the runtime pauses the task for the owner (`owner_blocker`) instead of
  spending further automatic attempts; its decision card names the item and offers Allow more attempts
  (after changing the file) and Close as is…. A model that produced unusable output twice in a row pauses
  the same way. The header's See what is needed brings it into view. When the
  attempts run out, the decision card says what they went to.
- Identified single and batch questions. Several open questions, of a task or of sprint planning, are one form: the owner sees every question, answers each one, changes any answer before sending, and sends them all with one button. Multiline answers are kept unchanged; a batch submits atomically. Paused task-level questions remain visible and require Resume before answering. A settled question reads as the question with its answer under it, and the Inbox says how many questions a planning run has open.
- Durable owner/coordinator conversation, plans and tool activity, in recorded order. Streamed text reads as the message it was, and a different message that follows it directly starts a new paragraph instead of running into it, and a worker's or reviewer's narration is left to that worker's page rather than shown in the Conversation; one turn's tool calls fold into one line with its steps, and machine identifiers and display updates are not shown. First result is `Result v1`, independent of its runtime entity version; each Request changes that produces a new result adds one (v2, v3), and the banner, Accept label and inspector use the same number. A paused task reads Paused in the sidebar, and cancelled or failed workers are counted apart from finished ones, in the header and on the team bar alike ("Workers: 0 finished, 1 cancelled").
- Worker order. Every list of a task's workers (the team bar's menu, the Work details groups, the Inspector) shows the newest worker first, and the team bar names the newest worker at work.
- The team bar. Once a task has workers, one bar stands directly above the composer, pinned outside the scrolling conversation, and stays put while the conversation scrolls. It names the worker at work now (role, state and current step; else the next active one, else the last), counts the other workers at work, and opens that worker's activity on a click. All workers opens every worker's row, a running command's Stop and a Work details link as a menu over the page; opening or closing it moves neither the conversation nor the composer. At narrow widths the bar keeps the worker's role and state and drops the step and counts. A task with no workers shows no bar.
- Background reads update screens in place. A read that changes a task, its checks, diffs, workers, archived rows or connections updates the rows already shown; it does not rebuild them. What the owner opened (folds, check output, Evidence, connection details), picked (diff layout, a model's policy), checked (archive rows) or is typing (messages, a new diff comment) stays as it was. The rule holds on every screen: a background update never resets a scroll position, a highlighted or selected row, focus, an open menu, popover, fold or folder, or typed text, including the composer's `/` and `@` suggestion lists, the Search and actions list and the Changes, Code, Board, Inbox, sprint planning and Settings screens.
- A check that did not pass is listed in the Inspector by name, with its recorded reason or what that outcome means.
- A sprint plan lists task titles, one line each. Read more opens a right sidebar with the title, dependencies and full description, which scrolls on its own and is always docked beside the plan, never over it. Panels give way by one rule ([sprint plan](#sprint-plan-list-graph-and-full-view-desktop)), and on a window with no room for both, Details takes the plan's place with a way back. Escape closes it.
- All work names sprints on the board and the list: each column and status group shows standalone tasks, then each sprint under its name, and a card repeats the sprint's name in its footer. The heading leads to that sprint.
- A task waiting for the owner, in Needs you or Review, is tinted on the board and the list and marked in the sidebar. Same-branch and non-Git results use Finish in the sidebar; the inspector describes committing or closing, not merging. Review the diff opens the result-bound Accept control before the separate commit or merge confirmation.
- Status and reason text is plain language everywhere (sidebar, board, task header, queue strip, notifications, check rows): a recorded reason code reads as a sentence, a code the app does not recognise reads as a generic sentence, and no internal code or underscore is ever shown. A sidebar row always keeps the task title and shows its short status in a separate word beside it; the longer reason is read in the task itself. Ready to merge is a group heading; its rows still read Finish or Merge. The current sprint is highlighted in both places it appears, instead of its project. The project is highlighted in All work and Standalone scopes. On a task page, the visible links for that task are highlighted instead of the sprint. One table (`packages/workspace-ui/src/status-text.ts`) holds the wording. A code with a machine detail (`coordinator_prompt_failed:RuntimeError`, compared without regard to case) reads as its code's sentence and never shows the detail. Inside the task, the generic sentence never adds "Open the task for details".
- A card that changes status crosses to its new column instead of vanishing, and a folded column opens around it. Reduced motion moves it at once. A manual drag keeps the card, queued message or workspace tab at its on-screen size. In the Linux desktop app the preview follows the pointer inside the window; elsewhere supported browsers use native drag feedback. Start stays on the pressed card through background reads and shows a refusal there, with a retry path; sprint queue mode does not silently disable an otherwise startable draft.
- Every action shows its loader on the control that was pressed, after a short delay so a quick action shows nothing, and that control takes no second press until the action ends. Enter in a composer counts as pressing its send button, and an action from a menu shows on the control that opened the menu.
- A machine-persisted queue, sent by the service at the coordinator's next step. Edit removes an unsent item and restores the composer; Send now captures steering even during a running turn. Captured text is not replayed automatically after a lost response. Each queued row is one line: its text shortens and opens whole on a press or Enter, and Edit, Send now and remove keep their own box. The row shows no attachment chips; the message keeps its files, which show again in Edit and open from the conversation once sent.
- Live and finished workers from retained runtime rows, including prose retained at tool boundaries, result summaries, the whole public report when it is opened (retrieved in bounded pages, never shown in part) and all available combined task/command/guidance history automatically loaded by attempt using byte-bounded continuation pages, including older guidance attachments and delivery status. There is no Load older control; a failed read retains history and offers Retry. Following and the reader's scroll anchor stay unchanged while pages arrive. Checkout locations say Project folder or Worktree separately from the branch; the project title chip shows the current open branch, not the Git default.
- The Git page, worktrees, Code, a task's Changes and Code tabs and the merge dialog read Git when they open, after the owner's own Git action, when a task's state moves while they are on view, on return to the window after 10 seconds, and every 30 seconds while on view in the focused window. The Git page's status bar has a **Refresh Git status** control beside Fetch, Pull and Push that reads it at once. Nothing reads Git for a screen that is not on view, for another project or while the window is hidden.
- Direct worker messages recorded for that channel and delivered with its next successful Cortex tool response. Delivered does not mean read or acted on. Immediate interruption and withdrawal remain unavailable.
- Stop worker through runtime cancellation, from the worker composer's action button (see Worker messages below); Stop command for an exact running gateway command with a required reason. Native actions have no command-stop receipt. Task Stop is a durable resumable pause, separate from those cancellation controls; task-level and pending worker questions/access waits survive it and service restart through a recorded new-worker continuation. Individually cancelling a worker still ends its waits.
- Persisted review comments returned to Cortex, and result-bound requests to rerun a check. These request work; they never mark a check passed themselves.

Every form for a message or new work, including sprint creation, planning messages, Request changes, diff comments and conflict guidance, supports picked/dropped files and clipboard images with removable chips and image thumbnails; saved files open from their task, sprint or message. Other areas are enabled only where their service capability is present, and otherwise disabled or guarded in real mode; the labelled demo prototype keeps its mock screens. The theme is chosen only in application settings. Reopening the window restores history and queue from the machine. Restarting the service preserves a recorded task pause and allows explicit Resume through the runtime's recovery path. It does not revive lost engine processes: pending worker decisions continue through an explicitly recorded replacement, with access revalidated. Cancelled worker-local waits do not revive; unavailable controls refuse rather than replaying work.

## Core session controls (desktop)

These controls keep the working principles of the current Cortex session configuration (see [requirements](requirements.md) CW-R26). Each control sends an intent; the value shown as current is the one the runtime confirms. Each control needs unavailable, stale and permission-denied states, and an unsupported value is never silently replaced.

Task composer (task discussion; the create-task form reuses the same controls for optional settings):

- Model: provider and model, from the runtime's current route catalog. A model the route does not support is shown as unavailable with its reason.
- Effort: the effort list for the selected model. Changing the model resets effort, and the reset is shown.
- Access mode: Development, Guarded or Auto, each with its own description of what engine file tools ask before and what commands may do: in every mode a command can read anything the owner's account can read, outside the project included, without asking (in Guarded it cannot write there; a writing worker's commands in Development and Auto can). An unknown stored value is shown as Guarded, never as Auto.
- Language: Auto, English or Russian. It applies to Cortex's own status, question and stop text, and the control says so.
- Send mode: Steer or Queue, described under Task send modes below.
- Voice input: a microphone control that places dictated text in the composer for review, described under Voice input below. Local Russian dictation is built in the Linux desktop slice; Paseo's composer dictation is its reference ([CW-R31](requirements.md), [CW-U11](backlog/CW-U11.md)). Dictated text is never sent automatically.

Task header and inspector:

- Read-back line: the effective provider, model, effort, access mode and language as confirmed by the runtime, with the configuration revision they come from.
- Context: the last reported context occupancy and its age. With no snapshot the context value is not shown; it is never shown as zero or estimated from cumulative tokens.
- Usage: token counts, each labelled observed, engine-reported, estimated or unknown. Plan usage is labelled with its basis (at route selection, or a provider window), and a missing value stays explicit.
- Engine limits: for each connected Codex and Claude engine, the percentage of each weekly limit used, the
  window and the reset time, with the time of the snapshot ("As of 14:05"). Codex has one weekly window.
  Claude has two, shown separately in the sidebar limits bar ("Claude" for all models, "Fable"), the
  inspector's engine limits and the Connections limits table ("This week, all models", "This week,
  Fable"); any other model window Claude reports follows them. An unknown value is never shown as a number
  or as zero: a window the provider did not report says "Not reported" (nothing is estimated), with no
  placeholder in each field. A snapshot older than 15 minutes is marked stale ("Stale, as of …"). Values
  refresh in the background while the app is open, without a restart; while the window is visible a shown
  value is at most about six minutes old, because the app reads the machine shortly after each five-minute
  snapshot is due ([token and plan usage](../features/token-and-plan-usage/index.md)). The sidebar limits
  bar gives each window its own line (name, bar, percentage in shared columns), so three or more windows
  fit the narrowest sidebar without overlapping or cutting a value; a name too long for its column ends in
  an ellipsis, and the bar's accessible name and its popover say every window in full. OpenCode is not
  shown, because it spans multiple providers ([CW-R36](requirements.md), [CW-U10](backlog/CW-U10.md)).

Machine settings:

- Default model, effort, access mode and language for new tasks on the selected machine, saved through the machine's orchestration settings ([CW-U07](backlog/CW-U07.md)). Offline, stale and unsupported saves are shown as not saved.
- Engines and connections: connection status, and the engine limit usage described under Task header and inspector for connected Codex and Claude engines.
- Orchestration: routing, model, limit, tool and profile settings for this machine, specified under [Orchestration settings (desktop)](#orchestration-settings-desktop).

The small sprint dependency graph under the plan's list uses its intrinsic layout size, shrinking only when the panel is narrower, and only while its labels keep at least 11 px on screen (a scale of 0.92 or more, at most 320 px tall). A one-task plan never scales its node to fill the panel. A plan too large for that shows one row instead, such as `Dependency graph · 57 tasks in 9 layers`, with Open graph.

## Sprint plan: List, Graph and Full view (desktop)

The plan has its own tab beside the planning chat (Chat, Plan, Board, Summary). The plan header holds a chip for what waits in the chat, when anything does, the plan's name and count, a `List | Graph` switch and the plan's own actions. List is the default. The owner's choice is remembered and never changes by itself with the plan's size; the switch is hidden while there is no plan. Edit opens the editor in List, and the Graph then previews the draft.

The Graph view stacks a toolbar, the canvas and a status band under the plan header; the minimap is the only thing drawn on the canvas. Full view (the Full view button or E; Exit full view or Esc to leave) fills the window under the title band and tab strip. It hides the sidebar, the conversation, the sprint header and tabs, info strips and the plan header, and keeps warning strips, the stopped-planning strip with its reason and Retry planning or Resume planning, and Details. It writes no preference, so leaving it restores every panel as it was. It is not kept across a restart and is not the system's fullscreen.

Panels never float over the plan. The plan keeps at least 640 px in Graph and 420 px in List; with less,
the sidebar gives way, and on a window under 940 px Details takes the plan's place with Back to graph.
Details is always docked at 300 px. Giving way writes no preference, and space returning brings the panel
back. The panel opened last wins: showing the sidebar (Ctrl+B) where there is no room closes Details and
keeps the selection, and opening Details lets the sidebar give way. Ctrl+\\ moves between the Chat and
Plan tabs. The chat stays mounted while the Plan tab is shown, so an unsent message survives, and the plan
header's chip says Question waiting (or 3 questions waiting), Working on the plan or Unsent message and
opens the Chat tab. Below 800 px of window height the Graph view keeps only warning strips and the
stopped-planning strip. Request changes opens the Chat tab, leaving Full view, and puts the cursor in its
message box, keeping the selection and the view, at every window size. Menus opened from the graph
toolbar, Layers from the More menu included, stay whole inside the window.

Navigation. Zoom runs from the lower of Fit and 50% up to 150%, in steps of 1.25, and the percent opens Fit (F), Fit matches (Shift+F), 100% (0), 75% and 50%. Fit shows the whole graph with 24 px margins and never above 100%. A plan opens at Fit; zoom, position and collapsed layers stay for the tab, per sprint and plan version. Wheel pans, Shift+wheel pans sideways, Ctrl+wheel zooms at the pointer, and a drag of 4 px or more pans. Nothing in the canvas animates. The minimap, 176 by 112 px in the bottom right corner of the canvas, shows only while part of the graph is off screen. It is off by default on a canvas under 760 px wide, where the first press of M or its toggle turns it on; a click or drag on it moves the view, and arrow keys pan while the focus stays on it. Under 760 px of toolbar Layers and the minimap move into a More menu, under 640 px Status too.

Finding. From 8 tasks the toolbar has search, which matches the task number (`17` or `#17` puts task 17 first) and the shown title, case-insensitive, never the description. The count beside it reads `2 of 7`, `4 of 57` for a status filter alone, or `No matches`. Enter and Shift+Enter step through matches, a step into a collapsed layer opens it, and Esc clears the search, then returns to the canvas. The Status filter (`Status · 2` while active) appears once the plan's tasks exist and never for a past version or a draft; its checkbox rows count the tasks in each state and the popover stays open. Search and filter combine; tasks that do not match dim in place and nothing moves. Dimmed text uses the quieter text colour, never transparency, and keeps 4.5:1 against the canvas in both themes. When nothing matches, the band says `No task matches "zzz".` with Clear search and Clear status.

Selection. A click or Space on a task lights only its links: Direct by default, or Whole chain (T). What it needs is drawn solid, what it unblocks dashed, and the rest dims. Only a task the owner picked dims anything. The band then reads `Task 17 · Needs 3, 5 · Unblocks 20, 21` with the title, Start when the task can start, Open task and Details. A double click or Enter opens Details; a click on the empty canvas clears the selection.

Layers. Tasks group by dependency layer. From 13 tasks in 2 layers a heading such as `Layer 3 · 12` sits above each column and a click on it collapses the layer; the Layers popover lists every layer, with Collapse all, Expand all and, once tasks exist, Collapse finished layers. A collapsed layer is one band with its task count, up to three states worst first, its matches and its linked tasks; edges into it merge, counted `x3`.

Compact display. Titles stay 12 px on screen at every zoom up to 100%. At Detail (80% and up) a task shows its number, title and state; at Title (50 to 80%) its number and title; at Number (30 to 50%) its number, with its state icon from 42%; at Overview only a box marked by its state. At Number and Overview the picked task, the current match, the task with keyboard focus and the matches carry a pin with number and title, at most 12. A task that is not loaded is a dashed box that reads Not loaded; no plan state is drawn green. Past versions read `Past version · read only` and drafts `Draft · not saved`, both without task states; a draft's band says `Change titles and dependencies in List.` with Open list.

Keys, with the canvas focused and never while typing: the arrows move to the nearest dependent or dependency and up or down a layer, Shift+Left and Shift+Right cycle the same task's dependencies or dependents, Home and End go to the first and last task, Space selects, Enter opens Details or expands a collapsed layer, C collapses or expands the task's layer, + or =, - and 0 zoom, F and Shift+F fit, Ctrl+arrows pan, / or Ctrl+F focus search, M toggles the minimap, T the links and E Full view. Esc steps back one thing at a time: search text, then Details, then the selection, then Full view. The canvas is one Tab stop with roving focus, drawn with the quiet focus line under keyboard use. Each task is a button named with its number, title, state, needs and unblocks, and one polite status region announces counts and moves. Help and Settings → Shortcuts list these keys, and the palette offers Show graph, Fit graph and Full view.

## Task workers (desktop)

The task view opens on the conversation ([Conversation adoption](#conversation-adoption)). There is no plan strip or tab bar: a task bar states progress in words and keeps Work details, Changes, Code and Terminal one activation away, with their routes, and folds them into one Details menu on a narrow pane. The stage sequence (Planning, Execution, Review, Verification, Ready to merge) keeps its marker, which is a text or icon and never bold text, so no layout shifts ([CW-R34](requirements.md)); the plan revision with its "What changed" link and the context and task token totals move there too. Selecting a stage filters the worker list in Work details to that stage.

The Conversation shows only the coordinator's own rows. The task's workers stand in one bar pinned above the composer, outside the scrolling feed, and its menu lists each worker with role, objective, elapsed time and status. A row opens that worker directly in the full Workers view at every width. There is no intermediate side-pane mode or Full view switch.

Task and worker timelines show what was done to the project: reading, searching and changing files, commands, Git, browser actions and reading other workers' reports. The runtime's own plumbing is not shown or counted in the folded lines: polling a command or paging its output, discovering tools, loading skills and task context, drafting and submitting reports, plans, results and requests (each already appears as its own card), and code-index upkeep. This is presentation only; the runtime keeps the full record. Settings, Appearance has a "Show every tool call" switch (off by default, remembered on this device) that adds those steps to the folded lines, their counts and the worker pages at once, without a reload; the engine's own status updates (thinking, usage, mode changes) stay hidden either way.

The Workers view keeps the worker list on the left and the full activity stream on the right. The stream reads like Conversation: the worker's messages in order with each run of tool steps folded into one collapsed block, and a finished worker's result, checks and report at the end of the stream, immediately visible. The Coordinator entry shows the coordinator's own recorded activity in the same form and refreshes while the task runs. Narrow windows use a worker picker instead of the side-by-side list.

Worker list:

- Groups: Coordinator (one row showing planning, idle, waiting for you or writing the result, with its route and context); Execution, Review and Verification in plan order; and Finished. Finished is collapsed while the task runs. A worker moves there only after its stage has ended, so rows do not jump mid-stage. Finished groups attempts by plan step: each step shows its newest attempt, and "Show N earlier attempts of this step" opens the retries under it (an earlier attempt that is selected stays open).
- Each row shows status, role, objective, elapsed time (frozen at a terminal state), the model and effort the runtime confirmed, the current activity, context occupancy and tokens, each shown only when the runtime has a value. An absent value is not shown, and it is never shown as zero. Up to two badges appear: unread activity, a waiting question, a pending message count, and an attempt count after a retry or route fallback.
- Sort by plan order (default), needs-you first, or recently active. Filter by role or objective text.
- A worker card opens its live view directly.
- Worker result: a worker that submitted a failed outcome shows "Reported failure" with its summary, its checks and an Open report button, the same as a finished worker. A worker with no result says why in one line when the runtime recorded it: you stopped it, the coordinator cancelled it after a plan change, or it was lost when the service restarted or its engine session ended; a refused result keeps its refusal reason.
- Worker states beyond Working, Finished, Failed and Cancelled: a cancelled worker whose session is not confirmed gone reads Stopping…; a failed worker that named its dead end reads Found problems (work another role has to do) or Could not run here (a tool it needs is unavailable); an attempt a newer plan replaced reads "Replaced in plan vN" instead of a current failure.
- Refused results: the card says why in words for every refusal code, with up to three items the latest refusal named ("it had not read README.md in full", "the files changed while it reviewed", "a command it cited had not finished", "it had not read the earlier workers' reports", "it reported a failed check without repairing it"), and the earlier reasons after it. A worker that was accepted after being sent back reads neutrally, not as a problem: "Sent back 7 times before it was accepted (most recently: it had not read README.md in full). Nothing is needed from you."
- States: planning with no plan yet ("Planning. Workers appear when the plan is accepted."); a direct answer with no workers; and activity that could not be loaded. The last case keeps the rows and shows "Activity could not be loaded · Retry". It is never shown as "no workers".

Live worker view:

- Header: role, objective (a long objective shows its first four lines with Show the whole goal; opened, it scrolls inside a box of at most 40% of the window, so the activity keeps its room), attempt number, status, elapsed time, route and route choice, context meter, tokens, a "Stop a command" control (only while gateway commands are running) and a menu with the report, a copy-id action and the instructions sent to the worker.
- A worker's model reads "Engine · Model name", the name being the one its engine reported when the session was configured (recorded with the session), then the machine's model list, never a bare alias id; one that predates the record and is not in the list shows its id, and the retired `default` alias reads "Default (engine alias)".
- Stream: a worker's page lists all of its recorded tool steps, not only its edits and commands: its
  reads, skill reads, searches and other tool calls come from the same recorded rows the Conversation
  shows, so a worker that only read files still shows what it read. Tool steps fold by turn into a
  collapsed block whose line says what happened, such as "Edited 2 files, ran 3 commands, and read 5
  files". Opening it lists the steps the same way in Conversation and the worker view: one row per
  recorded call identity, merging input, status and output updates, with an icon, a plain name and its
  target on one line (Read and a path, Edit and a path, Shell and a command, Read skill and the skill,
  Search and the query, or the tool's own readable name), the same for engine-native tools and Cortex
  tools; the coordinator or worker role that native activity records before a call's title is not part of
  its name. A step opens on click only when there is something to see beyond its row: a command with its
  full command line and output, an edit with its file (linked to Code) and its diff, or a refused call
  with the runtime's reason in words (never its raw reply). An edit's diff is open with the step, with no
  second disclosure: the changed lines, as `+` and `−` lines with up to three unchanged lines around them,
  from the call's recorded before/after text (a write's whole text, an edit's replaced and replacement
  text, or the engine's own diff) or, on a worker's page, from the runtime's record of that Cortex write;
  the row carries the edit's `+N −M`. An edit with no recorded lines is a plain row with its file and,
  when known, its counts: it never opens on an empty diff box. A step with nothing more to show, such as a
  read of a named file, is a plain line with no disclosure; its file path on the row opens the file in
  Code. A read of the project folder itself names it "project root" and opens Code on the project's files.
  Opened blocks and steps stay open, and the reading position stays put, through every live update,
  including a block growing, its first steps leaving the bounded live window and the turn finishing. There
  is no verbosity setting. Stored Markdown escapes are removed in prose, results and finished worker-list
  previews, while inline and fenced code retain their real backslashes. Character references (`&amp;`,
  `&#32;`, `&#x20;`) read as the characters they stand for, as text, and stay literal in code; an image
  reference reads as an "Image: <name> (<host · file>)" chip, a link only for an http(s) address, and no
  picture is loaded from a message. Messages, results and reports read Markdown headings, bold, italic
  (`*x*`, `_x_`), strikethrough (`~~x~~`), inline code, links, lists, quotes, tables and rules (`---`). A
  fenced code block is one panel with a header naming the fence's language, when it names one, and its own
  Copy, which copies the block's lines exactly as written, without the fences, and then says "Copied" or
  "Not copied"; the message's own Copy still copies the whole Markdown source. Worker notes span the content columns; internal message prefixes are
  not displayed. Tool labels, icons, Worked summaries and which calls appear reuse the Paseo integration’s
  presentation functions, so both clients show the same rows; an engine's raw tool name never reaches the
  screen, and an unknown tool gets Paseo's humanized name. Every tool the runtime advertises has its own
  words (for example List model routes), whether the engine names it `mcp.cortex.…`, `mcp__cortex__…` or
  `cortex.…`, so Codex's `execute` kind on an MCP call never makes a Cortex call a Shell row; only
  `command_run` and `outside_command` are commands. Thoughts, context figures and other engine status
  updates stay hidden before grouping and counting; report bodies are never shown, and the published
  worker result remains visible. Native commands never invent an exit code or offer gateway Stop.
- Recorded output: the output of an opened step, a command's output tail and an edit's diff each sit in one labelled box that keeps lines exactly as written. A long line scrolls sideways inside its box, and the box scrolls vertically as well, up to 280 px (320 px for a diff). Nothing is cut off at the column's edge, and the column and the page do not scroll sideways for it. The box is a named stop in the Tab order; once focused, arrow keys, Shift+wheel and a trackpad move it. Its name is the label, such as Command output, Output, Diff or Edit.
- Tool call detail: every tool call except file reads and edits opens on click into one shared detail, the same in the Conversation, a worker's page and the coordinator's activity. It shows two separately labelled boxes, Input and Output, taken from what the runtime recorded for that call. A box says why it is empty and never claims more than it holds: "Waiting for output…" while the call is pending, "No input was recorded." or "No output was recorded.", "Private or redacted." (private report, draft and result bodies are never shown), "Recorded empty.", or a notice that the recorded value is clipped. Text over 1,000 characters starts as a preview ending in an ellipsis; Show all retained text reveals the whole recorded text (Show preview folds it again) and Copy copies it, then says whether it copied. A file read keeps its path link and a file edit keeps its diff; neither gets a generic Input/Output detail. The runtime's own steps (command status checks, skills, task context, report drafts, questions, access and scope requests and result submissions) stay hidden unless Settings › Tool calls › Show every tool call is on; with it on they are listed and open like any other call. The setting changes visibility only; everything stays recorded either way.
- Command output on demand: a `command_run` or `outside_command` call recorded by the runtime's command broker carries only a reference (attempt and call identity), never the output. Opening the step asks the local service for a 4,096-byte preview; Load more output fetches further pages of up to 16,384 bytes. The detail shows the exit status or signal, the bytes loaded of the total, and whether capture was complete or truncated, and its Copy copies only the loaded text. The output is the broker's combined stdout and stderr, never separate streams. Bytes that are not text are shown as base64 or labelled as converted. Output that was never captured or has since been removed reads "Command output is unavailable."; a command still running reads "Waiting for output…". A call with no broker reference (an engine's own shell, a coordinator row recorded before the reference existed) shows only what its row recorded and offers no output read. The read exists on the desktop; the phone and remote services refuse it.
- Live activity: a quiet Working… indicator remains while the task or worker runs. On a worker's page it reads Reading…, Editing… or Running command… only when that worker's recorded activity establishes it. The Conversation's line speaks for the coordinator: while workers run it reads Waiting for and the running worker's role (or the number of running workers), never a worker's step. It stops for terminal or waiting states and respects reduced motion. While the task waits for the owner (Needs you) and no worker runs, the Conversation's last line says Waiting for you, as the header does, with no spinner. The stream arrives at tool boundaries; missing output and clipped previews are explicit.
- Following: every feed follows one rule, on the desktop and the phone alike: a task's Conversation
  (including the task that plans a sprint), an earlier sprint's planning chat, a worker's activity and
  the coordinator's activity. At the very end a feed follows: streamed text, new rows and expanded
  Markdown keep the end in view. Once the owner moves away from the end (wheel, keys, touch, a press on
  the feed's own scrollbar or keyboard focus moving up) it stops following, and nothing moves what they
  read: appended content keeps the scroll offset, and a row above the viewport changing height keeps the
  visible reading anchor still. Only a move of the feed itself counts: a wheel or touch over a command's
  output or a block's steps that can still scroll moves that box, and a feed shorter than its box never
  stops following. A click or a text selection in the rows is not a scroll, so a feed at its end stays
  there while content streams. A background read, a re-rendered row or a change of the dock never moves
  a feed away from its end. Reaching the very end again, by any means, follows again.
- Latest is the only follow control; no feed has a Follow switch. While the owner is away from the end it
  reads "↓ N new" when that many entries arrived since, and "↓ Latest" otherwise; it jumps to the end and
  follows again. It floats over the feed's bottom edge just above the dock (the team bar and composer),
  with no band or background of its own: the feed shows around the pill, which keeps its own opaque
  surface, and the area around the pill takes no pointer, so the feed under it still scrolls and its rows
  still respond. It lies out of the dock's flow and changes neither the dock's nor the feed's size, so it
  coming or going never moves the end. The folded "Send a separate message" row under an open question
  keeps its opaque band of the timeline's surface inside the dock.
- Opening a block or step in any feed stops following, so the end no longer pulls the opened row away,
  and scrolls the opened content into view only as far as the row's heading stays visible. Answer, See
  what is needed and a notification bring their card into view the same way: following stops first, so
  the end never takes the feed back from the card.
- The owner's own message or answer sent from a feed's screen (the composer, Request changes, an answer to
  a question or a permission, a message to the planner or to a worker) brings that feed's end into view
  and follows again, so it and the work it starts sit above the composer. A message that arrives from
  another window, or the service's record taking the place of a sent line, never moves an owner who
  scrolled up: it counts on Latest as new. A press held on the feed's scrollbar is never overridden by
  content updates, and letting go away from the end leaves the feed where it is.
- The worker's activity has no toolbar strip above it: no search field and no Follow switch. Following resumes on its own when the owner scrolls back to the last row, and Latest (or "N new") brings the end back. Automatic paging loads the rest of the history. Refreshing the same worker attempt preserves loaded pages and in-flight page reads; switching attempts discards them.
- The worker's header has no Stop button of its own; a running worker is stopped from its composer (see Worker messages below).
- Stop a command: pick the running command, enter a required reason, then confirm. States are sending, requested, stopped, missed (the command had already finished) and refused. The worker receives the reason and continues. This is the only control that interrupts one action of a worker.
- Waiting for you: the question card is pinned above the worker composer, with options as buttons, "Other…" and Stop the task. "Other…" opens a free-text answer field with line breaks (Shift+Enter) and attachments, as in the main composer. Files can be dropped into the field, and a copied screenshot can be pasted into it ([CW-R39](requirements.md)). The composer placeholder becomes "Answer the worker's question…", and text sent there is an answer.

Finished worker view:

- Header: status (Finished, Finished with issues, Failed, Cancelled or Interrupted), duration, route, final context, tokens and attempts.
- Result: a card at the end of the stream with the worker's state, "Open report" and the attempt's own summary, clipped at 16,000 characters with a visible note.
- Checks: one row each. Outcomes are passed, failed, not run, unverified, closed by rerun, retracted and superseded. Not run is never shown as green. The blocker or evidence gap is shown for not-run and unverified rows. A missing record shows "Check details unavailable", and an empty list shows "No checks reported".
- Report: "Open report" retrieves all bounded public pages before showing the complete Markdown document, without Next page. Headings, lists, tables, quotes and code are formatted; code keeps literal backslashes. Supplied HTML stays inert. Relative, HTTP(S) and mailto links are allowed; executable/unknown schemes remain text. An open report survives background refreshes. Very large reports still require initial rendering time; this is not a guarantee of frame timing on every machine.
- Changes by this worker: the file list with counts and per-file diffs, labelled "From recorded edits (may omit commands' side effects)". "Open task Changes" shows the real branch diff.
- Activity: the full history, read-only, following by the same rule as every feed. Re-opening a finished worker always restores it.
- A finished worker accepts no messages. Its result card ends with Ask about this worker, which pre-fills the task composer with a worker reference and returns to Conversation. The button is part of the card, after its checks, so it never floats over them.
- Review stage: reviewer rows show the verdict (Approve, Changes requested or Block). The reviewer view lists findings with severity, file and line, each linked to the cited activity.
- Verification lane: each runtime command with its argv, elapsed time, exit code and output tail. A failed verification shows a readable reason followed by its code.

Worker view states: loading (skeleton rows and header, never "empty"); just dispatched ("Waiting for first activity"); history still loading ("Loading recorded activity…" while the remaining pages load automatically, with the rows loaded so far kept); history read failed (loaded rows kept, with the error and Retry); offline machine ("Offline · last update 13:47", with the composer disabled and the reason shown); interrupted ("Worker interrupted · Coordinator will retry or re-plan", and pending messages become "Not delivered"); narrow windows below 1280 px hide the inspector; below 900 px the full stream uses a worker picker, with horizontally scrollable tabs.

## Worker messages (desktop)

A composer at the bottom of a running worker's view reads "Message <role>…". It has an attach control, a send-mode chip, a voice control and Send. While the worker runs (working or waiting for the owner) and the field holds no text or files, Send is Stop, exactly as in the task composer: the same slot, size and stop icon, named "Stop worker…", with the effect line "Stop ends only <role>; the task keeps running." It opens the Stop <role>? dialog, whose optional reason goes with the stop when it is confirmed; nothing stops before that confirmation, and Enter never presses Stop. As soon as text is typed it is Send again. Where the machine does not offer stopping a worker, Stop stays in its place, off, with the reason. Files can be dropped into it, and a copied screenshot can be pasted into it ([CW-R39](requirements.md)). The worker's model, effort and access mode are shown read-only, because the route belongs to the runtime. The same composer opens from the worker row menu, and a Dialog worker row opens the full worker stream.

- Send mode "At next step" is the default. The message is appended to the worker's next Cortex tool result, or to the next prompt of the same session if the turn ends first. A worker that uses only native engine tools receives it at its next Cortex call, and the UI says so: "Delivered when the worker next calls a runtime tool".
- "Interrupt now" is shown disabled, with the reason "Not available for this engine yet", until the engine is qualified for it.
- Message states, shown as words as well as icons: Sending…; Pending · next step (Withdraw is available until delivery); Delivered at a time (recorded in the worker's activity, which is not proof that the model acted on it); Withdrawn; Not delivered · worker finished (Send to Coordinator instead, or Discard); Not delivered · stopped, cancelled or replaced (Send to new attempt if one is running, Send to Coordinator, or Discard); Failed · not sent (text stays in the composer, with Retry).
- Several pending messages are delivered together, in order, at one delivery point. A pending badge appears on the worker row.
- The owner's text reaches the worker verbatim, framed as guidance that changes no scope, permission, review or acceptance criteria.

## Task send modes

- Enter sends in the delivery mode the Queue toggle beside the plane shows (off sends now, on queues), and only that visible toggle changes the mode; Ctrl/Cmd+Enter does not queue. Shift+Enter inserts a newline. Help and Settings → Shortcuts say the same. Key bindings are proposals until the owner decides them ([open choices](DESIGN.md#open-choices)).
- Steer: captured at once; running workers, review and verification continue (nothing is cancelled). The coordinator receives it at its next step: after the current planning turn, or once the workers, review and verification of the revision finished. Steer cannot be edited or removed after capture; a correction is a new message. Under the message: "Received, waiting for the coordinator's next step" (paused: "Received, kept for when you resume"), then "Delivered to the coordinator" and the time the engine answered the prompt that carried it; a send that did not arrive is Not sent with Retry. A refused capture means nothing reached Cortex, and running work continues.
- Queue: held by the machine until the service sends it at the coordinator's next step. Queued items sit
  above the composer. Each item shows its text on one line and offers Edit, Send now (which sends it as
  Steer), remove and reorder (drag or Alt+Up/Down); its files are not listed in the row. Edit removes the
  item from the queue and returns its text and files to the normal composer, where they can be removed
  before it is sent again; the queue never edits an item in place. Send now sends the files with the
  message, and they then open from the conversation. The queue is as wide as the chat column. Its
  background is transparent at its edges and in the gap between the queue and the composer, so the
  messages behind it stay visible. Its controls are never clipped; long text shortens with an ellipsis and
  opens whole on a press or Enter. The queue is kept by the machine, and the Workspace service sends it,
  window open or closed: at the coordinator's next step it takes the task's queued messages in queue
  order. A taken message leaves the queue and reads "Taken from the queue for delivery" in the
  conversation; from then on Edit, move and remove refuse ("This message was already taken for
  delivery…"). Until then it can be edited, reordered or removed.
- Edit over a non-empty composer draft asks first: Replace draft, Keep both or Cancel. The composer never overwrites a draft silently.
- When Cortex is idle the two modes are identical, so the mode chip is hidden.
- While a coordinator question is open, the answer composer sits inside the question card, and the general composer folds to "Send a separate message". Its answer field supports line breaks and attachments, as in the main composer, including drag and drop and paste ([CW-R39](requirements.md)).
- One action button, as in other chat clients: with an empty field it is Start on a draft, Stop while the
  task runs (not while it waits for the owner's answer, is held or is in review) and Resume while it is
  paused; with text or files it is the paper plane (Send now or Queue, Send and resume, Send and reopen),
  and clearing the field brings Stop or Resume back. It keeps one size and place whatever it is, the
  effect line says what pressing it does, and Enter never presses Start, Stop or Resume. The composer is
  the group named "Task composer". The title band has no state chip and no Start, Stop or Resume (the
  strip under the tabs, the sidebar and the Inspector say the state; the Inspector names the next step);
  it keeps the one next step that is on the screen nowhere else (Answer, Review changes, Continue, the
  closing in Ready to merge, Close task… for a failed or cancelled run, Archive for a closed task,
  and Return for rework where eligible), the
  Git and Open buttons and the task menu, which keeps Start, Stop, Resume and Continue. A machine with no
  message composer keeps Start, Stop and Resume in the title band. A sprint's planning composer works the
  same way: Stop cancels planning and Resume continues it. Stop opens the stop dialog, which offers Stop
  and Continue, Close as is, and Cancel task; see [CW-U08](backlog/CW-U08.md).

### Return for rework

For eligible closed tasks, including Done and **Closed, not verified**, choose **Return for rework**
from the title band, task menu, or Board or List card menu. Dragging a card out of Done does not reopen it.
The action opens the shared message composer in Chat and focuses the message box. Its effect line
reads “Sending reopens this task for rework”. Enter a message (attachments are optional), then choose
**Send and reopen**. This is not ordinary **Request changes** or a late chat message.

If the app knows a dependent sprint task already started on the old result, it asks you to confirm with
**Cancel** or **Send and reopen**. Reopening does not stop or change those dependent tasks. Cancel or
a refusal keeps your draft. An archived task has no rework action: restore it first. See
[rework eligibility and behavior](local-task-execution.md#return-for-rework).

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

## Notifications (desktop)

The machine service raises the desktop notifications, so they also arrive while no window is open ([notification behavior](maintenance-and-controls.md)). Each names the work (a task or sprint title) and its place, never a question's text, and clicking it raises the window and opens that place as opening it by hand would:

| Notification | Raised when | Opens |
|---|---|---|
| Question waiting | a question or permission waits in a task | the task's Conversation, scrolled to that question while it still waits |
| Task stopped: your decision | the runtime stopped a task and asks the owner; the second line is its headline | the task's Conversation, at that question |
| Result ready for review | a task has a result the owner has not accepted (not for a sprint in Automatic mode, which accepts it itself) | the task's Conversation, with the result card in view |
| Task failed | a task failed or could not start, with no question | the task |
| Planning question waiting | a sprint's planning asks the owner (its planning task's question included, never raised as a task's) | the sprint's planning chat, or its Conversation for a sprint planned by a planning task |
| Sprint paused | a sprint paused itself; the second line is its reason (not Manual mode's stop after a result, nor the owner's own pause, close or delete). In an Automatic sprint a saved hold that requires the owner raises it, including a blocked acceptance or integration | the sprint |
| No progress | a working task (not paused, waiting for you or finished) has made no progress for the chosen time, 3 hours unless changed: "<title> has made no progress for 3 h." and a second line with what it spent (time working, turns, tokens or "tokens not reported") and the short reasons its last attempts hit. Again every 3 hours while it stays stuck; the task keeps working and is never paused or changed. Progress is an accepted plan, a worker result, an approving review, passed verification or a passed check (or the runtime's `TASK_PROGRESS` records once it writes them); the clock restarts when the task resumes or you answer, steer or continue it. The runtime records `NO_PROGRESS_NOTIFIED` with the numbers only | the task |

Routine progress, an automatic acceptance that went through and anything on view in the focused window raise nothing. One notification stands for each task or sprint: a newer state replaces it (an answered question followed by a result, for example), and resolving it any way (answering, accepting, closing, archiving) withdraws it. Settings, Appearance turns notifications off and on, and below that switch each kind on its own; all are on at first. Below the kinds, "Tasks with no progress" picks Off, 1 h, 2 h, 3 h (the default) or 6 h. While a working task is past that time its title band shows a quiet "No progress for N h" badge. The task inspector's Spend section and the top of Work details show what the task spent, from the runtime's records only: time in a working state, engine sessions and turns by engine and model, and tokens where an engine report is stored, otherwise "Not reported" (the runtime does not record per-turn token reports yet). It also holds the "Show every tool call" switch described under Task workers.

A tab that already shows the task or sprint (any of its tabs) comes forward instead of a second one, the active tab first, and moves to the place; otherwise the active tab goes there, and Settings with unsaved changes asks first. A question already answered or withdrawn opens the task or sprint without scrolling; a task or sprint that no longer exists opens its project's board, and an unknown project Home. No dialog is shown. Opening a notification marks nothing that opening the place by hand would not. A click while no window is open starts the app on that place. Platform behavior and limits are in [platforms](platforms.md#desktop-notifications-on-gnome-and-wayland).

## Sudo password prompt

**Status: built in the shared interface (`packages/workspace-ui/src/sudo-prompt.tsx`, `sudo-prompt.css`) and shown by the desktop and the phone shell alike; covered by Vitest and no-paid service tests with a fake `sudo` only. Not seen in the native window or on a device, and no real sudo password has been typed through it.**

When a command a task runs on the machine asks for root with `sudo`, a card titled **Password for sudo** appears over the bottom right corner of the window (on a phone, across the bottom of the screen). It is not modal: the rest of the interface, and the task's other work, carry on; only the asking command waits. The card names the project, task and worker, shows sudo's command line in a code block and the folder it runs in, says that the password goes to sudo on the machine and is neither saved nor shown to the model, and counts down to its close (10 minutes). Below that is a masked password field (no autofill, spell check or capitalization) with **Cancel** and **Submit**. Submit sends the password once and clears the field; the card closes when the machine confirms. Cancel sends a cancel and sudo fails as it would without a password. A refusal (the prompt already closed, or a password that spans lines) shows under the field in words. Several open prompts stack, oldest first. The prompts are read every 2 seconds while the machine is connected; an older service without them shows nothing.

On a phone the same card answers over the paired connection. While the computer's Settings, Remote access, **Passwords and tokens from phones** is off, the phone's card has no field and no Submit: it says passwords and tokens from phones are turned off on the computer and offers Cancel only. That switch is on by default and appears only when the machine reports it.

## Owner action card

**Status: built in the shared interface (`packages/workspace-ui/src/owner-actions.tsx`, `owner-actions.css`, `terminal-command.ts`) on the service's `owner_actions.list`/`owner_actions.act`, and shown by the desktop and the phone shell alike; covered by Vitest and no-paid service tests with a fake `gh` and fake tokens only. Not seen in the native window or on a device; no real sign-in or token has gone through it.**

When a task's work needs access the machine lacks (it is not signed in to GitHub, Codex or Claude, or `sudo` needs the owner), the task does not stop and the blocked work waits. A card appears over the top right corner of the window (on a phone, across the top of the screen), titled by what is missing: **Sign in to GitHub**, **Sign in to Codex**, **Sign in to Claude**, **sudo needs your password**, or **Access needed:** and the service's name. It is not modal. It names the project and task, says in plain words what the task could not do, and offers what the machine supports for that service:

| Service | Open sign-in in terminal | Paste a token | Check again |
| --- | --- | --- | --- |
| GitHub | `gh auth login` | yes (`gh auth login --with-token`) | yes |
| Codex | `codex login` | no | yes |
| Claude | `claude auth login` | no | yes |
| sudo | no (the card says a Password for sudo prompt opens when the command asks) | no | yes |
| other | no | no | no |

- **Open sign-in in terminal** (desktop with a terminal) opens the task, opens the bottom terminal on its project shell (the project folder) and types the login command there once. Without a terminal, and always on a phone, the card shows the command in a code block and says to run it in a terminal on the computer, then check again. The phone has no terminal.
- **Paste a token** is a masked field (no autofill or spell check) with **Sign in with token**. Submitting clears the field at once; the token goes to the CLI on the machine once and is not saved or shown to the model ([security](../../SECURITY.md#owner-actions-sign-ins-and-pasted-tokens)). On a phone with Settings, Remote access, **Passwords and tokens from phones** off, the card has no field and says to paste the token on the computer.
- **I've signed in, check again** (for sudo, **Check again**) asks the machine to check now; when the access is there the card closes, otherwise it says the sign-in is still missing. The machine also checks every 60 seconds while the card is open, and a sudo card closes when a Password for sudo prompt of the same task is answered.
- **Dismiss** closes the card; the task retries the blocked work anyway. **Open task** goes to the task.

Several cards stack, oldest first. They are read every 5 seconds while the machine is connected; an older service without them shows nothing. A new card raises one window notification on the desktop (through the window's own notifications, while notifications and "Questions and permissions" are on and the task is not on view), opening the task; it is announced once. The phone has no notifications.

## Keyboard and visual rules

- Esc never stops a task. In a recording, Esc cancels the dictation. This deliberately differs from Paseo and is an open decision.
- Ctrl+1 to Ctrl+9 switch workspace tabs, so a task's own tabs have no number keys.
- Proposed keys: Ctrl+Shift+P toggles the plan graph; Ctrl+Shift+F filters workers; End jumps to the latest activity; Ctrl+. opens the command-stop picker (never an immediate stop); 1 to 8 choose a question option. Each key is checked for conflicts on the target platform before implementation.
- No focus or press highlight. A click, a press, a route change or a dialog opening leaves no outline, ring, border change, fill or accent on any control, and a text field shows only its caret. Selection that means something (the current page, the open task) stays: it uses a neutral fill, a visible text or icon marker and `aria-selected`; a fill alone does not meet the contrast needed for selection. Selected labels never change font weight or size, and selection never shifts layout ([CW-R34](requirements.md)). There are no coloured side stripes. Only the running marker pulses, and it is static under reduced motion.
- Keyboard position: moving with Tab or an arrow key draws one quiet 1 px neutral line around the focused control (`--focus-line`), and a text field shows only its caret. The only focus mark is that line, and the next pointer press clears it. A control that a dialog or panel focused as it opened shows the same line, where the keys act, until the next press. A row that fills its scroller or list draws the line just inside itself, so nothing cuts it. The app records the last way of moving in `data-keyboard` on the root element, because the engine's `:focus-visible` also matches after a click that opens a dialog. Checkboxes and radios are drawn by the stylesheet, so no native focus ring can appear on them.
- Panel toggles: each icon that opens a panel (sidebar, worker pane, terminal, inspector) shows that panel's real state by colour alone: lighter while the panel is open (`--panel-on`), darker while it is closed (`--panel-off`), in both themes. Hover and click add no fill or outline, the icon never moves, and a closed panel always reopens from the same icon. On a screen with no such panel the toggle is not shown and its place stays empty, so the band never shifts. The worker pane toggle is shown only where it controls the pane: the Dialog tab at 1800 px and wider.
- [Desktop tooltips](#desktop-tooltip-inventory) are selective; they never use native titles. A control that cannot be used says why in visible text: in its bar's reason slot beside the controls or, where the bar has no slot, in a box that a press or keyboard focus opens beside the control. A composer has no reason slot: nothing is written beside its send button, and each of its controls (send, Attach, Voice, model, effort, Settings) says why in its own box. The reason is never drawn inside the control and never opens on hover. A control that nothing could ever use on this screen is not offered at all.
- Window edges: the frameless window keeps the outer 5 logical pixels of every edge (multiplied by the display scale) for edge resizing. Scrollers at the right and bottom edge stop short of that zone, so a scrollbar can always be grabbed and dragged and the window can still be resized from its edges.
- Shadows: none, or so faint that they are barely visible. No card, popover, queue or composer uses a visible shadow.
- Interface words: a label never names the program, because the whole program is Cortex: "Use", not "Use in Cortex". The agent you talk to reads as Coordinator where a name is needed. Labels and hints are English only.
- Task state: a same-branch (or no-Git) review points to Ready to finish, not Ready to merge, in the
  header and the review bar alike (Accept moves the task to Ready to finish), and so does the New task
  form's hint for a project-folder task on the branch it would merge into ("The task stops at Ready to
  finish for your confirmation."). A task working again after Request changes or Return for rework shows
  no result and no Accept until its next result exists, and its Inspector counts only the current version's
  worker checks, never the previous version's. A worker check the runtime kept
  unverified says why in words (run before the final files, or a command that tests nothing), never that a
  tool was unavailable: every reason code the runtime records reads as what it means, also inside a
  reviewer's own sentence, and an unknown code is named as recorded, never as a missing tool or service.
  Once the runtime's own verification of the version passed, self-reported checks it does not cover are
  listed on the result card as notes and do not keep Accept off. A worker's failed probe it withdrew is
  not counted once the runtime confirms the withdrawal from the run it was bound to; a check that still
  keeps Accept off is named in the reason beside it ("Worker check “…” failed. Rerun that check or request
  changes."), never a generic instruction. A task in review shows its result card as soon as its page
  opens: the summary, the version and the runtime's checks come from the result's own record, and while
  the result is still being checked against the project folder Accept stays off with "Checking the result
  against the project…" and no other action is offered; the same card fills in when the check lands,
  without being rebuilt. A board read does not hide a result card already shown. A task in Ready to merge
  whose files are still being checked against its target branch says so in place of its commit facts and
  offers no commit until they are known. The reason beside a disabled Accept ends in one full stop. A
  project-folder sprint task's result is not called stale because a later task wrote other files, and a
  result the app committed itself is not stale. The inspector shows the completed outcome once, without
  repeating the same status as a second fact.
- Held and ended tasks: a task the runtime holds for the owner reads Interrupted only for a lost turn or
  an outcome nobody could observe, and Stopped for a stop the runtime decided; its reason reads in words
  wherever it shows (Home, Inbox, the strip), never as a raw code, and the header offers Continue (the
  runtime's Retry: the coordinator decides the next step from the recorded state; nothing is replayed). A
  previous result next to such a task says it waits for the owner's message, never "Work is running", and
  its Accept waits for the task to finish. Finish chosen on the runtime's stop question closes the task as
  is, so it leaves Needs you, the Inbox and Home at once. The machine presents that close as a saved close
  as is (not verified), so the task archives like any closed task, by Archive or automatically after its
  days in Done, and a sprint counts it as closed and moves on. Returning it for rework takes it out of
  Done and stops the Done-days archive countdown. The
  runtime's stop question card shows its full explanation (what happened, what each answer does), each
  option's consequence under its label, a Recommended mark on the recommended one, and buttons for what it
  names: Open review findings (the reviewer's page with its report), Open task “<title>” (the task holding
  the files), and the denied path as text. A question it replaced reads "Replaced by the question below:
  <reason>"; another withdrawn one reads "Withdrawn: <first line>". A working task whose next step waits
  for another task's files reads "Waiting for the task “<title>” to release <files>" on its Working card
  and in a visible status strip on its opened page. The title matches the holder's card and links to that
  task from the strip when available. The waiting step starts by itself. A paused task waits in Needs you (Paused, with Resume), never in Working; a failed or
  cancelled run there reads as its outcome. Its title band offers Close task…, whose dialog closes it as
  is or cancels it and offers no Stop; its composer says so. A closed task can be archived without a
  question. A runtime result that can never be accepted (its evidence is unavailable,
  or it comes from an earlier version) offers Close as is… beside the reason Accept is off. The sprint
  Summary counts Needs you apart from Working. A project-folder task whose record names no target branch
  reads Ready to finish, not Ready to merge. An answer that changed no files reads Nothing to commit with
  Close task, and a closed task offers no Commit in Changes. A worker whose result submissions were
  refused says so on its card and, where the binding is exact, on each refused Submit result step.
- Sprint queue (Hybrid): "All queued tasks have run. Finished results wait for your confirmation; nothing closes by itself." stays only while a task of the sprint is still open; once every task is closed the queue says nothing.
- Identifiers: task, assignment, report, route and connection ids and build hashes never appear in the interface. Diagnostics is the one exception, and it shows the build and connection identifiers for support. A version elsewhere reads as `1.0.6`. Two connections offering one model are told apart by connection name.
- Counts: nothing to count shows nothing, never "All 0". What differs from a profile is listed in words, for example "Claude: 4 sessions at most, was 2".
- Titles and names are edited by clicking them. There is no separate Rename or Edit control. A click opens the name as a field with Save and Cancel beside it; an unchanged name also closes when focus leaves it. A task's title stays editable after Start, review, finishing and closing, because it is presentation, not lifecycle; only a cleared task's title is read-only. Edit task… (the saved request and its settings) remains for a never-started task only.
- Text is saved only by Save, or Enter in a one-line field; Cancel or Escape puts the saved text back. Nothing saves on leaving a field or after a pause in typing, and a read of the machine never replaces text still being edited. This holds for the project name in project settings, the machine name, task and sprint titles, connection and profile names, and the new task's title. A refused save keeps the typed text with its reason below the field. The rules are in [editing text](DESIGN.md#editing-text).
- Selection: text can be selected and copied everywhere, with Ctrl+C. Buttons, links, the title band, the sidebar and draggable cards are excluded.
- One piece of text, such as a path, is cut with an ellipsis and never broken across lines. Text with a full-text box still opens it on press, Enter or Space. On desktop, marked text that is actually cut also has a delayed hover and keyboard-focus tooltip. Text that fits has none.
- Absent values: a value Cortex does not have is not rendered. No dash (—) or placeholder glyph stands in for it, and an unknown value is never shown as zero or as success. When a whole panel has no data, the panel carries one state line with its reason, not a placeholder in each field.
- Diff colours are always paired with `+` and `−` text. Every icon has a text label.
- A polite live region announces real transitions only, such as "Implementer finished with 1 issue". Polling never announces.

## Desktop tooltip inventory

The shared desktop tooltip opens after 500 ms on pointer hover or keyboard focus. Moving the pointer away closes a hover-only tooltip; it does not close or restart a tooltip for a keyboard-focused control. Blur, press or Esc can still dismiss it. It is linked with `aria-describedby`, has no native title, and is at most 320 px wide. It flips or shifts away from the trigger and surrounding controls; if no position fits, it stays hidden. It adds no tab stop of its own. The phone shell does not mount this layer.

| Where | Triggers |
| --- | --- |
| Desktop frame and sidebar | Sidebar (Ctrl+B), Back, Forward, Project actions, Terminal (Ctrl+J), Inspector (Ctrl+Alt+B), show/hide a project's tasks, Sort projects, Settings (Ctrl+,), Help and shortcuts, and New shell in this checkout. The connection chip explains Connected or Offline; numeric Inbox and Archive badges say what they count. |
| Workspace and editor | New tab (Ctrl+T); Close shell and Hide terminal (Ctrl+J); Refresh files, Collapse all, Match case (Aa), and File actions. |
| Composer | Attach files, Voice input (Ctrl+D), composer Settings, the send/save/queue or run-action icon, the queue reorder grip and the Queue delivery toggle. The send action shows the writing field's Enter or Ctrl/Cmd+Enter shortcut where it applies; Start, Stop and Resume do not. The Queue tooltip describes the current switch position without changing it. |
| Git and conflicts | Refresh Git status, Fetch, Pull/Pull…, Push; Stage/Unstage/Revert; commit options and branch action menus; Open the project folder; previous/next conflict. Git letters expand to the status word; the ahead/behind summary explains the counts. |
| Task and sprint views | Task/card action menus, icon-only Sprint planning and Sprint summary, More queue options, Filter and Sprint actions. The graph's previous/next match, zoom (−/+), minimap (M) and overflow have hints; graph task nodes show the full name and status or dependency summary. Labelled graph buttons and tabs do not repeat their labels. |
| Archive, settings and dialogs | Archive and settings row menus; Remove list entries, variables and windows where shown as icons; the settings `?` (About…) still opens its help on press. Icon-only close/cancel controls in task, sprint, Git, Archive, code, machine, worker and Help surfaces say what pressing them does, with Esc where applicable. |
| Marked cut text | Existing full-text targets (such as paths, title-band crumbs, branch names and row text) and marked cut names in cards, board rows, sidebar titles, workspace tabs and the file tree, including cut board warnings and check summaries. Uncut text and full breadcrumb names have no tooltip. |

No tooltip repeats a clear text action such as Save, Add project, New task, Search, New branch, Fit, or Full view, and menu items with printed shortcuts already show their keys. Essential information remains available without hovering: the controls keep their accessible names and visible state, marked text keeps its press box where provided, and disabled reasons keep their existing visible slots or press/focus boxes.

## Theme (desktop)

- The theme is System, Light or Dark, chosen only in application settings. The default is System. The sidebar and window chrome have no theme switch ([CW-R33](requirements.md)).

## Worktree settings (desktop)

- Settings › Worktrees is a per-machine page. The real service reports both the current folder and its absolute default. The default is `$XDG_DATA_HOME/cortex-workspace/worktrees` in the service environment (fallback `~/.local/share/cortex-workspace/worktrees`); the dev service isolates XDG data under `<channel-state>/data`, so its default is `<channel-state>/data/cortex-workspace/worktrees`. The field, "Use default" action and example all use that reported path; reset is unavailable until it is known. New worktrees use `<root>/<project-key>/<task-id>`, where project-key is the 16-character hash of the common Git directory. The folder is typed in the built page. Saving changes only future worktrees and never moves existing checkouts.
- A new path is checked before it is saved. It must be absolute, writable and outside every registered project checkout. A failed check is shown beside the field with its reason, and the saved path does not change. An offline, stale or unsupported save shows "Not saved", as in machine settings.
- Task creation offers two checkout choices: the project folder on its open branch, or a dedicated worktree under the saved root. If the saved root is unavailable when a task starts, the task does not start in another folder; it shows the reason.
- Project worktree options (branch prefix, setup script, files copied into new worktrees, removal after merge) stay in the project's Worktree settings.

## Orchestration settings (desktop)

**Status: built in the Workspace interface (`packages/workspace-ui`), part of the unreleased 1.0.6 changes.** The eight-section structure below is the built structure. It replaces the former nine-page structure (Overview, Coordinator, Models, Routing, Route preview, Limits and fallback, Tools and MCP, Profiles, and Files and presets) and the separate Engine connections item. Where the build differs from the Paseo panel, the difference is stated in this section.

Owner decision (2026-11-19): the Workspace orchestration settings follow the structure and the way of configuring of the Cortex settings panel in the Paseo plugin. The same sections appear in the same order, each setting is configured the same way, and each section keeps the same save model. The Workspace visual language is applied on top. The owner chose to implement without a mockup approval step; this section records the built behaviour.

Per-machine orchestration settings reach field-level parity with the Cortex configuration that Paseo exposes ([CW-R41](requirements.md), [CW-R23](requirements.md)). [CW-U07](backlog/CW-U07.md) holds the field inventory and the acceptance criteria, and names the section for each field.

Each section is scoped to one machine. The context bar shows the machine name and connection state, and the profile bar below it shows the active profile and its state. The Project picker appears only where a project scope applies.

Eight sidebar sub-items sit under the machine's Orchestration item, in this order:

| # | Sidebar label | Paseo section | Writes | Project picker |
|---|---|---|---|---|
| 1 | Defaults | User defaults, with the default preset card | `user_defaults` (preset actions run at once) | Hidden |
| 2 | Models | Models | `routing` | Shown |
| 3 | Engines and models | Engines & models | `routing` | Shown |
| 4 | Project ceiling | Project ceiling | `project_ceiling` | Shown (required) |
| 5 | Routing | Routing | `routing` | Shown |
| 6 | Profiles | Profiles | Profile commands (at once) | Hidden |
| 7 | MCP servers | MCP servers | `mcp_servers` | Hidden |
| 8 | Connections | Connections | `connections` (commands at once) | Hidden |

- Deliberate difference from the Paseo panel: the Paseo panel shows its workspace picker above every section. Workspace shows the Project picker only on sections 2 to 5, which take a project scope; sections 1, 6, 7 and 8 are machine-wide.
- Engine connections is no longer a separate top-level machine item; Connections is section 8. The `section: "connections"` route stays, so existing links keep working.
- The bare Orchestration route opens Defaults. Existing deep links resolve: `defaults` opens Defaults; `models` opens Models; `routing` with tab `roles`, `kinds`, `quota` or `rules` opens Routing, scrolled to the matching group; `effective` opens Route preview; `policy` opens Defaults › Machine ceiling under machine scope, or Project ceiling under project scope; `tools` opens MCP servers; `profiles` opens Profiles; `files` opens Files and sources.
- Secondary pages are not in the sidebar. Each opens from a link in the section named, and has a "Back to <section>" link: Route preview (from Routing and from Engines and models); Files and sources (from Routing, from the Defaults notice about kept keys, and from the invalid-routing and migration strips); MCP server add and edit pages (from MCP servers).
- The Overview page is removed. Its needs-attention list moves to the top of Defaults.

### Frame on every section

From top to bottom:

1. **Context bar**: the machine name and connection state, as today. At its end is the **Project** picker, a choice of "Machine only" and then the machine's projects. It is shown only on sections 2 to 5 and writes the same scope as today. With no projects it offers only "Machine only", and the status lane says "This machine has no projects, so only machine settings can be chosen." Below about 720 px a **Section** choice that lists the eight labels sits at the start of the bar, because the sidebar is held away there. Below about 720 px the machine name is not shown in this bar. Below about 560 px the connection state keeps only its icon beside the Project picker; its word is still read by assistive technology.
2. **Profile bar** (see below).
3. **Status lane**: the offline, remote-write and no-projects strips as today, and the routing source strips on sections 2, 3 and 5.
4. **Title and lead**, with section actions at the end of the title row.
5. **Section body.**
6. **Save bar** on the draft sections 1 to 5 and 7. Sections 6 and 8 act at once and have no save bar.

### Profile bar

One line under the context bar on every section and secondary page, with a hairline below it:

- **Profile**: a choice of saved profiles, plus "No profile". Changing it opens "Switch to <name>?" when the machine differs from the profile or drafts would be dropped, with "Save and switch", "Discard and switch" and "Cancel". Otherwise it switches at once.
- **State word**, with an icon and never colour alone: "Applied", "Modified (n)" (opens the list of differences), "No profile", "Interrupted" or "Unavailable".
- **Actions**: "Save to profile" (off with the reason "Nothing differs from <name>" or "No profile is active"); "Revert to saved" (shown when modified); and "Save as new…", which asks for a name; the description is added afterwards under Profiles.
- **Interrupted**: "A switch to <name> was interrupted, so the files may be a mix." with "Activate again", "Save current settings to it" and "Detach".
- **Partial profile**: "<name> stores only <sections>. Save to profile stores every section."
- **Offline**: every action is off with the offline reason, and the picker shows the last known value.
- **Narrow width**: the bar wraps onto more lines as it narrows. Its controls keep the 28 px row size (32 px in the phone shell).

### 1. Defaults (`user_defaults`)

Title "Defaults". Lead: "What a new task on this machine starts with, and the widest limits any task may use." Groups, in order:

1. **Needs attention**, shown only when there are configuration issues: one link per issue to its section.
2. **Default preset**. "Apply preset" creates only the files that are missing and never replaces one; it is off with "Every file exists, so there is nothing to create". "Reset to default…" replaces routing and defaults with the preset after a warning that names the files replaced and the backups kept (`<name>.json.cortex-backup-<time>`), and a literal confirmation. Both are commands, not draft edits; their outcome shows under the group, with "Read again" after a conflict.
3. **Coordinator**. Model: a picker over enabled, available models; each option names its engine. Recommended: "A strong general model; it plans and checks the work." Effort: the model's observed levels; a general list is labelled "Not observed: this is a general list". Status: "Bound to <route>" (the engine and model, or the exact route), or the error with "Fix in Models". "More coordinator parameters" is a closed block of name and value rows that opens when it holds a value.
4. **Language**: Automatic, English or Russian. Recommended: "Automatic, or Russian to always get Russian."
5. **New tasks**: Access mode (Development, Guarded or Auto) and Checkout for new tasks (Worktree or Project folder).
6. **Exact default route**: closed by default, and open when set or in error. Use an exact route or clear it, with pickers for the connection and model and the route parameters. Recommended: "Leave this off and use the coordinator above."
7. **Machine ceiling**: "The widest limits any project or task may use. A project can only narrow them." Recommended: "Leave every restriction off unless you need a hard lock; switch models on and off under Models instead." Rows: Attempts per task (blank means no limit; about 8 to 12 stops runaway retries), Connections and Data recipients. A **Fallback** subgroup, with "Off unless all of these allow it.": Failure classes, Budget, "Allow work to move to another provider", and the fixed note on hard denials (cancellation, refusal, authorization loss, provider restriction). Recommended: "Leave fallback off. If you want it: both failure classes, budget 1, consent only if another provider may see the code." Advanced: Allowed routes and Fallback target routes. "Stored, not applied" rows appear only when set.
8. **Kept keys**, only when the app keeps keys it does not manage: "Kept unchanged, not edited here: <keys>." with a link to Files and sources.

Save bar: one draft for groups 3 to 7, labelled "Save defaults on <machine>". After a save: "Saved on <machine>. Not yet in profile <name>.", with "Save to profile". With a project selected this section shows no project picker, because machine defaults never become project values.

### 2. Models (`routing`)

Title "Models". Lead: "Switch on the models tasks may use. Only these appear in the model selector and in automatic routing. New models stay off until you switch them on."

- Head actions: a "Filter models" field and "Refresh model lists".
- **Model lists**: one read-only row per connection, "<Engine> · <connection name>", with "Observed <time>" and, after a refresh, its outcome. On a failed read: "List could not be read; the previous one is kept." Recommended: "After an engine update, refresh to see new models." When no connection exists: "No connection is registered. Add one under Connections."
- One group per engine, headed "<Engine>" with a count, "n of m on":
  - "Turn on all shown" and "Turn off all shown", when more than one row is shown. They skip rows that a broader rule blocks.
  - When the engine is switched off under Engines and models, a notice says so and links there. Its rows are off with that reason.
  - One row per model: the display name; the model id, followed by "Not confirmed by the engine yet" or "Context window <n> tokens" when observed; and a switch named "<name> enabled". A model that a broader rule blocks shows the reason "Off because of "x", which can cover other models too" and "Remove rule "x"".
  - "<Engine>: not offered now": models that are switched on but no longer offered, each with Remove.
  - Empty texts: "No connection for this engine.", "No models known yet: refresh the model lists.", "No model matches the filter." For OpenCode with no models: "No models. Enable OpenCode under Connections first."
- Strips: the routing source strips (invalid routing, migration, project and environment notes). A connection whose model list could not be read shows "Not read" on its row under Model lists.
- Save bar: "Save routing rules on <machine or project>".

### 3. Engines and models (`routing`)

Title "Engines and models". Lead: "How automatic routing uses the models you switched on. Every model stays selectable by hand."

- Head action: "Preview routes", which opens Route preview.
- Group "What these switches change": two or three visible lines that summarise the switches, with the full help behind "?".
- One block per engine, headed "<Engine>" ("optional engine" for OpenCode):
  - **Now**: the engine's state and its connections by name and state (read-only).
  - **Use <Engine>** switch. On: "Its models can be chosen and routed." Off: "Off: every <Engine> model is hidden from the selector, refused when chosen or pinned, and never routed. Connections and logins stay." Recommended: "On for every engine you pay for."
  - **Automatic routing may use <Engine>** switch. It is off for the last engine, with the reason "At least one engine must stay available to automatic routing". Off: "Its models stay selectable by hand; automatic routing, fallback and pins skip it." Recommended: "On, so workers can use both plans."
  - A model table with the columns Model, Automatic, Tiers, Preferred work and a disclosure, with one row for each model switched on under Models:
    - **Automatic**: a switch named "Pick <name> automatically". It is off with "Switched off under Models" when the model is not enabled, and it shows "Choose a tier." as its error when no tier is set.
    - **Tiers**: Light, Standard, Deep and Flagship toggles, in that order. Off with "Turn on Automatic first" when automatic is off.
    - **Preferred work**: a summary in words, shown in full on press or focus.
    - **Expanded details**: the "Now" line (automatic verdict and reason, with matching deny rules in monospace; no route ids); the checklist of the twelve work kinds, in two columns and one below 560 px, each with a help line that names its roles (legacy kinds shown as "<kind> (legacy, kept)"); "Prefer <name> when", a text area with a counter ("n of 500. The coordinator judges this. Retries ignore it."); and "Effort and policy for <name>", a link to that model in Routing › Model policy. For a model that routing excludes by default (OpenCode routes), "Allow <name> in automatic routing" writes an `auto_allow` token for that exact route; its effect is open point 6.
  - Empty texts: "No models: no connection for this engine.", "No models known yet: refresh the model lists under Models.", "No model of this engine is switched on (see Models)."
- **Other entries in this file**: deny or auto-allow tokens that match no listed model, each with Remove. Shown only when there are any.
- Strips and save bar as on Models: one shared routing draft.

### 4. Project ceiling (`project_ceiling`)

Title "Project ceiling". Lead: "Narrows the machine ceiling for one project. It is stored in that project and never in a profile." Recommended: "Only for projects that must stay on specific models or providers."

- **Machine only** (no project chosen): one state line, "Choose a project in the bar above to read or set its ceiling.", or "This machine has no projects." when none exists. No save bar.
- **Project chosen**: the notice "This project has no ceiling of its own. Saving creates one." when none is set; a table of Limit, Machine, Project and Effective, with the states "Starts from the machine ceiling", "No longer narrows" (warning) and "Must stay within the machine limit." (danger); "Remove project ceiling…" with a confirmation; and the save bar "Save project ceiling on <project>". A project ceiling may only narrow the machine limit. Its rows are attempts per task, connections, data recipients, failure classes, fallback budget, consent to move work to another provider, allowed routes and fallback target routes.

### 5. Routing (`routing`)

Title "Routing". Lead: "Who runs what. Pins are strict; leans and preferences only rank what is eligible." Head actions: "Preview routes" and "Files and sources". Groups, in order:

1. **Routing files** (read-only): "Rules in use" (User, Project or Environment routing file, with "(invalid)" when invalid); "Precedence": "Project, then environment, then user. Files never merge."; "Model policy version", with the migration strip when it is not 2; and "Saving to", the file for the current scope.
2. **Model policy**: one disclosure per model, grouped by engine, with an anchor for each model. Its summary is the name and tiers in words. Its fields: Tiers; Effort floor and Max effort (pickers over the observed levels, Neutral, and any stored value the model does not offer, labelled "Not offered by this model"); Preferred work (the same checklist as Engines and models); Prefer when; Effort ladder (three pickers, light models only); Also standard for (light models only); Request names (a tag list). A stored value the model does not offer stays visible as "Not offered by this model"; the page shows no separate notice for other stored data. Recommended: "Most owners only need Engines and models; set these for models that work badly at low effort or that you call by a short name."
3. **Roles and pins**: the role table, Role, Work kind, Tier and Pin, in the groups Writing, Read-only and Consultant, with an "All roles" row. Work kind and Tier are built in and locked. The Tier column reads the built-in ladder from the domain in words. "Pin…" or "Edit pin" is a visible line button that opens the pin editor with its strict-pin warning. Recommended: "No pins: automatic routing balances cost, quota and quality."
4. **Work kinds**: Work kind, Lean to engine, Preferred models and Light models also standard. Recommended: "Leave every lean on Built in."
5. **Quota windows** (open when windows exist): the per-engine quota editor, with the locked hot-band note.
6. **Deny list** (open when non-empty): the tag list with token validation, and the other entries.
7. **Auto-allow** (shown only when non-empty; its effect is open point 6) and **Legacy tiers** (shown only when present, with Migrate) sit inside the Deny list group, not as groups of their own.
8. **How the router decides**: locked rows for fallback order, effort by tier, rank order, hot band and the flagship rule. Route preview shows the same rows.

Save bar: the shared routing draft, labelled "Save routing rules on <machine or project>".

### 6. Profiles

Title "Profiles". Lead: the first two sentences of the profiles help. A table of saved profiles (name, description, stored sections, updated, matches, and actions), where a name is changed by clicking it. "Activate…" is a visible line button; Edit description, Detach and Delete are in the More menu. Empty state: "No saved profiles. Current settings still apply; use Save as new in the bar above to keep them." Note: "Profiles never hold project settings, MCP commands, arguments, environment or credentials." The environment-override note stays. This section acts at once and has no save bar.

### 7. MCP servers (`mcp_servers`)

Title "MCP servers". Lead: "External tools for agents on this machine, and who may use each one. Servers are per machine; a project cannot add its own."

- **List**: Server, Tools, Engines, Network, Checked and Enabled. Each row has a visible Edit button; Check tools, Duplicate and Remove (a danger item) are in More. Head action: "Add server".
- Tool counts are worded by audience, so overlapping grants never read as one total ("15 tools for writing workers, 11 for read-only roles, 8 for the coordinator"). Invalid, path-missing, unmanaged-settings and disabled servers carry their reason.
- The enabled switch is a draft edit under the save bar, as it is today. Paseo saves it at once; the save bar says so.
- **Add** takes three steps: Define, Connect and list tools, Tool access. Connecting starts the server in its sandbox and lists its tools; it calls none. The draft stays in the client until it is saved. A server with no granted tool cannot be saved, even when disabled. Listing tools from an unsaved draft calls no tool, and its failures show fixed reasons only. The last step offers "Save, keep off" and "Save and turn on".
- **Tool access matrix**: one row per tool, with the columns Coordinator, Read-only, Writing, Consultant (fixed to Never) and Always load (Claude only). Coordinator implies Read-only, which implies Writing. Removing Writing clears the higher levels, so every saved list is a subset of the list above it. New tools get no access by default. A filter row (All, Granted, New, Not offered and Warnings) and a search box narrow the matrix.
- Each tool shows one state: Granted; New (listed by the server, never granted by default); Not offered (granted but no longer listed, kept until removed); or Not checked (a granted tool that no check has listed, such as a name typed by hand).
- Bulk actions set the selected rows to Off, Writing only, Read-only too or Coordinator too. "Suggest from server" applies the server's hints as a labelled preview; it is never automatic.
- **Check as**, for one role (or the coordinator) and one engine, shows the access result the runtime would use.
- Environment values are never shown after saving; they can be kept, replaced or removed. Redacted arguments keep their stored value until the whole part is replaced.
- Consultant access is fixed to "Never", with its reason; it is not a setting.
- The edit pages (Connection, Tools, Sandbox) keep their fields, with "Back to MCP servers".

Open owner decisions, not in the current configuration schema: per-tool engine access (today set by duplicating a server per engine) and access per worker role. The prototype simulates tool listing, the route preview and the new runtime operations, and labels each as simulated; none is runtime behaviour.

### 8. Connections

The existing Connections page, rendered inside the orchestration shell so that it gets the profile bar. Groups: Engines: download and connect, polled every 3 seconds; the connections table; "Details of <name>" for the generated fields; and session limits, which are profile-bound. Every action is immediate: rename, max concurrent sessions, validate, remove, download, add connection and OpenCode enable. A line on the page names the models each engine offers and links "Models" to section 2.

### Secondary pages

- **Route preview** (read-only; from Routing and from Engines and models). Inputs: role, work kind, complexity, attempt, and, when the routing draft has changes, a choice of saved or draft rules. Outputs: the tier path, effort, the ranked routes with the decision that placed each, the excluded routes with their reasons, and the all-roles grid. A blocked result states its reason in words. The page states that live availability, retries and plan hints are known only at dispatch.
- **Files and sources** (read-only; from Routing, the Defaults notice and the invalid-routing and migration strips). It holds source precedence, path, existence, validity and revision; the full list of preserved keys; the environment overrides and `ssh-askpass.json`; and model-policy migration (preview, then apply with explicit consent). Exact paths and variable names appear only behind "Copy file location" or "Copy variable name".

### Shared rules

- **Drafts** are per surface, not per page. Models, Engines and models, Routing and Route preview share one routing draft and one save. Defaults holds one `user_defaults` draft for the coordinator, language, new tasks, exact default route and machine ceiling. The project ceiling and MCP servers each keep their own draft. Moving between sections that share a draft never prompts and never loses an edit. The save bar names what it covers, for example "4 unsaved changes in routing rules", and adds that running tasks keep their configuration.
- **Save bar**: Discard; "Go to the first" when a field has an error; and Save, which is off with the reason "Fix <n> fields" until the errors are fixed. An error opens its disclosure by itself.
- **Conflict**: a save over a surface that changed elsewhere writes nothing. The bar reads "Changed elsewhere since you opened it. Nothing was written." with "Discard mine" and "Review changes". The review lists changes made elsewhere, changes you made and collisions. "Reapply mine" is offered only after that review.
- **Unknown outcome**: "No answer. It may have been saved." with "Read again". Nothing is replayed.
- **Refused save**: the refusal shows in a note above the bar, and the draft is kept.
- **Saved**: "Saved on <where>. Not yet in profile <name>.", with "Save to profile" and, where a project file still wins, "Open source".
- **Offline**: every write and profile action is off with its reason, last-known values stay visible, a dirty draft is kept, and nothing is queued.
- **Built-in values** (role tiers, complexity shift, fallback order, effort by tier, hot band, rank order, the flagship rule and the work-kind table) are read-only, marked with a lock and "Built in". They are never controls.
- **Stored values** the page does not offer (a model no longer offered, or a server key the app does not manage) stay visible as stored, and no save removes them.
- **Absent values** are omitted. No dash or "None" stands in for them. Selecting anything changes no font weight, size or layout.
- **Advanced** blocks are closed by default and open by themselves when they hold a non-default value or an error.
- **Help**: every editable row shows its one-sentence description; a row with a recommendation shows "Recommended: …" beneath it; the longer help stays behind "?". The help text is in `orch-help.ts`.
- **Names**: labels are English only and never name the program. No route, connection or task identifier appears; pickers show names. Only the listed desktop [tooltips](#desktop-tooltip-inventory) are used, never native `title`.

### States

- **Offline or read-only**: the existing strip, "<machine> is offline, so settings are read-only. Last confirmed <time>. Nothing is queued." Every write control and profile action is off with that reason.
- **Empty**: one state line per panel, with its reason and the section that fixes it. No placeholder values.
- **Unsaved draft**: "<n> unsaved changes in <surface>. Running tasks keep their configuration." with Discard and Save.
- **Field errors**: Discard, "Go to the first" and Save, as above.
- **Saved**, **Conflict**, **Unknown outcome** and **Refused save**: as under Shared rules.
- **Invalid routing, migration, environment override, shadowed or seeded project file**: the source strips on sections 2, 3 and 5, and the needs-attention list on Defaults.
- **Profile interrupted, unavailable or partial**: the profile bar.
- **Catalogue failure**: the Models row "List could not be read; the previous one is kept", and a strip for each connection.
- **Loading**: open point 2.

### Responsive and keyboard

- Reading width: Defaults, Project ceiling, Profiles and Connections use the narrow column (about 860 px). Models, Engines and models, Routing, MCP servers and Route preview use the wide column (about 1120 px).
- From 900 px: the context bar shows its labels, rows are two columns, and the engine table shows all four columns. From 720 to 900 px: the context bar hides its caption words but keeps the Project and Profile pickers, and the tier toggles wrap inside their cell.
- Below 720 px: the Section choice appears in the context bar; the profile bar takes two lines; each row puts its control under its description; engine model rows become labelled rows below 560 px of table width; the preferred-work checklist is one column; and the MCP tool matrix keeps its own horizontal scroller.
- No page scrolls sideways. On the phone shell the same eight items sit in the drawer's settings navigation.
- Keyboard: sidebar sub-items are tabs with `aria-selected`, and activating one moves focus to the page title. Tab order follows the reading order: context bar, profile bar, strips, page actions, groups top to bottom, then the save bar. Disclosures and advanced blocks expose `aria-expanded`. Tier toggles are individual stops with `aria-pressed`. Popovers close on Escape and return focus to their trigger. "Go to the first" focuses the first error inside its opened disclosure.
- Icon-only controls have accessible names, such as "Details of <model>" and "More for <name>".

### Wording

The section, page and group names are the labels above. Recommendations use the prefix "Recommended: " and a sentence-case text taken from `orch-help.ts`. Where the Paseo wording is clearer it is reused. The renamed labels are "Lean to engine" (was "Engine lean") and "Light models also standard" (was "Stretch").

### Removed on purpose

- **Overview summary rows and the Source row.** They restated the other pages. Each section shows its state where it is edited, and the routing source is shown under Routing › Routing files. The needs-attention list is kept and moves to the top of Defaults.
- **Free-text identifiers** (Paseo's route id, connection id and policy engine and model fields). Pickers over enabled models and connections by name replace them, as the current Workspace pages already do. A stored value that a picker does not offer stays visible as "No longer offered".
- **"Restrict automatic routing to engines" list.** The per-engine switch "Automatic routing may use <Engine>" replaces it, and at least one engine stays on.

### Acceptance

- The sidebar lists exactly the eight sections above, in that order, and nothing else. The bare Orchestration route opens Defaults, and each existing deep link lands on the section or group named above.
- Route preview and Files and sources are reachable from the links named above, each with a Back link, and are not in the sidebar.
- Every field in the CW-U07 checklist is reachable on the section that CW-U07 names; a test walks the list.
- The profile bar appears on every section and secondary page, and offers the actions listed above.
- The project picker appears only on sections 2 to 5.
- Every editable row shows a visible description, and every row with a recommendation shows it.
- Tiers show full words in the order Light, Standard, Deep, Flagship. No single-letter tier control remains.
- Models, Engines and models, Routing and Route preview share one routing draft, and their edits are never lost when moving between them.
- A save over a surface that changed elsewhere writes nothing until the owner has reviewed the changes.
- Offline, every write and profile action is off with its reason, and nothing is queued.
- No interface text shows a route, connection or task identifier, the program name or a dash for an absent value. Use the shared [desktop tooltip](#desktop-tooltip-inventory), not native `title`.
- At about 390, 820 and 1440 px wide, in light and dark, no page scrolls sideways and labels stay on one line.
- Every section can be operated from the keyboard alone.
- Evidence: the rendered statements in this section need screenshots of every section, Route preview and Files and sources at those widths, in light and dark. None is recorded yet.

### Open points

These are not decided by this section. Each stays open until the owner or a later review decides it.

1. **Help language.** [CW-U07](backlog/CW-U07.md) lists English, Russian or both (default both) under App > Appearance. The interface rule above says labels and hints are English only, and the settings help is English only today. This section keeps English. The owner must decide.
2. **Loading state.** The current orchestration pages render from the machine snapshot and have no loading state of their own. This section asks for one state line, "Reading this machine's settings…", rather than empty fields. Whether a snapshot can be absent while a section is open is not established.
3. **Whole-ceiling switch.** Paseo's "Set an owner ceiling" switch is not offered. A Workspace save always writes the ceiling as set, and the domain has no unset state, so the machine ceiling cannot be removed as a whole. Each limit has its own Restrict control instead. A whole-ceiling switch needs a domain and service change.
4. **Reload of profile state.** An explicit reload is wired only in the demo. In real mode the Unavailable state needs either a working Reload or a statement that the next machine read retries it.
5. **Profile summaries.** The native read model computes a per-profile summary (providers, selected models, default model, language, enabled MCP servers and whether a ceiling is set), while the Workspace `Profile` type carries only the stored sections. This section does not add those values to the Profiles table. The owner decides whether they appear.
6. **`auto_allow`.** The runtime's routing validator describes `auto_allow` as lifting the OpenCode exclusion only for an exact `opencode/<provider>/<model>` route, and the catalogue reports `auto_allow` tokens for each model. The current Workspace help and [CW-U07](backlog/CW-U07.md) say it has no effect under model policy version 2. These statements conflict. The routing decision code has not been traced, so the per-model switch stays in the Engines and models row and the note stays open until the decision code confirms the effect.
7. **Deliberate deviations from Paseo**: Coordinator comes before the exact default route, because Paseo's own help advises leaving the exact route empty; the MCP enabled switch stays a draft edit rather than an immediate save; and the per-engine switch replaces Paseo's ordered engine list.
8. **Route ids.** [CW-U07](backlog/CW-U07.md) lists the route id among the per-connection route facts, while the interface rule keeps identifiers out of the interface. This section omits route ids. The owner must decide whether a route id is ever shown.
9. **Rendering.** No screenshot backs the layout statements above. They are the specification until captures exist.

## Application updates (desktop)

- Settings › Updates shows the installed version, the time of the last check, Check now and, when a newer release exists, Install version <x>…. Release notes appear as plain text in a scrollable area.
- Each state is shown in words: Up to date; Checking…; Update available (version and release date); Downloading (bytes received); Verifying signature and checksum; Verification failed · nothing installed; Waiting for administrator approval; Installing; Restart required; Install failed · previous version still installed; Rolled back to version <x>; Check failed · GitHub unreachable, last check <time>.
- Install does not start while a task on this machine has a running worker. The dialog names the blocking task and offers Stop first or Cancel. The update never stops a worker by itself.
- Install needs owner confirmation and administrator approval. Cancelling either leaves the installed version unchanged.
- Each machine shows and updates its own installation. The local app does not install on a remote machine.

## Project board and card actions (desktop)

- Card actions (Open, Preview, the column's primary action and the more menu) appear on hover or keyboard focus in a strip outside the card content. They never cover the card's title, branch or fact line ([CW-R37](requirements.md), [CW-U01](backlog/CW-U01.md)).
- In a Hybrid or Automatic sprint, the queue's rule sits behind a `?` button on the board, closed by default, and is not a line above it. It states the run mode's description: for Hybrid, "The queue runs one task at a time in dependency order. You confirm each merge."; for Automatic, "Tasks go to Done on their own in the current branch. You answer work questions and permission requests, and review blocked results.", followed by "Finishing on a different target branch still needs confirmation." A Manual sprint shows no such line.
- New sprint asks for the run mode with three labelled choices, each with its description: Manual ("You start each ready task. Starting one never starts its neighbours."), Hybrid and Automatic. The sprint's Finishing line reads "Automatic on the current branch; another target needs confirmation" in an Automatic sprint and "Each task waits for your confirmation" otherwise. See [sprint run modes](../cortex-workspace/local-task-execution.md#sprint-run-modes) for what each mode does.
- A sprint's board, Planning and Summary say why the sprint is not moving, with an Open task link to the task that holds it: what Run sprint would refuse in an automatic sprint, or the dependency the first unstarted task waits for in a manual one.
- A sprint whose recorded tasks could not all be loaded says how many are unavailable and offers Retry, which only reads again. It is never shown as empty, and Archive stays off until every recorded task is loaded and done. An empty created sprint, and a sprint's own board, offer New task with that sprint already selected; Ctrl+N from a sprint's pages does the same.

## Code file toolbar

Above an open file, one **File actions** button opens three items: **Go to file**, **Open in VS Code** (or **Open in default app**) and **Show in file manager**. The last two explain when they are unavailable. On a local Linux desktop, the file manager selects the open file if supported; otherwise it opens the containing folder. Save and the view switches stay where they are. See [Code file actions](ide-and-git.md#code-file-actions).

## Title band Open and Git buttons (desktop)

- At the right of a task's or project page's title band, two split buttons: Open (the editor, the Code tab or the terminal panel, whichever the owner last used, with the rest in its menu) and Git (one next action for the state, with every Git and task action in its menu in a fixed order, each on or off with its reason). The state table, the menu and the commit dialog are in [IDE and Git](ide-and-git.md#title-band-actions).
- The Git button's main action replaces the title band's former Merge…, Review and merge… and Commit to <target>… buttons and keeps their names. Review changes, Close task… and Close as is… stay where they were; Start, Stop and Resume are the task composer's action button; the task menu keeps its entries.
- Beside Git, ↑ and ↓ count the commits ahead of and behind the upstream and a dot counts uncommitted files. On a narrower band the two buttons show icons only. On a narrow band (the smallest window, or a wide sidebar) the counts and the state chip give way (the task's strip repeats the state), Open folds away (the task and project menus keep Open in VS Code, and Code is a tab), a stopped task's Close task… moves into the task menu, and Git is one menu button unless its main action is the task's own next step. The buttons keep their size and place whether they are on or off.
- A refusal opens beside the button in the service's words, with Retry where repeating it makes sense.
- A pull stopped at a conflict comes first in the Git button: **Resolve conflicts** while files remain in conflict, then **Continue merge** or **Continue rebase**. **Abort merge…** or **Abort rebase…** is in the Git menu and on the Git page.
- Diverged histories, when no pull is stopped, no file is uncommitted and no task result is waiting to merge: the Git button's main action is **Pull…**, which opens a dialog where the owner chooses Merge or Rebase before anything is pulled. In the Git menu, Push and Pull and push are off there with their reasons; the Git page's Push button is not disabled by divergence. A worktree task on a branch other than its target has **Update from <target>…** in the Git menu and in the Worktrees page's worktree menu, where it is shown off with its reason when the machine does not offer it; it opens its own dialog. See [IDE and Git](ide-and-git.md#title-band-actions).

## Ready to merge and conflicts (desktop)

- Every finished task, including tasks of a Hybrid or Automatic sprint, reaches Ready to merge with its target branch. The target defaults to the repository's Git default branch and can be changed for a task or sprint. A local squash merge or a PR runs only after owner confirmation; there is no automatic merge ([CW-G07](backlog/CW-G07.md)). In a Hybrid sprint, a task that passes its checks moves to Ready to merge by itself, with no separate Accept step; the owner confirms there. In an Automatic sprint the task continues to Done on its own: it shows "Accepted; awaiting automatic finish" only briefly, and the owner is asked only when its different target branch needs confirmation.
- Ready to merge always has one primary next action. A task that is not accepted yet offers "Review and merge…": its dialog shows the exact result version, checks and target, and its one confirmation records acceptance of that version first and then merges or creates the PR. A refused merge keeps the dialog open and the acceptance on record; nothing is resent by itself. An accepted task offers "Merge…" and "Create PR…".
- An accepted task whose pull request was created shows a "Pull request created into <target>" row with the recorded address and a **View PR** button, which opens the address in the browser where `/usr/bin/xdg-open` or `/usr/bin/gio` is installed. Without a recorded address the row says "GitHub CLI did not report this pull request's address, so it was not recorded. Find it on GitHub." and has no button. A worktree task on a branch other than its target that is behind it gets an **Update from <target>…** button in the Finish task dialog where the machine offers the update; otherwise the dialog shows no such button. See [IDE and Git](ide-and-git.md#title-band-actions).
- A project folder without Git has nothing to merge. Ready to merge offers "Close as is…" and "Set up Git…" instead of Merge and PR. Closing a result that is already accepted, or promoted after passed checks, keeps it on record as checked, without the Not verified warning. Setting up Git opens the project's Git page, where creating an empty repository asks first. New task offers no worktree or target branch for such a folder.
- Loading covers the whole action: a multi-step action such as staging every change and then committing, or accepting and then merging, keeps the pressed button loading and closed to a second click or shortcut until its last step ends. The label stays readable and a thin bar runs along the button's lower edge.
- Before Ready to merge, a check the system skipped runs automatically. A failed or not-run check offers a forced re-run. A result with an unpassed check can be accepted only when that check cannot complete for a technical reason, and only with an explicit confirmation ([CW-R40](requirements.md), [CW-U04](backlog/CW-U04.md)).
- The review's comment bar appears only in a project with Git. A comment needs a line of a diff, so a project without Git hides the bar rather than showing it disabled.
- A conflict shows "Conflict with <branch>" in words and an icon on the card and the project row. The Changes tab lists conflicts first.
- For a saved source merge in live mode, **Hand to the app…** opens an owner confirmation. A recorded handoff shows **Stop resolution**; an unconfirmed outcome offers **Inspect merge state** without resending. **Finish merge** and **Abort merge** appear only where the saved Git state and writer status permit them.
- A saved merge from an earlier version without a verified identity shows **Clear saved merge…** instead of the normal merge controls. Its confirmation tells the owner to finish or abort in the original checkout using Git first. A refusal stays in the dialog; clearing leaves Git unchanged and earlier checks do not qualify the checkout.
- After either manual or Cortex resolution, the card links to the diff and offers **Request fresh checks** before separately confirmed integration. See [Integration and conflicts](local-task-execution.md#integration-and-conflicts).

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
