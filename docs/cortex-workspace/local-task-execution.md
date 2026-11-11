# Local task execution

**Status: implemented source slice, not full product or installed-package qualification.**
The local wire version is 1.8. See [protocol](protocol.md) and
[IDE and Git](ide-and-git.md) for the wider contract.

For update, native/API-key connection, voice-settings and command-control behavior,
see [desktop maintenance and controls](maintenance-and-controls.md).

## Recorded data and pending questions

Machine connectivity and task process confirmation are separate. An unconfirmed task on a connected machine shows one notice with one icon, explains that the machine is connected but cannot confirm whether the task is running or stopped, and offers **Re-check state**. The composer stays read-only and points to that control, not a fictitious reconnect requirement. Re-check rereads the existing projections; a still-unknown reply stays blocked and points to Machine settings → Diagnostics. A failed read keeps the task and a retry explanation. No task is resumed, stopped or replayed by this read.

Coordinator messages and selected tool steps use the retained public `history.page`
sequence, not the coordinator's one-line status note. Available history rows
also carry worker-shaped `activity` (message, note, tool, command or edit) when
that convenience copy fits the page. Otherwise the readable text remains and
the helper reconstructs the activity. Long activity previews are explicitly clipped;
the public row retains its text within the existing wire limits.
Pages bind the project, task, session and page size, freeze their high-water mark,
and survive window reopen and helper restart. Runtime bookkeeping and thoughts
are hidden without deleting source history; hidden-only pages still advance.
Oversized or missing display content remains explicitly unavailable.
The window reuses complete conversation histories only while project, task version and execution identity match; transactional history invalidations cause a fresh read. Page construction validates each row and the final page without revalidating every growing prefix. Working indicators retain their icon and status text without continuous animation; slow Git completion publishes updated facts rather than triggering another full task read.

The machine `workers` helper accepts `workerId: "coordinator:<taskId>"` with that
task in `taskIds`, returning a coordinator detail with the same activity and
continuation fields as a worker. It reads the same public history projection.
Opening a worker activity view automatically reads each available continuation page, without a Load older control. A failed page keeps loaded rows and offers Retry; new live-tail rows remain retained for the same attempt. Existing keyed rows, collapsed tool groups and scroll anchoring are reused (the activity stream has no virtualizer). Project-folder checkout labels are separate from branch names; the project title chip uses the observed open branch rather than the Git default.
Ordinary worker lists stay worker-only. They retain each work-graph retry;
the latest keeps the assignment identity, and older attempts use their own
selection identities. `routeAttempts` additionally records each audited engine
fallback within a work attempt, oldest first.

Task list/detail `coordinatorRoute` and worker list/detail `route` describe recorded
confirmed runtime settings, separately from editable settings and machine defaults.
Worker routes resolve the provisioned engine attempt, not the work-graph attempt;
native confirmation takes precedence over dispatch intent, including narrowed or
unavailable effort. Coordinator routes require confirmed active configuration;
a pending/indeterminate application or mismatched current attempt stays unknown.
Missing confirmation gives a null route, and missing effort gives an empty string,
never an inherited guess. Recorded model names are preferred; a recorded model
without a name is explicitly marked as an alias. Transport, connection and
configuration identities are not display labels.

Task and project freshness uses recorded task/question/history timestamps, never the time a poll completed. Task freshness uses that task rather than an unrelated project refresh. Elapsed durations use one whole-second formatter: seconds, minutes plus seconds, then hours/minutes/seconds. An unreported time is omitted.

A worker can ask while the runtime remains EXECUTING for other workers. Its actionable pending question still appears in the task conversation, Inbox and sidebar; it does not imply that the entire runtime is paused. Answers held for a question batch may bind to a newer display version only when the question text, options, owner, requirement and permission identities are unchanged; a changed question clears the held batch instead of submitting an old decision. The native window can notify outside the open task/sprint; see [notification behavior](maintenance-and-controls.md). All work lists sprint planning with an Open planning link before it has materialized tasks, rather than implying there is no work.

Weekly Claude/Codex usage comes from the runtime quota reader used by route selection. Packaged Claude discovery can use the verified SDK CLI when no global `claude` is on PATH. Claude uses named weekly windows; Codex uses the reported 10080-minute duration in either primary or secondary, never the slot name alone. Missing Codex duration stays unknown. Multiple scopes/connections use the highest reported percentage, not their sum or the last entry. Unknown usage/observation time remains unknown; OpenCode and five-hour windows are omitted. These source and fixture checks do not qualify live account availability.

## Editing saved tasks and closing sprints

A task that has never started offers **Edit task…** in its title bar and board/list menu. The shared composer edits the original description and attachments, with title, model/effort/access/language and checkout choices. Saved files load before Save is enabled; a failed read offers Retry and never silently replaces them with an empty list. Save changes starts nothing. The original task version is checked transactionally against Start: an intervening edit refuses a stale save, and any execution identity (including an unknown or failed start) permanently makes the saved request read-only. Live messages and supported session-option changes remain separate actions.

**New task** on a created sprint adds a draft even while its queue is active. It inherits the sprint checkout choice. New members append to the existing order; existing tasks and dependencies are not rewritten. Editing a pending member changes only its request, not the original planning document or dependency graph. Adding after queue completion leaves the queue paused until the owner continues it. If saving succeeds but linking is unconfirmed, the UI opens the saved task and explains the partial effect rather than creating another copy.

**Close sprint…** is visible beside the sprint actions (in the sprint's actions menu when the title band is narrow) and asks for confirmation. It archives the sprint container and stops its automatic queue and planner. It does **not** cancel, accept, merge, close, archive or delete member tasks or discard files: unfinished tasks remain in the project outside active sprint grouping, and tasks already running continue. Individual task closure and unmerged-work/archive decisions keep their existing confirmations. The closed sprint disappears from active lists, sidebar, search and New task choices, and appears under **Archive → Closed sprints**, with its retained Summary. Closed sprint controls cannot restart its queue or accept new members. This is not permanent deletion or release qualification.

## Choices and checkout

`tasks.create` saves the description and optional per-task model route, effort,
access mode, message language and checkout kind. Saving does not start work.
Explicit Start is task-local: another Workspace execution, pending/unknown start,
or ordinary ACP task in the project never refuses it. A task with an existing
execution still returns `busy`; refresh to see that execution instead of starting
it again. Request replay returns the original receipt without redispatch. A
crashed start remains unknown for that task only and is never replayed.
The reverse direction is independent too: pending, unknown or running Workspace
starts do not reserve the project against a new ordinary ACP first prompt.
Source-event replay still recovers the original ACP task without creating another.
Each execution has its own ACP session and cancellation target. Project-folder
tasks still share files; start admission is not a filesystem isolation boundary.
Sprint queues retain their dependency checks and pause rules. Manual
`tasks.start` does not itself check sprint dependencies; dependency-aware
board/sprint controls are presentation guards, not server enforcement.

Selections are session-local: they do not rewrite machine defaults. Auto language
does not inherit a previously chosen explicit language. Composer changes require
an idle owning session; busy or unavailable sessions refuse them. A multi-setting failure explicitly names acknowledged and unconfirmed fields; it never reports a partial update as a successful whole. Refresh before retrying an unconfirmed step.

The Settings machine catalogue retains every connection-selectable route identity, including disabled models with their existing routing switch off and different connections for the same model, and cached observed
efforts for the chosen model. Unsupported or withdrawn routes are not silently
replaced. A failed dispatch remains unknown and is not replayed automatically.

A folder without Git runs in the project folder. On start, a saved worktree choice becomes the project-folder checkout (internal kind `main`) because there is no repository from which to create a worktree; no repository is created implicitly. Tasks, results, review and acceptance do not depend on Git. Code can read/save project files, while Changes explains that a Git diff is unavailable. Accepted work reaches Ready to merge; the owner can close it as-is without Git, or explicitly initialize Git before using Git integration. Closing as-is a result that already reached Ready to merge through acceptance or the automatic passed-checks promotion keeps it checked (`notVerifiedByCortex` stays false); closing any other result as-is still records the lasting Not verified warning.

A separate worktree is prepared on explicit start under the machine's configured
worktree root, grouped by repository identity. New task branches and directory names
use `cw/<description-slug>-<12-character-suffix>` and the matching leaf name, not
the task's transport identifier, based on the project's open HEAD. Existing saved
branches and checkout paths remain unchanged, including legacy names and after a
default-root change. The default root is
`$XDG_DATA_HOME/cortex-workspace/worktrees`, or
`~/.local/share/cortex-workspace/worktrees` when the service has no XDG data override. The dev service sets XDG data to `<channel-state>/data`, so its default is `<channel-state>/data/cortex-workspace/worktrees`. Overview reports the absolute default separately from the saved root; the real reset control and example use that value. Changing or resetting the root never moves existing worktrees. Settings reject roots inside Git checkouts or registered non-Git projects, and locations occupied by files or lacking write/search permission. A writable external folder such as `/mnt/Big_Storage/.cortex/worktrees` is allowed; the `.cortex` name alone does not make it a project store. A refused save retains the draft and a persistent readable reason beside Save. The actual linked checkout is the ACP session cwd and permanent runtime
task workspace; it shares the main repository's **one store per channel**: `.cortex/cortex.sqlite3` for release/default ACP, `.cortex-dev/cortex.sqlite3` for Workspace Dev. Dev never opens the release store; see [channel ownership](backlog/CW-S12.md).
Task Changes reads that task's baseline-to-current diff, including its untracked
files, not another checkout's diff. Project-folder tasks share the owner's files;
pre-existing uncommitted work therefore belongs to that visible checkout.

Activation adds the exact Workspace helper paths to local Git excludes, without
editing tracked ignore rules. File projections and app staging exclude them too;
a commit refuses helper data already staged by another client. Separate-branch integration enumerates changed/untracked user paths and stages them literally, without adding or pathspec-matching ignored helper folders. Failed staging or commit restores the prior index selection; user file contents remain. Git identity/hook problems show recovery guidance instead of raw Git advice. Project-folder tasks still share their visible user changes; this is not authorship attribution.

## Local settings and Git drafts

Machine configuration is reread after activation and on machine-fact refreshes, including background first-run setup. Dirty settings retain their original revision token even if a newer read arrives. A structured revision conflict keeps the draft and exposes Review changes and explicit Reapply mine against the refreshed revision. Unknown save outcomes remain unknown and are never automatically retried. A refused defaults update does not write Workspace-only mode/checkout values.

Git commit-message drafts update reactive checkout state synchronously, survive fact refreshes, and clear only after a successful matching commit. A newer draft entered while Commit is in flight is kept. Empty messages remain refused. Commit is local and requires no remote: an existing staged selection is kept, or, when the index is empty, the listed non-helper changes are staged and committed. Amend uses the same visible message and explicit confirmation. App-generated messages use English templates for task commits, squash/PR defaults, Git Generate, automatic merge updates, conflict completion and revert commits, regardless of task language. Owner-written messages remain editable and are not translated. Revert requires an empty index before applying its inverse and making the English-message commit; a failure leaves Git’s visible pending work for manual recovery. Push refuses an unknown remote; Pull refuses an absent tracking upstream with setup guidance. When a remote exists, Pull is disabled with a reason when no upstream exists or the last fetched state has no incoming commits; Fetch checks for newer remote state. Local-only repositories omit Fetch, Pull, Push, PR creation and push-status wording rather than showing dead network controls. Local commits, review, squash merge and confirmed closure still work.

## Integration and conflicts

Result cards retain headings, paragraphs, lists, quotes and code instead of collapsing
the runtime summary into one line. Only nonzero check outcomes have counters, and
an all-zero open-check record does not warn that checks remain open. Work awaiting
owner merge confirmation stays visibly actionable on the board, list and sidebar.

A complete, current runtime verification with valid workspace binding promotes
work to Ready to merge. Owner acceptance and integration remain separate.
New untracked UTF-8 text files appear as full additions within the shared diff-size budget; symlinks and binary or oversized content are not read as text. A conversational no-file follow-up retains the implementation summary when the observed source digest is unchanged; its reply remains in conversation, and new runtime checks still govern acceptance. A recorded missing-command/tool blocker can survive session-owner renewal only for the exact same check and source, never a policy refusal, stale attempt or pass. Acceptance observes the checkout again. `results.get.acceptanceAvailable` reflects current evidence rather than an always-false placeholder. `product.read.tasks[taskId].reviewActions` supplies Accept/Request changes availability, a readable blocking reason and the unblocking action (`wait`, `request_changes`, `rerun_checks` or `refresh`), plus current result identity/version/digest and whether technical blockers require explicit acknowledgement. The mutation rechecks these facts and the result version. Request changes uses the existing owning-execution prompt control; it does not require acceptance or Git. For a settled completed result, the service restores its exact durable session automatically before capturing owner guidance, including after restart; a live stale or differently bound session still refuses. An informational answer with no checks can be accepted when its runtime evidence permits it; this never certifies code for merging.

Changed files, stale results, unavailable
evidence and unsuccessful work verification refuse integration.

The owner confirms squash merge into a local target branch. Dirty target
checkouts are refused; nothing is auto-stashed or reset. A target that predates
the task baseline is refused rather than silently including earlier branch work.
Updating from a newer target invalidates acceptance and requires fresh checks and
review before integration. Conflicts stay in the task checkout and do not modify
the target. The target control explains why a branch predating the baseline is incompatible: squash would include earlier unrelated branch work. The UI can choose an entire displayed text-file version, stage it and
finish the source merge, or send resolution guidance to the owning Cortex
execution. Manual completion requests fresh checks. These are requests, not proof
that a model resolved a conflict. Add/delete, symlink, binary and mode conflicts
require manual Git/editor resolution rather than destructive text replacement. Oversized or non-text sides show bounded metadata and instructions to resolve contents/type/mode, stage with Git add or remove with Git rm, and finish. Finish checks the complete Git index, including paths omitted by the bounded display. The previous result identity/version is retained so it cannot certify the changed source.

PR creation uses the owner's installed `gh`, never copied credentials. Missing
`gh` is stated on the control. The branch must already have an upstream and no
unpushed commits: creating a PR never hides a push. A retained attempt prevents
blind retries after an ambiguous remote response. Confirm the outcome in GitHub
before another attempt. Real GitHub creation and paid-engine resolution are not
qualified by the no-paid fixtures.

Clean worktree removal uses Git without force and retains the branch. Dirty trees
are never forcibly removed. After a confirmed squash, optional immediate cleanup removes only a clean, separate task worktree and retains its branch. Integration is recorded first; a failed cleanup is recorded as retained, not as a failed merge. Archive purge recognizes already removed worktrees. Removing a merged checkout can make its old verification evidence unavailable; the scheduler preserves Done and the recorded integration instead of reopening the task. Only explicit reopening changes that owner/integration state.

Acceptance binds the runtime-evidence event version, not the execution display version: late transcript/progress writes cannot stale an unchanged result. New runtime evidence and changed source still invalidate old decisions. The UI uses current service review actions and source-drift warnings rather than inferring availability from check counts. An unsuccessful or unconfirmed Accept keeps its reason and Refresh result action beside the result after the transient toast closes. A paused queue identifies the blocking task and its review reason; review it, then use Continue queue.

Planning failures expose Retry planning and retain up to eight bounded non-content diagnostic entries per sprint: attempt, engine stage, stop reason, event/text counts, elapsed time, cancellation/deadline flags and exception class. They never retain prompts, owner answers, model output or raw error text. An old untraced stop remains unexplained; this trace is not proof that its cause was repaired.

### Already in the branch and external completion

Ready to merge reads two independent facts. **Already in <branch>** means the task’s recorded content landed in the local target branch (or its separate-branch patch is contained there, including squash/cherry-picked content); it does not mean that branch was pushed. This view replaces merge/PR and acceptance controls with **Close task…**. The owner confirms closure; the app never closes automatically and records no merge or acceptance. Confirmation checks containment again. A changed target refuses closure and asks for refresh.

For separate branches, containment compares the saved task baseline with current tracked and untracked task content, excludes private helper paths, and reverse-checks the binary-capable patch against the local target in disposable Git index/object storage. The real index, objects and working files are unchanged. Missing baselines/worktrees, conflicts, unreadable or oversized content and failed patch comparisons never assert separate-branch containment. This context-sensitive comparison remains conservative for later overlapping edits.

When checkout and target are the same branch, there is no merge, squash, PR or same-branch from/to flow. The service reads the task’s successful channel-bound file-write receipts, not the whole shared checkout. Exact recorded content in the target or its bounded local ancestry counts as landed, even when another task later edits that file. A known Git baseline bounds that history; without one, historical matches must not predate the write receipt. Missing/incomplete receipts, unsupported deletions or links, content beyond the lookup budget and ambiguous edits remain unknown, not a positive assertion. Older tasks are evaluated on refresh with the evidence already retained; no rerun or data repair is needed.

Pending recorded files offer **Commit to <branch>…**, with owner confirmation and the existing acceptance/evidence gates. Only those exact files are staged and committed; another staged file is preserved. A different staged version of a task file, changed content, symlink path or unrecorded mode change refuses this operation. The UI lists pending paths and warns when other uncommitted files share the checkout. No reset, stash or push occurs. A successful local commit is labelled Committed, not Merged. Unknown attribution explains its limit and offers **Close as is…** or **Done outside the app** after finishing in Git, rather than a meaningless merge. A folder without Git keeps Close as is and Set up Git.

When a remote exists, a separate line reports pushed/unpushed from the target's last-fetched tracking reference, or unknown if none is available. It never fetches or pushes and never blocks closure. With no remote, no push-status line is shown.

A board card can be dragged only to **Done**. Dropping elsewhere leaves it unchanged with a calm hint. Done opens **Done outside the app**; nothing changes until the owner confirms. Running/stopping/unknown tasks must be stopped and confirmed first; an active merge must be aborted. The service rechecks these conditions, persists `closedAs=external` and Done, clears active acceptance/integration labels, and retains files and historical evidence without inventing checks. A fresh window reads the same closure. Existing smooth card animation remains in use.

An item the owner closed **as is**, like an externally finished item, settles sprint dependencies and is never started again by the queue. Its Not verified warning remains; closure does not invent successful checks or a merge. An already-paused queue can recover through **Continue queue**, without repairing saved data; the owner decision does not silently undo a pause. Cancellation still blocks the queue. A live or unknown execution must settle before the queue can start a dependent task.

Shared styles give buttons, links, tabs, menus, cards, selectable rows and toggles a pointer; disabled/busy controls use the default cursor, while text inputs retain a text cursor. Cursor tests check the real shared cascade and inventory custom JSX click targets.

## Refresh responsiveness

Task refreshes keep their coherent runtime/product placement but do not wait indefinitely for repository details or machine discovery. Repository inspections have a 50 ms foreground allowance and otherwise finish separately, with one in-flight inspection per project and a short reuse interval. Review changes stays unavailable with an explanation until repository details arrive. Explicit Git/file/task effects invalidate that reuse; the service still rechecks effect-specific evidence before mutation. Machine discovery and disk accounting finish independently of the action that requested a task refresh.

The service reserves separate bounded helper lanes for task reads, expensive inspections and owner actions. Reply logging uses a separate lock from product writes. Closed tasks do not reproject old result verification during product reads, and unrelated result projections are excluded from the product writer critical section. Known transient read failures keep the last coherent view and retry quietly twice; persistent failures and malformed facts remain visible. Mutations and ambiguous outcomes are never automatically replayed.

## Archive and service-owned work

On activation, a saved materialized sprint whose members are missing from the selected channel store can restore those members as drafts. The saved plan version, original task identities, dependency order and task overlays must match exactly. Existing rows and explicitly purged members are not replaced. The queue pauses before restoration and explains that previous execution history is unavailable in this channel; nothing starts automatically, and no old execution/result is inferred or read from another channel. An ambiguous or missing plan stays paused for attention.

Retracted worker checks (withdrawn probes or invocation errors) are rederived from the persisted result, exact provisioning and command receipts, not the reviewer-only `current_outcome` field. Valid retractions are omitted from the active result checks and worker overview counts, as in the runtime's own completion gate. They neither block Accept/Ready to merge nor count as passes. Other failures, missing receipts and current runtime verification requirements remain blocking.

Sprint creation first checks that the chosen model exists in the current selectable catalogue. A missing/unavailable model refuses with a connect/select-model explanation, keeps the description in the form and creates no sprint or planning task. A preset name alone is not a connected model. Sprint creation offers the same **Where it works** choice as a single task: **Project folder** (the currently open local branch) or **Separate worktree**. The machine default fills the form until the owner chooses; unsupported worktrees and non-Git folders use Project folder with an explanation. Creation saves its chosen checkout kind, route and target before starting description-only planning. Planning and Summary show the saved checkout choice. Every task materialized from the plan inherits the sprint checkout kind, overriding per-item plan hints; worktree mode uses the existing per-task worktree preparation on explicit start, not one shared sprint checkout. Manual and automatic starts use those same saved task options. No checkout is created during planning, no existing checkout moves, and machine defaults are not rewritten. Older sprints without a saved choice keep their per-item/default Project folder behavior. The choice is fixed for that sprint; task execution/recovery semantics are unchanged. The service uses a separate bounded read-only/plan-mode auxiliary session in an empty temporary directory, with no project tools or runtime task. It returns a validated dependency plan or an owner question; it must not implement the description or claim project inspection. The planner must ask the owner, not decide itself, when the description explicitly requires an owner question. For a multi-item plan, the planner supplies each task's self-contained scope, acceptance criteria and relevant owner answers in plain text; only minimal shared context belongs there, never sibling work or serialized conversation records. An unanswered owner-question requirement belongs only to its own item. The service does not append the whole sprint to every item. A sole planned task retains the original owner requirements and plain-text follow-ups as a safeguard against a shortened summary; oversized combined descriptions refuse rather than truncate. Scope decomposition remains model-generated, not a filesystem permission boundary. Create tasks materializes the plan as unstarted user tasks. Only Run sprint or an explicit task Start begins execution. The auxiliary turn explicitly requests the task-plan JSON as well as supplying session instructions. Invalid prose, task/dependency validation errors, incomplete output and engine-stage failures produce a service-owned readable explanation beside Resume planning, outside the scrollable conversation; raw model output and engine errors are not displayed. Resume clears the current failure notice while retaining conversation history. Invalid output or interrupted planning never produces a ready state with a null plan. Cancellation fences late output and closes the auxiliary session; older hidden planning runs are stopped through their execution control. Materialized tasks inherit its settings and target; changing a task target makes that target explicit. Sprint-target changes propagate to inherited task targets, not explicit overrides. Once tasks are materialized, change their routes individually rather than silently rewriting them.

The activated service child advances automatic sprints without a window.
It reads bounded runtime projections, pauses for questions, unknown outcomes,
conflicts or missing/unsuccessful checks, and uses a stable explicit start identity.
A question pause records the waiting execution identities and clears when those
same executions leave their answer wait. An explicit owner pause, a failed check,
a conflict or unknown outcome does not self-resume.
A lost active session after service restart remains unknown, not silently resumed.

Before synthesis, the runtime attempts each exact executable worker check reported
as `not_run` or `unverified` once on that observed source revision. An existing matching final
verification receipt avoids a duplicate run. Policy/paid-consent blockers remain
blocked and explicitly not run, never converted to failures or automatically executed. Plan submission and verification repair first probe program availability in the real verification sandbox, using the same final launch path and check directory. An unavailable probe environment does not establish that the check program is missing; an incomplete wrapper command is refused rather than declared available. Workers declare checks early with the same preflight; it proves availability only, not success. Missing tools require an isolated project-private installation or an executable alternative, without sudo or system changes. If neither is possible, the responsible worker asks the owner immediately through the existing question route. New unrunnable executable gap declarations are refused before publication. A nonzero test result is a failure, not a technical waiver.

Human-only judgements and legacy gap items without a recorded command appear separately as **Check manually**. They keep their original evidence and explanation but do not enter passed/failed counters, offer Rerun, or block owner Accept. Legacy items with a recorded command remain rerunnable; an actual missing-program exit is a visible failed run, not a fabricated pass.

The check rerun control executes the retained literal command in its recorded task-checkout directory (including a subdirectory)
through the ordinary read-only verification runner, without a model turn. It does not widen the runner to a host-writable worker profile. It
binds to the current result version, check index, source digest and owning session. The native desktop bridge carries this same bounded identity, not executable arguments; the runtime still refuses a stale result or unavailable check.
Rerun is offered only for an item with a recorded command and no policy prohibition. A fresh run has a 60-second deadline and displays its actual outcome, exit code and a redacted bounded output excerpt, including successful output; complete output remains retained separately. Timeout/cancellation never earns a pass. An item without a command returns a plain-English manual-check explanation instead of a doomed rerun. The UI refreshes result evidence even after a technically blocked rerun, so a subsequent Accept uses its new source version. Check rows in working, question and review states show evidence availability and readable blockers, not report identifiers or internal codes. Conversation activity counts tools needing attention separately from actual failed checks; a refused tool submission is not itself a failed check. Identical delivery keys do not repeat execution; stale input refuses. Cancellation
uses the active session's existing stop path. New complete passing evidence can
close a check only on the matching published and reviewed source; later failed,
pending, incomplete or ambiguous attempts cannot reuse an earlier pass.
A rerun does not accept work, complete a failed assignment or resume a failed-check
sprint. For an open automated worker check, acceptance requires a retained technical or policy blocker
and the owner's explicit acknowledgement; manual items need no acknowledgement. Missing checks and real test failures
cannot be waived by that acknowledgement.

Maintenance checks eligible records roughly once per minute while the activated
service child runs. Done records are eligible for archive after the machine's
auto-archive interval (default seven days). Unmerged work asks the owner first;
declining leaves it outside the Archive. Archive retention defaults to 30 days
and is machine-configurable. Purge requires archived state and checks live or
unknown execution protection. Runtime deletion reuses the existing exclusive-lock,
journaled retention cleanup and reports SQLite checkpoint/VACUUM outcomes. Storage shows a persistent count of purged records with failed space reclamation, including explicit purges, and a per-maintenance-run count.
Dirty worktrees explicitly allowed for discard can still be retained rather than
force-removed; their Git branch and files remain available. Owner queue/comment
records are removed with the purged item. No unrelated project is swept. Retention boundary tests inject a maintenance-only clock callable through an unshipped test launcher: neither the desktop protocol nor the packaged launch path accepts clock overrides. The host clock and existing task rows are never rewritten.

Remove project stops its service child and removes only the locator from the
visible registry. Files, branches, task store and product history remain.
Re-adding the same folder preserves its public project identity.
The project menu, the worktree menu and Code can open VS Code through a closed
native bridge action when its launcher is found, on `PATH` or in its usual
folders (`code`, `code-insiders`, `codium`). Without one the same control reads
Open in default app and hands the file or folder to `xdg-open` or `gio open`.
With neither, it is off and says what to install. The webview names a project,
checkout, file and line only; the service resolves the real path inside that
checkout, and the webview cannot supply a command or an arbitrary root.

The native window handles Alt+Left and Alt+Right explicitly for Back and Forward;
these shortcuts do not depend on browser-default navigation. A finished worker's
published report remains open when the same worker/report identity is refreshed.
Changing that identity closes the old report and discards its page cursor.

## Editing files

Code edits the files of the project checkout or the task's worktree in
CodeMirror 6, with tabs, a dirty marker, find and replace, undo per tab and
syntax highlighting. Ctrl+S or Save writes through the machine service; Autosave
in Settings, Appearance is off by default and saves about a second after typing.

Every save names the revision the owner last read, which is the SHA-256 of the
file's bytes. The helper compares it while holding the machine lock, writes a
temporary file beside the target, syncs it, compares once more and renames it
over the target, keeping the file's mode. A save with an older revision writes
nothing and returns the current one. Reaching the file walks its folders one at
a time without following any link, so a link anywhere on the path is refused,
even one that stays inside the checkout. `.git`, `.cortex` and `.cortex-dev` are refused at any
depth. Files over 512 KiB, binary files, text that is not UTF-8 and files that
mix line endings open read-only with the reason; a file's own line endings are
kept. A model writer that uses plain file writes can still land between that
last comparison and the rename: the window is small and not closed, so the rule
below is what protects the owner while a task runs.

The open file is compared with the disk every three seconds and when the window
regains focus. A clean file follows the disk. A file with unsaved text shows the
conflict at once and keeps that text: Reload from disk takes the disk's version
and one undo brings the owner's text back; Keep mine saves it over exactly the
version that was shown, so a further change is another conflict. If the file was
deleted, Keep mine recreates it only while nothing else has.

A save is refused while the runtime has not confirmed that the checkout is
quiet. The helper reads the project store: a task bound to that checkout whose
runtime state is not `COMPLETED`, `FAILED`, `CANCELLED` or `PAUSED` holds it, and
an unreadable store counts as held. The file opens locked. Edit anyway lets the
owner type, and that text waits in the app until Stop or the task's end, when
Save writes it through the same revision check. Nothing here is a lease: the
runtime is not told, so it does not hold writers back for the owner.

Unsaved text lives above every screen, so moving between tabs, projects and
settings keeps it with its undo history. Closing a dirty tab asks first. Closing
the window by its button or through the window manager lists the unsaved files
and offers Save all and close, Close without saving or Keep working. A reply that
never comes leaves the tab at Save not confirmed: the disk is read once to see
whether the bytes landed, nothing is resent on its own, and Save asks again.
Unsaved text is held in memory, so a crash of the window loses it. Create,
rename, Save as, Compare and language servers are not built.

## Automatic task titles

New task requires a description, not a title. After saving, the activated service
requests a short title from that task's selected model route (or the controller's
configured route). Saving does not wait for naming and does not start execution.
The title prompt asks for the description's language, independently of message
language settings. Until a valid title arrives, the description's first line stays
visible. No engine, missing login, invalid output or any other naming failure
silently retains that fallback.

Two auxiliary sessions at most run concurrently per activated project; pending
descriptions are picked up by the service loop. Each attempt has a 30-second
deadline, 12,000-character description input limit, 1,024-character streamed-output
limit and 200-character single-line stored-title limit (the prompt asks for 80).
A durable attempt marker prevents retries after replay or service restart. An
interrupted attempt keeps its fallback rather than repeating a potentially billed
call. These sessions have no task channel, external MCP or filesystem/terminal
bridge, run from an empty temporary directory, and require read-only/plan mode.
They do not change owner settings.

Generated titles use the existing product overlay and polling path, including
materialized sprint tasks. Planned titles are not silently recorded as owner
renames. A user rename always records titleState=user, even for identical text.
Generation and rename serialize through the same file lock and atomic write;
a late result cannot overwrite an owner rename or resurrect a purged task.
No storage migration or wire-version change is required.

## Attachments

Every form where the owner writes a message or creates work accepts a file picker,
dragged files and pasted clipboard images: new task and new sprint, the task
conversation (including paused resume and review discussion), queued messages,
direct-worker messages, every answer inside a question card, planning messages, Request
changes, diff comments and conflict-resolution guidance. Text-only fields such as
names, branches, searches and commit messages take no attachments. Files appear as
removable chips with their name and, for images, a small thumbnail. A queued
message is the exception: its row in the queue shows no chips. The message keeps
its files; Edit returns them to the composer as chips, and once sent they open
from the conversation. A rejected file
(too large, too many, unreadable, an unusable name or type, or a machine without
attachments) shows its reason beside the form until the next successful action;
no part of that selection is silently sent.

Saved files stay available where they were sent. A task page reads its creation
files on demand, sprint descriptions and planning conversations show theirs, and a
sent conversation message keeps its files in the service's private product state as
a recorded copy. A chip with its contents opens a preview of an image or, for any
file, offers **Save a copy** through the system's save dialog; the owner picks the
destination. A chip with only a name cannot be opened: worker-guidance records carry
names, and a recorded copy drops its bytes, oldest first, when a newer queued
message or sprint needs the bounded space. Answers to questions keep their files
with the delivered answer, without a chip on the answered card. Each submission permits up to eight
files and 4 MiB of combined decoded bytes. A question batch shares that bound.
Text is required alongside attachments.

Queue saves both text and bytes in the service's private product state. It
survives window closure; **Edit** removes the queued item only after acknowledgement
and restores text and attachments to the composer. Existing composer text is
preserved before the restored text. A failed send retains the draft and files;
an ambiguous response is not automatically retried.

The local wire uses `Attachment {name, mimeType, data}` with standard base64
content. The service rejects path-like names, malformed encoding and excessive
input. Accepted resources are snapshotted in private `.cortex` source-block
storage and exposed through runtime prompt-resource references/`read_source`;
images use ACP image content, other files embedded resources. Answer text remains
unchanged; its identified answer, resources and wake-up references commit in the
same transaction. Stale or rejected answers capture nothing. Queue removal is not
a resource-delivery claim. No arbitrary URI is fetched and no attachment is
executed.

The native window disables Tauri's drag interception so WebKitGTK receives file
drops. On WebKitGTK versions that expose only URI text for native drops, a GTK
drop observer snapshots those exact files and offers a one-use, five-second buffer;
the webview cannot submit a filesystem path. WebKitGTK does not give the page a
clipboard image: it reports nothing, or only the copied page's `text/html`. A paste
with no plain text therefore asks the native clipboard for PNG, then any other image
GTK can decode, re-encoded as PNG within the same 4 MiB limit. Plain text is pasted
as usual, and no clipboard text or file path is read natively. Task creation retains
validated bytes in the private draft options until explicit Start snapshots them.
Direct-worker files are snapshotted with the identified guidance record and reach
only that channel at its next successful Cortex tool response. Resource references,
not filesystem paths, are delivered. Neither saving a draft nor attaching a file
starts a model call.

The data contract also accepts optional attachments for sprint creation, planning answers/messages, review comments and conflict-resolution guidance. Sprint descriptions and conversation entries expose their saved bytes for listing/opening. Planning uses the same ACP image/resource representation as task inputs, through the existing auxiliary planner (no project tools), and never places base64 in prompt text or diagnostics. A sprint's initial files and all planning follow-ups share eight files/4 MiB in total because a retry resends that input; excess is refused before saving. Omitted attachment fields on older records remain valid. The private product-state response has a bounded aggregate size below the 8-MiB local frame cap; a full state refuses new content rather than saving an unreadable record. Task creation bytes are readable on demand through `readTaskAttachments` on the real port, backed by the owner-only targeted `product.read` selector, rather than copied into every board snapshot.

## Task Stop and Resume

Task Stop records `PAUSED` independently of ACP turn cancellation, from a running,
task-level or worker question/access wait, or between-turn state. It preserves pending
question identities, batches and assignment-local waits transactionally; questions
stay visible with answering disabled, and runtime access reconciliation is held.
Resume restores the saved state; a question wait still requires the original
answer. Other interrupted work enters the runtime's recorded recovery path rather
than replaying the interrupted prompt. The hold survives service restart, and only
an explicit owner Resume reconstructs the exact recorded session metadata. Resume
refuses a stale display version or changed owner. Stop is urgent intent for the
exact execution and does not require the background progress version to remain
unchanged. Close as is and Cancel task are separate confirmed terminal controls.

A started task whose runtime row does not exist yet (its coordinator's engine is
still opening) projects as `starting`, not unknown, as long as its start, session
and owner facts agree. Retry recovery may advance the owner epoch beyond the original launch binding while keeping that same task/session; older epochs remain invalid. Stop and Code’s “Stop the task first” cancel this exact launch through retries; prompts and worker controls do not receive an epoch exception. Each automatic engine-launch retry shows the runtime's own
retry notice as the task status; a give-up projects as `blocked` with that notice
and needs the owner. There is no state to pause yet, so Stop cancels that launch
through the runtime's ordinary task stop and the task becomes stopped; a stopped
launch no longer holds owner checkout actions that require stopped tasks. A new service process still treats an
unconfirmed open launch as unknown.

This is not engine-process suspension. Stop fences old worker channels. Pending
worker waits retain their exact decision identity, including across service restart.
After Resume and the actual answer or runtime access decision, the runtime records
a replacement attempt for the same node and delivers the saved decision as source
data. This is explicitly a new session, not same-session recovery; completed work
must not be repeated and required access must be revalidated on the new channel.
Unknown action outcomes or unconfirmed process termination still block replacement.
Resume is runtime continuation, not a new user message, answer or requirement.
Stopping an individual worker or closing the task remains cancellation of its waits.
No promise of reviving a lost engine or undoing an external side effect is made.

## Worker reports and older activity

Open report reads only a published report referenced by a retained worker result
in this project. Pages have a verified immutable digest, identity-bound cursor,
4,000-character maximum and explicit partial/end labels. The view uses plain text,
not executable HTML, and never opens private runtime paths.

Older activity uses one continuation for both task events and command-control/guidance
rows, bound to the project, task, worker and attempt. Both event positions advance
together without repeating or skipping a namespace; newer arrivals stay in the live
tail. One read transaction supplies each page. Each worker DTO is at most 64 KiB,
with at most 200 combined events. A snapshot shares a 512-KiB history budget and has
a 768-KiB envelope ceiling: deferred history retains its own continuation rather
than disappearing. Oversized metadata or more than 100 workers for one task refuses
with a narrower-read message, rather than silently omitting workers. Text fields
are limited to 8,192 serialized UTF-8 bytes (command previews: 2,000), including an
explicit clipped-prefix/original-byte-count label. Original stored content is not
changed. Old task-only cursors require reopening history.

Older guidance pages carry delivery status and attachment names into the worker
view. Activity filters include recorded failures, and scrolling away turns Follow
off until the owner returns. Loading older events does not count as new live activity.

## Local voice input

Ctrl+D or the microphone starts and stops on-device Russian dictation in a
composer. Partial recognition appears live inside that composer; stopping leaves
a preview, never a message. Cancel discards only the recording. Insert appends the
final transcript to the editable draft. Insert and send is a separate explicit
action where that composer supports sending (including the answer inside a question card); new-task
dictation never creates or starts a task.

The service captures the default desktop source through PulseAudio's parec
(including PipeWire's PulseAudio compatibility server), not Web Speech or a cloud
speech service. The package contains sherpa-onnx-node 2.0.1 and uses bundled Node.
On first use it downloads the fixed small Russian Zipformer model (about 110 MB
download, 27 MB installed) with visible progress and verifies the archive and model
files against committed SHA-256 hashes. Subsequent sessions use the channel-private
model cache without a download. No audio is uploaded or written to disk.
Only explicitly sent transcript text enters the ordinary Cortex prompt path.

Recognition refreshes partial text using bounded overlapping offline inference;
it is not a token-by-token streaming model. Long utterances are segmented on
silence or at 20 seconds. A recording stops at 90 seconds and still requires
Insert or Insert and send. The service allows one microphone lease at a time;
a lost composer/window lease expires after ten seconds. Service stop cancels it.
Missing capture tools, audio-server/microphone access, unavailable local runtime,
failed download, failed integrity or a lost session are reported, not simulated.
The default model is Russian-only; other speech languages and physical-device
quality are not implied by fixture checks.

## Explicit limits

- MCP Connect, Check tools and Retry list tools from the unsaved draft through the existing sandbox, without saving or calling them. Listings are bounded and redacted; configured grants remain editable in the matrix. Dismissing a check ignores its eventual reply; its server stops at the deadline. A sandbox refusal remains a refusal, never an uncontained retry. The real sandbox fixture requires working user namespaces and is not qualified on hosts that refuse them.
- Open logs opens private rotating status events: timestamps and fixed categories only, no raw service output. It uses `xdg-open`, then `gio open`; with neither, the control is off and says what to install. An opener's failure is shown as that failure, and one still running after eight seconds counts as handed over.
- The two-task, windowless real-service fake-engine sprint journey and forced failed/skipped-check rerun receipts are not yet qualified. Check-rerun remains a request, not proof that a check ran.
- Ctrl+J opens the local project shell; hiding or closing the window preserves it, while Close shell stops its jobs. The initial cwd is the registered project folder even in a task view, not the task worktree. See [terminal authority and limits](security.md#owner-terminal-built-local-linux). Remote-machine controls remain off; local voice input is described above.
- Native rendered qualification must run on a newly built binary. Source tests
  and a failed native build are not rendered evidence.
- This slice is not a new Debian package. Stamping, packaging and installation
  qualification remain separate release steps.

## Focused no-paid checks

Use checkout-local Python and sequential checks; do not run the entire suite.

```sh
timeout 70 .venv/bin/python -B -m pytest -q --timeout=25 tests/test_workspace_task_options.py tests/test_workspace_task_git.py tests/test_workspace_purge.py tests/test_workspace_scheduler.py tests/test_workspace_machine_tools.py
timeout 80 .venv/bin/python -B -m pytest -q --timeout=60 tests/test_workspace_titles.py tests/test_workspace_service_integration.py tests/test_workspace_journey.py
~/.cargo/bin/cargo test --locked -p workspace-service --test terminal
~/.cargo/bin/cargo test --locked -p cortex-workspace-desktop --no-default-features
npm --prefix packages/workspace-ui test -- test/terminal.test.tsx test/terminal-client.test.ts test/voice.test.tsx
# Voice fixture and model cache prepared in disposable container artifacts:
python3 -B packaging/container/check_rendered.py --container --artifacts <build>/artifacts --only voice
# Against freshly built native binaries; disposable no-network container, no images:
python3 -B packaging/container/check_rendered.py --container --artifacts <build>/artifacts --only terminal
npm --prefix packages/workspace-ui run typecheck
npm --prefix packages/workspace-ui test -- test/real-port.test.ts test/real-screens.test.tsx test/store.test.ts
```

The fake-gh fixture uses a local bare remote and a disposable executable; it proves invocation and duplicate-attempt guards, not GitHub behavior.

Git-rule fixtures inject the runtime-result validator; they establish Git safety,
not runtime verification. The service journey uses the real adapter/store with a
fake runtime, creates a real task worktree, confirms its own file/diff and single
store, and retains detach/restart behavior. No check calls a paid engine.
