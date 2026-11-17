# Changelog

Entries record changes as they were made. Files, commands and tests they name that belonged to the Python runtime (removed on 2026-11-16, see 1.0.6) are history, not current commands.

## 1.0.6 (unreleased)

"Open report" on a native task's worker card no longer answers "This control is not available for native execution yet." The native service refused every wire-1 query except `history.page`, so the report reader never ran. A natively hosted project now serves `reports.read` read-only: the report must be a published report of a runtime task bound to a workspace execution of that project, it is paged at up to 4,000 characters through the same digest- and cursor-checked reader the workers use, and anything else is `not_found`. A refused or failed report read is shown as a readable message, never the raw refusal. The other reads the desktop makes (`project.snapshot`, `tasks.list`, `tasks.get`, `executions.get`, `results.get`, `events.page`) are not offered by a native project and are served by the native query route instead, so they never reach the refusal.

The desktop no longer flashes "Not connected to the service. Nothing shown here is current." while the service is running. The service served at most 64 connections and accepted and dropped any caller past that bound; the desktop opens one connection per request and its own slow machine reads held up to about 60 of them, so its three-second `service.status` check or a resubscribe could be dropped and read as a lost service. A caller past the bound now waits to be accepted instead, and the desktop keeps at most 48 ordinary requests in flight so the status check and the subscription's own resync always have a connection (a request that waits past its deadline for one is refused as busy with nothing sent). One failed status check while connected no longer shows the machine offline; two in a row do. A subscription that ends is resubscribed and resynced silently and is shown offline only if that fails for five seconds, a `native_delta` the window cannot validate re-reads its project instead of ending the subscription, and every end is logged on the desktop's stderr as one bounded line naming its reason code (no frame content).

A large task no longer makes the project's facts and worker reads fail with `native_facts_limit`. Receipts, queued messages, workers and activity are read in bounded pages: a worker's page returns its newest 200 rows with an `activityCursor` that reads the older ones, an over-large reply moves its oldest rows behind that cursor instead of being refused, and a result with more check receipts than one read presents is marked `checksPartial` and cannot be accepted from that view. The coordinator page shows only the coordinator's own rows, a command run on the coordinator's channel no longer appears on a worker's page, and the Conversation no longer lists a worker's tool calls. Older tool steps, notes and queued messages are reachable through the same cursor, and Accept itself refuses a result whose receipts exceed one read. When worker facts cannot be refreshed, a worker last seen running under a task that has ended shows as not confirmed instead of "Working", and the failure message names the time of the data still shown.

A worker that reported its assignment failed now shows that failure on its card, with its summary, checks and a button to open its report, instead of "reported no result"; workers without a result say whether you stopped them, the coordinator cancelled them or they were lost to a restart.

Task states no longer read as waiting, running or mergeable when they are not, and no task is left waiting without an action. Completion no longer refuses a verified worker pass because the worker's descriptive `checks[].command` differs from the cited receipt's argv (`completion_worker_check_unverified`; submission never compared them), a completion gap hit by a task driven after a restart or Resume takes the live driver's coordinator path instead of an "interrupted" hold, and a result an earlier build held only for that reason is checked again on the next start. A stop for any other work blocker (for example a worker whose turn ended without an accepted result and whose retry gave up) now asks the runtime's localized stop question at once instead of waiting in RECOVERING for a Retry no screen offered; such a stop left by an earlier build gets its question on the next start. A worker's correctable `result_submit` refusal (`open_failed_check`, `omitted_failed_check`, `check_blocker_required` and similar) is returned to the worker with its code and correction instead of a bare Forbidden, every refusal is retained as `WORKER_RESULT_REFUSED` (code and attempt only), and the worker card and exactly bound Submit result steps say why. Finish chosen on the runtime's stop question (`USER_FORCE_FINISHED`) is projected as the owner's close as is, so Needs you, the Inbox and Home drop the task at once, also for tasks in older stores. An answer-only task that changed no files reports `noChanges` and reads Nothing to commit with Close task instead of "There is no record of which files…". In Workspace, a held task offers Continue (the runtime's Retry), says Interrupted only for a lost turn or unknown outcome and Stopped otherwise, names recovery codes in words, never shows "Work is running" when nothing runs, and its Accept waits for the task to finish; a paused task waits in Needs you (counted apart from Working in the sprint Summary); failed and cancelled runs read as their outcome; a project-folder record with no target branch reads Ready to finish; and a closed task offers no Commit in Changes.

A worker left running when the service was killed no longer blocks its task, its sprint's deletion and its files for good. The native host records engine processes only through a delegation row and a capacity reservation, so after a SIGKILL those rows stayed `ambiguous`/`termination_unconfirmed` with no process record, every restart in the same boot kept them so, and the owner's message, Stop, Resume and Delete were refused (`native_shutdown_unconfirmed`) until a reboot. Restart recovery (`crates/workspace-service/src/native_recovery.rs`) now settles such an attempt (delegation `terminated`, reservation `released` with observation `no_engine_process`, `NATIVE_PROCESS_SETTLED` evidence `service_gone_and_no_engine_process_in_project`) when the process table shows no engine adapter, Codex App Server or session relay of this user working in the project or a task workspace; an unreadable entry counts as one and keeps the attempt fail-closed. The task is held as interrupted for the owner, never driven on by itself, and this applies to rows written by earlier builds, including closed tasks, so a retried Delete sprint completes. Recovery also removes a dead service's broker directory (`cortex-mcp-…`) that no socket is bound in. Write holdings (`holdings_tx` in `crates/cortex-core/src/scheduler/policy.rs`) no longer count a node of a finished or deleted task, or an attempt whose rows are all settled, so a crashed worker no longer write-locks its files project-wide; a ready step whose files another unfinished task's worker holds now waits for that holding (`NATIVE_WRITE_CONFLICT_WAITING`, once per holder) instead of stopping its task with `native_continuation_unavailable`, and a stop that does carry that code reads "no planned step could be started" (localized) instead of the raw code. In Workspace, a Delete sprint that has started but still waits for a task now closes its confirmation dialog; the receipt line and the sprint's Deleting strip say what is left.

Sprint planning no longer gives up on a recoverable planner reply. The plan parser now accepts a missing `dependsOn` (no dependencies), a single-string `dependsOn`, a title over 200 characters (cut to 200 with an ellipsis) and a reply with prose around exactly one JSON object or code fence; duplicate keys, unknown dependency keys and cycles remain errors. A rejected reply is reported with a stable code and the failing task (1-based index and key) and rule, for example "task 4 (t4): description is empty", "task 7 (t7) depends on unknown key t12" or "question options: more than 12", never raw model text. Before cancelling, planning makes one more pass with the exact validation error and the required schema (a fresh planning session with its own 90-second limit); only a second invalid reply cancels planning, and no task is executed either way. Engine failures and time limits are not retried.

A task's Conversation now shows only the coordinator's messages (in the task's language) with the compact work rows, worker summaries, questions and results. Worker and reviewer narration, which they write in English by design, no longer appears there as message paragraphs; it is shown on that worker's page under Work details as text rows in order with its tool steps (newest 60 messages of up to 4000 characters, under a shared size bound, marked partial when cut), and the worker's tool steps still count in the "Worked …" lines. The display history leaves out an agent message only when its attempt is recorded under a role other than the orchestrator, and the worker page reads the same messages by that attempt, so histories written by earlier builds read the same way and any message with no recorded attempt stays in the Conversation. No wire schema changes.

Cortex Workspace owner-interface defects from a live run of the dev build are fixed. A started, finished or closed task's title can now be renamed through the same Save/Cancel title editor as a draft's: the title is presentation, not lifecycle, so `product.task` `rename` (and the Core `Change::Rename`) no longer refuses a task with an execution with `task_started`; its saved request and settings stay read-only after Start, and a cleared or deleted task refuses the rename with `not_editable`. The floating "N new"/"Latest" pill and the folded "Send a separate message" row now stand in an opaque band of the timeline's own surface that the timeline reserves below its content, so they no longer lie over a question's options or a result's footer. The desktop no longer announces a question again after a relaunch: the question identities already seen are kept in the app's local storage, and one that arrived while the app was closed is still announced once. A blocked Continue queue takes the shared disabled look of a main action instead of an outlined button that read as usable. The new-task and new-sprint hints and two shortcut descriptions no longer say "the plane" (meaning the send button), which read as a typo. Settings → Tools and MCP names each tool count's audience ("15 tools for writing workers, 11 for read-only roles, 8 for the coordinator"), because the three grants overlap and were read as one total. A model-catalogue observation whose engine does not confirm a model switch (seen with an OpenCode connection in no route) now reports `engine_unreachable:model_switch_unconfirmed` (and `model_switch_unanswered`, `model_option_missing`) instead of the Python exception names `ValueError`, `KeyError` and `StopIteration`; such a failure of an engine no offered model comes from stays off screen. External MCP servers such as codebase-memory-mcp are still launched once per engine session, by design: each session's server runs in that session's sandbox and grants and stops with it.

Archiving, deleting and cleaning up tasks in a project whose working tree had a large uncommitted diff failed with "Git did not answer in time". The cleanliness check behind those actions built the full working-tree diff; a diff larger than one response frame (8 MiB) stopped the service reading Git's output while Git waited to write more, until the 25-second Git timeout. The service now stops Git as soon as a stream crosses its bound, the cleanliness check uses `git status` and the untracked listing without any diff, and the repository snapshot shows an oversized diff as files with line counts and no hunks instead of refusing. The Git tab of a project with a registered worktree whose folder was deleted (one `git worktree list` marks prunable) showed "No Git record" because the snapshot ran Git inside the missing folder and reported the failed start as "Git is not installed"; the snapshot now compares branches from the main checkout, and a missing working folder is refused as `checkout_missing`.

Two engine-session defects from a paid live run of the dev build are fixed. Claude sessions intermittently failed to start in `configure` with `transport_closed_outcome_unknown`, which stopped the task with "a worker session could not start" even when the coordinator's session had failed. claude-agent-acp 0.107.0 sends a new session's `available_commands_update` about 3 ms after its `session/new` reply; the engine host registered the session only when the waiting caller resumed, so a later resume made the reader see an update for an unknown session and close the transport. The reader now registers the session from the reply itself before delivering it. A transport lost while an unbound session is configured, with the engine's exit confirmed, starts the session once more under the same channel (`NATIVE_SESSION_START_RETRIED`, then success or `NATIVE_SESSION_START_FAILED` with attempt 2); the session is now bound to its channel only after it is configured. Every recorded start failure keeps the engine's exit status, the transport close reason and its stderr size, with a bounded, redacted stderr tail in a private `.native-diagnostics` file named by its digest. A coordinator start failure stops the task with `coordinator_session_start_failed` (a worker keeps `native_session_start_failed`). Every Cortex-launched Codex session also started the owner's plugin MCP servers (the computer-use REPL with `node_repl` and a sandbox probe, and the app tools `server.mjs` from the Codex primary runtime): codex-acp 2.6.0 sends a `features` table in each thread's config, and Codex 0.166.0 lets it replace the launcher's `-c features.*` layers. Launcher-backed launches now give codex-acp `CODEX_CONFIG` with the launcher's disabled features, which codex-acp spreads into each thread's table, and the launcher refuses a relay-carrying session without it. The catalogue observation does not set it yet. Regression tests: `cortex-engine` `engine` (`session_update_right_behind_the_session_new_reply_keeps_the_transport`, `failed_engine_leaves_bounded_redacted_diagnostics`, `redaction_keeps_ordinary_diagnostics_readable`, and the opt-in `real_claude_sessions_configure_concurrently_without_transport_loss`), `native_sessions` (`claude_update_behind_session_new_reply_never_fails_configure`, `configure_transport_loss_starts_once_more_and_records_both_attempts`, `repeated_configure_transport_loss_stops_with_the_roles_blocker`, and the relay hand-over scenario now also requires the thread feature config), `codex_launcher`, the `native_codex_runtime` unit test `base_layers_and_thread_config_disable_the_same_features` and the opt-in real-chain `codex_real_chain` (`real_codex_session_starts_only_the_cortex_relay`).

Workspace review-gate defects from a paid live run of the dev build are fixed. Accept stayed off on a native result whose runtime verification passed and whose review approved, because the worker's own superseded claims (a run before its last edit, a trivial command, a check its sandbox could not run) and the reviewer's matching gaps counted as open checks that nothing on screen could rerun; once the runtime's own verification of the current version passed on the unchanged folder, such `unverified` and `not_run` self-reported checks are notes on the result card, while self-reported failures, policy-blocked checks and failed runtime verification still block. The reason beside a disabled Accept no longer ends in "..". A reviewer reason naming `revision_mismatch` read "a required tool or service was unavailable": every reason code the runtime records now reads as what it means, also inside a sentence, and an unknown code is named as recorded. A worker that only read files showed "No activity recorded yet": a worker's page now lists its reads, skill reads, searches and other tool steps from the same retained rows as the timeline. During rework the Inspector no longer counts the previous version's worker checks; a project-folder sprint task's result is compared on the files it recorded writing, so later tasks' writes no longer make it stale; a result the app committed itself is not stale; the "All queued tasks have run" banner goes once every sprint task is closed; the New task form says Ready to finish for a project-folder task; and a closed task no longer offers Request changes.

A task whose worker session an earlier build refused before launching its engine (for example an external MCP attachment refusal) could never continue, be closed cleanly or be deleted: that build recorded the attempt's process outcome as unknown, so recovery kept it `ambiguous` forever and every owner message, Continue answer, Close task and Delete sprint was refused (`native_shutdown_unconfirmed`, `lost_worker_termination_unconfirmed`, `active_execution`). Restart recovery now settles such an attempt from its own start-failure event (`NATIVE_PROCESS_SETTLED`, evidence `session_start_failed_before_engine_launch`), and settles any other unconfirmed attempt once a later boot proves its process gone (`NATIVE_PROCESS_UNCONFIRMED` stamps); an attempt that may still run in the current boot stays fail-closed. A `broker`-phase start failure is now a confirmed exit, and a driver that ended without signalling no longer fails every later control. A task closed with Close task reads as closed even when its finish was refused, so it no longer shows a "Waiting for a decision" card or counts as Needs you; that card is shown only for tasks that really wait on the owner, and Mark as handled only where it is offered. Delete sprint names each remaining task by title in plain words and shows its refusal once.

Opening a project with tasks from earlier versions no longer breaks it, and current owner actions apply to those tasks. A project whose store held worker rows of tasks the retired Python host ran before runtime records existed could not open at all (`storage_unavailable`, caused by `FOREIGN KEY constraint failed`): restart recovery wrote a runtime event for each such task, and its single transaction rolled back. Settlements of a task without a runtime record now keep their evidence on the settled rows only, and each task's recovery runs in its own savepoint, so a task whose recovery fails is rolled back alone (and held with `native_recovery_failed` when it has a runtime record) while the project opens. Finished Python-run desktop tasks are now adopted (`NATIVE_TASK_ADOPTED` with `finished: true`, nothing replayed), so Request changes, messages, Continue, Retry and Close no longer refuse them with `native_execution_binding_invalid`. Process records of the retired host without an end time kept old tasks "running", so Delete task and Delete sprint refused them with `active_execution`: recovery ends a record from an earlier boot (`previous_boot`), or from this boot only when neither its exact process nor any process of its group exists (`process_gone`), and deletion ignores records from an earlier boot. Never-started Python worker rows (no engine session, no process record, capacity already released by that host) are settled instead of being flipped to "outcome unknown" on every restart, and a restart no longer re-records "recovery required" or a lost worker wait for a task already held for the same reason. Close task on a task whose earlier attempt is still unconfirmed reported a failure although the task was already cancelled, so the window kept the card; the close is now reported done (`processesUnconfirmed`), and deletion stays refused until the attempt is settled. Continue on a paused task held with an unknown outcome moved it to RECOVERING and started nothing; with every owned process confirmed gone it now relaunches the coordinator like an owner message. A finished result from an earlier version reads as a read-only result of an earlier version of Cortex with its own identity (it was an empty result with no digest and version "0"), a closed card reads "This task is closed." and a cancelled one "This task stopped without a result to review." instead of "Wait for Cortex to finish this result." Regression tests: `native_recovery::legacy_tests` (`tasks_without_a_runtime_record_or_with_a_damaged_state_never_fail_the_project`, `a_finished_python_task_is_adopted_and_its_result_is_shown_as_an_earlier_version`, `process_records_end_only_when_their_process_cannot_run`, `a_process_record_from_an_earlier_boot_never_blocks_deletion`, `never_started_legacy_rows_settle_and_restarts_record_nothing_new`) and `native_controls::resume_tests` (`a_closed_legacy_task_becomes_deletable_after_recovery`, `resume_never_erases_an_outcome_unknown_barrier`, `continue_on_a_paused_task_held_by_a_lost_session_relaunches_it`).

Three engine-setup defects from a paid live run of the dev build are fixed. On a fresh install only Codex's models appeared and new tasks defaulted to a Codex model: the per-connection catalogue refresh threads raced to create the shared catalogue folder and the losers failed with `refresh_lock_unavailable` (`private_directory` checked, then created), then waited 10 minutes. Private-folder creation now accepts a folder another thread or process created first, after the same checks, and a failed refresh is tried again 5 seconds later, doubling to 10 minutes. The open app did not show a model list that arrived later until relaunch: the store followed `catalogueRefresh` only on its own reads, not on the port's machine poll, and stopped reading once a list had failed; it now follows both and keeps reading while a list is fetched or has failed. Codex sessions started the owner's own Codex MCP servers although the launcher turned them off: codex-acp sends ACP `mcpServers` as one `mcp_servers` thread config table, which replaces the launcher's `mcp_servers.<name>.enabled=false` layer. Launcher-backed sessions now hand their relays to the launcher, which defines them as `-c` layers and checks them with `codex mcp list --json`; ACP `session/new` carries none. The limits read's App Server now carries the same feature layers, so it no longer syncs plugin marketplaces (`git ls-remote`).

Six Workspace review and closing defects from a paid live run of the dev build are fixed. A project-folder task in review said Accept moves it to Ready to merge while its header said Ready to finish; both now name the same step. After a relaunch, a task created since the private product overlay was last written read Ready to merge with no branch: the native `product.facts` read used the overlay alone, so it had no checkout. That read now merges each task's store-owned options (checkout and current branch, settings, base revision, saved draft title) and the Ready-to-merge integration facts, as the helper read does. Right after Request changes the Changes tab showed a phantom "Result v2" with Accept: the task keeps its plan revision until its next baseline, so the accepted result's host check was presented again; receipts before the latest reopen are no longer presented, and a working task never offers Accept. Sprint tasks are named by their plan items' titles again (the removed automatic naming used to supply them; every task read as the first sentence of its description): joining the sprint records the title as generated, never over an owner's title. An accepted, verified result closed as is reads Closed as is, accepted v<n> (not "not verified"), and its Changes tab keeps the files its workers recorded instead of No diff recorded. Finish on a project-folder task confirms files written through Cortex's own `project_write`/`project_edit` from `NATIVE_PROJECT_WRITTEN` and the matching completed request receipt (exact path and SHA-256), so they can be committed. A worker check the runtime kept unverified (`revision_mismatch`, `trivial_command` and the other evidence gaps) now says why in words instead of blaming an unavailable tool or service.

Seven Workspace timeline defects from a paid live run of the dev build are fixed. Edit rows open on a real diff in Conversation and on a worker's page: claude-agent-acp sends a call's input in later untitled updates, and native activity used to treat every untitled update as possibly private, so a `project_write`/`project_edit` kept only its path; it now judges such an update by the title and kind it retained for that call (still private when none was retained). A worker's edit rows read their lines from the runtime's own record of that write, cut to the changed lines with up to three lines of context; an edit with no recorded lines is a plain row with its `+N −M` and never an empty Diff box, and the Conversation diff opens with its step instead of behind a second View edit. A refused call (a coordinator's plan the runtime did not record) keeps the runtime refusal's message, bounded and redacted, and its row opens on it. Two messages whose streamed chunks follow each other (the coordinator's, then a worker's) no longer run together: the history projection starts the second with a paragraph break. A read of the project root reads "project root" and opens Code; the Workers line counts cancelled and failed workers as the header does; Ask about this worker is the finished worker's result card's last row instead of a floating button over its checks; a question with options keeps its answer line, so choosing an option no longer moves the page; and the owner's own new message turns Follow back on, so it and the Working line sit above the composer. Regression tests: `native_activity` (`an_untitled_update_keeps_its_calls_edit_and_refusal_but_never_private_text`, `prior_call_reads_the_announcement_of_the_same_task_call`, `retained_activity_projects_paths_skills_commands_and_diffs`), `native_owner_controls` (`an_edit_shows_its_changed_lines_with_a_little_context`), `task_review` (`native_workers_report_their_edits_result_and_checks`), the cortex-core `projections::history` tests and the Workspace UI `live-run-defects` test.

The owner can now run an external MCP server without the Cortex sandbox, explicitly: `"containment": "none"` on an entry of `mcp-servers.json`, or as the root registry default for every entry without its own value (an entry's `"contained"` overrides it; absent everywhere still means contained, so existing registries are unchanged, and any other value is refused as `invalid_mcp_registry`). Such a server runs with the owner's full user authority, with no Landlock, seccomp or network restriction; its path grants, `network` and `runtime_dir_env` are not applied, and it gets the owner's `HOME`, a `PATH` that includes its executable's directory and `~/.local/bin`, and the Cortex process's `XDG_RUNTIME_DIR`. It still launches only the registry's resolved executable, behind the channel-bound relay with its tool grants, reservations and no-replay, in its own process group stopped at session end (a daemon that detaches from that group is outside Cortex's kill guarantee). Its session start skips the per-file project walk that builds a Landlock policy (about 9.5 s per server on this repository, minutes on large projects). This lets `codebase-memory-mcp` 0.10.8, which cannot start contained, run when the owner opts in. Pre-save tool discovery follows the same choice. Settings shows such a server as **No sandbox** and has a per-server **Run without the sandbox** switch; profiles neither capture nor replace the field. Regression tests: `native_mcp` unit tests (`containment_is_explicit_with_a_contained_default`, `uncontained_launch_skips_the_walk_and_uses_the_owner_environment`, `contained_default_still_walks_and_refuses_unsafe_grants`), `native_sessions` (`uncontained_external_server_runs_with_owner_authority`), `mcp_discovery` (`uncontained_discovery_runs_without_the_sandbox_only_when_chosen`), `machine_save_tests` (`machine_mcp_containment_is_kept_inherited_or_explicit`), `mcp_management`, `settings_view` and the Workspace UI `first-use` test.

An optional external MCP server no longer stops a task. A writing worker whose scope named a file it had not created yet (for example `docs/styleguide.md` in a project without `docs/`) could not start whenever an external server applied: the server's Landlock rules tried to open the missing path and the session failed with `NATIVE_SESSION_START_FAILED` (`authority_refused`, phase `external_mcp`), which stopped the task with the runtime question. A not-yet-existing scope is now simply not granted to the server (never created, never widened to its parent). A server whose grants, scopes, containment, launch or relay cannot be prepared, or whose process exits before `session/new`, is withheld from that session only: the session starts without it, `NATIVE_EXTERNAL_MCP_WITHHELD` records the server name and a reason code, and the timeline shows a failed note naming both. Project entries the owner's own permissions deny (such as root-owned 0700 build directories) no longer refuse the server's containment; they receive no rule. `codebase-memory-mcp` 0.10.8 exits during its own startup checks inside this containment (it needs procfs and filesystem-wide directory opens, local sockets and `setsid` for its daemon), so it is now reported as withheld (`server_exited_before_session`) instead of appearing to the engine as a server that failed to start; the containment was not widened. Regression tests: `native_sessions` (`configured_external_worker_grants`, `optional_external_failures_withhold_only_that_server`) and the `native_sandbox` unit test `owner_unreadable_project_entries_are_skipped_not_refused`.

Workspace startup no longer reads the machine overview. The first screen waits only for `config.read` and the activated projects' `project.config.read`; the overview (storage accounting, retention and auto-archive days, worktree folder, version, GitHub CLI availability, cleanup and diagnostics) is the on-demand port query `machine.overview`, read once when Settings, the Archive, an archived task, the shown inspector Machine panel, project settings or a Create PR control first needs it, and refreshed with later machine refreshes. Until it arrives those places say they are reading the machine's details instead of showing defaults; a failed read is retried every 3 seconds and blocks nothing else. Regression tests: `real-screens` ("machine overview, read when a screen shows it"), `machine-startup` ("the machine's overview"), `real-port` and the domain `port` test.

Two defects found by the Paseo 0.15.3 qualification are fixed. A streamed coordinator or review message was stored as one history row per engine chunk and replayed with a distinct `messageId` per row (`<session>:<seq>`), so Paseo drew each few characters as its own timeline block. Native activity now stores one stable message identity (the engine's own `messageId` when it supplies one, otherwise the first chunk's) for consecutive chunks of one attempt, and the history stream and replay send it; a tool row or another attempt starts a new message. A second Cortex agent in a project whose first agent had finished failed with `scheduler_owner_busy` because the first `cortex-acp` process kept the scheduler lease for its lifetime; incoming agents now hand the lease over while idle with no unfinished task (see [native incoming ACP](docs/project/native-incoming-acp.md)). Regression tests: `native_sessions` (chunk identity), `owner_fencing` and `incoming` `shared-project`.

The bundled engine adapters move to codex-acp 2.6.0 (with `@openai/codex` 0.166.0, the release its `^0.166.0` range resolves) and claude-agent-acp 0.107.0 (Claude Agent SDK 0.3.323). Both are registered as exact qualified versions, and 2.3.0, 2.1.1, 2.0.1, 1.13.0, 1.12.0 and 0.97.0, 0.88.0, 0.87.0, 0.78.0 stay qualified for rollback. The Codex private patch for 2.6.0 is tied to its distribution digest and re-derives four of its 14 rules (session creation, MCP server conversion, MCP de-duplication and turn start, which now must not send upstream's permission-profile fields); Cortex still negotiates ACP v1. Claude 0.107.0 now starts a session in `auto` mode when the user's settings name no default mode; Cortex's native sessions set their mode explicitly. The engine lock, the pinned-dependencies manifest and the bundled-engine licence inventory are updated. No-paid observation (`initialize` and `session/new` only) advertised the same models and options as the previous adapters; this is not paid, native-tool or desktop qualification.

Native orchestration fixes (known issues KI-02, 03, 07, 08, 10, 15, 16, 17, 18). Stopping one worker no longer dead-ends the task: siblings finish, then the task returns to the coordinator with a plain-language reason, and dependents of the stopped node never start silently. A verification repair that drops or replaces a failed check records it as dropped whatever the list length, and a new trivial command cannot stand in for it (`verification_trivial_replacement`). A reviewer is routed to a different model than the work it reviews when the routes offer one; otherwise the review proceeds with a recorded `REVIEW_INDEPENDENCE_WARNING`, and the old, always-true attempt-id comparison is gone. Route escalation is scoped to the plan revision. Fix nodes inherit the routing fields of the node they fix and review nodes route as review work (`plan_json.routing`). A reviewer's REQUEST_CHANGES that cites a failed check starts a fix cycle instead of `review_blocked`. Runtime-composed synthesis follows the documented contract (execution summaries only, escaping, 1,600-character and 48 KiB bounds, a changed-file list from a retained baseline manifest, localized labels, the `SYNTHESIS_RUNTIME_COMPOSED` event, no fast path after a waived review). The `situational_skill_required` promise was removed from the `result_submit` and `read_skill` descriptions: the native runtime observes neither skill reads nor per-attempt changes. Inline report text that merely starts with `r_` is no longer treated as a report id.

Cortex now contains no Python. The Python runtime (`src/`, including `src/cortex_acp.py`), every Python test under `tests/`, `.venv`, `pytest.ini` and the Python helper scripts were removed on 2026-11-16. The ACP agent is the Rust binary `cortex-acp` (`crates/cortex-acp`), which is the only entry for Zed registration, the Paseo launch record and the Paseo source-root launch: the Python entry, `--python`/`--python3` and `CORTEX_ACP_ENTRY=python` are gone, and `print-client-recipes` refuses them. The workspace service has only the native Rust execution host: there is no `machine_tools.py` helper, no legacy ACP child or `agent_launcher.py`, no `CORTEX_WORKSPACE_NATIVE=0` opt-out, no three- or five-argument host form and no `--fixture-launcher`; only `<allowed-root> <runtime-root>` and the one-argument MCP relay remain, `projects.resynchronize` is removed, and a request that only the Python helper could answer is refused natively with its own code. The `.deb` no longer depends on `python3` and ships no `src/` or `.py` files (the installed real-input gate now fails when `python3` is present in its container unless the archive's `Depends` names it). Native attachments remain unavailable on the native host, as before. The workspace contract schema moved to `crates/workspace-protocol/schema/workspace.json`.

Build, packaging and release tooling is Rust. `crates/cortex-xtask` (wrapper `scripts/cortex-xtask.sh`) provides `generate-contract`, `generate-instructions` and `profiles` (each `check` or `write`), `check-generated`, `package stamp|check`, `runtime-payload list`, `check-pins`, `prerequisite-probe`, `test-fast` (replacing `run_tests_fast.py`: bounded, sequential, no-paid cargo, Vitest and Paseo steps), `build-deb` (used by `scripts/build-dev-deb.sh` and `scripts/build-prod-deb.sh`) and the installed real-input release gate `installed-gate` with `check-install`, `check-systemd`, `check-real-input` and `check-bundled-engines`. `crates/cortex-paseo-tools` (wrapper `scripts/cortex-paseo-tools.sh`) provides the Paseo plugin build step `bootstrap-paseo-git-install`, `install-paseo-plugin`, `configure-paseo-plugin-launch`, `init-storage`, `status`, `validate-connection`, `install-engine`, `add-connection`, `add-opencode-connection`, `refresh-catalog`, `pin-route`, the guarded dev wrapper `paseo-dev` (including `gui [--xvfb] [--port]`), `paseo-clone`, the no-paid `acp-smoke`, the paid probes `probe-codex` and `probe-claude` (still requiring `--i-authorize-paid-call` and `--approval-ref`), `print-client-recipes`, `uninstall` and `clone-dev-settings`. The Paseo plugin has no Python route. The Python-era `cortex_inspect_task`, timeline and trace-view tools, the qualification assertion and observer, the read-API fixtures and benchmarks, `cortex_diagnose`, `cortex_engine_quality`, `cortex_model_policy_migrate`, the isolated-test launcher, the incident evaluation, the login keepalive, the Playwright MCP installer and the routing preview check were removed without replacement, as were the `paseo-dev` modes `--wire-trace`, `route-preflight` and `binding`. The storage `plan`/`migrate`/`rollback` tool was removed too: the Rust store upgrades format 11 to 12 automatically on a normal writable open (`OpenMode::Initialize`), with a verified private backup in `.migration-backups/`; format 10 and unknown formats are refused, and no rollback command ships.

A SQLite store can no longer be corrupted by opening an existing database file twice. `Store` initialization used to open an existing database with a plain file descriptor; closing that descriptor dropped every POSIX lock the process held on the file, including those of live SQLite connections, and let another process delete the live WAL and shared-memory files. Initialization now only creates a missing database file (`create_private_database` in `crates/cortex-core/src/store/locks.rs`) and never opens an existing one that way.

Documentation, `README.md`, `SECURITY.md` and `AGENTS.md` describe the Rust-only product and the new commands. Pages that carried statements about the removed Python runtime keep them only as dated history or as unverified carried-over behavior, and say so. The documentation link check no longer exempts links to `.py` files.
The rendered visual QA tools no longer need Python. The layout, scroll, feed, page-capture, sprint-graph, CPU and performance gates, the flow/focus/sweep measurements and the timeline captures are Node drivers over one small C WebKitGTK window host; the scrollbar/window-edge check, the common X11 visual audit, the series runner and the rendered image recipe are `cortex-xtask` commands (`rendered-scrollbar`, `check-rendered`, `journey-series`, `rendered-image`); the first-use UI test drives the real native service instead of the removed Python helper. The complete owner journey of the former Python rendered check (Wayland frame and controls, terminal, voice, attachments, project registration, task lifecycle through the no-paid native peer, worker controls, offline states) is `cortex-xtask check-rendered` on X11 and, with `--display wayland`, on headless Mutter; the two Python files are removed.

The rendered visual audit now expects the design's interface face, Noto Sans Variable (it still fails on any other face, and on the bundled code face not loading), and exempts the composer's send control from the light-background rule the same way as the primary buttons, since both are filled with the primary colour. The audit found one defect, now fixed: the command text of a check row on the Changes tab (`.rv-cmd pre`) fell back to the system's bare `monospace` instead of the bundled code face. Run on the dev build, the owner journey also records three failing steps that are product behavior, not harness gaps: the focus after Request changes and Send and reopen lands on the page body, the Stop worker dialog stays open with no refusal shown and no request recorded, and a command a worker runs through the gateway has no Stop command control on its page.

The rendered check's attachment stage now exercises attachments where the host offers them, instead of failing because it had no way to drive them. On X11 and on headless Mutter it drags a fixture file from a separate GTK window onto the New task composer with the real pointer, pastes a PNG held on the OS clipboard by `wl-copy` or `xclip` with the real Ctrl+V, and picks a file through the system file chooser. It then saves the draft, requires the task page to read every file back, starts the task, and requires the no-paid peer's recorded coordinator prompt to carry the exact image and each text file, the store to retain each file by name and SHA-256 with no event carrying the bytes, and the task to complete. When the host reports attachments unavailable the stage is still `not_applicable` with the page's own words.

Workspace separates action acknowledgements from background projection updates, scopes task/sprint reconciliation to their project, and discards reads begun before a newer command reply. Confirmed renames and sprint queue controls display immediately; failures and unknown outcomes are never promoted. Sprint routing reuses held project identities instead of scanning every product. Product/worker facts share one helper startup (`product.facts`, which the desktop bridge now also allows; a build without it showed "The workspace could not be loaded." with `bridge_request_refused`, and a policy test now fails if the real port names any machine operation the bridge refuses) and overlap Git inspection; Git branch/common-directory reads are reused within each product read. Additive format-12 activity and runtime-operation indexes remove repeated JSON scans; non-initializing opens never install them. Attachment validation imports are deferred until needed. See [measured action hot paths and remaining costs](docs/cortex-workspace/performance.md#action-latency-recorded); no installed-desktop latency claim is made.

Workspace loaders move again, so a pending action no longer looks frozen. After 150 ms, the bar under a pressed button sweeps back and forth until the service replies. Loading activity, a report, the workspace and server start show a turning loader beside their words, and a working status turns its loader slowly. Loaders exist only while their work is pending, move only by transform or opacity in 12 steps a turn, and pause in hidden windows and tabs out of view; reduced motion uses a slow pulse. `timeout 30 node packages/workspace-ui/test/webkit-cpu.mjs` now checks that nothing runs when nothing is pending, that a pending button and working task animate, and that a hidden window runs nothing. It records WebKit CPU per state; see [Working indicators](docs/cortex-workspace/performance.md#recorded-on-the-reference-machine). Measured on a software display only, not on the owner's GPU or in the installed app.

Workspace adds Delete sprint… for active, paused and archived sprints. One confirmation names the task count and warns that running work will stop. The service fences queue/task starts, uses the existing task-wide cancellation path, and deletes only after runtime/process quiescence is confirmed. Partial failures retain a retryable stopped sprint. Confirmed deletion disappears immediately without waiting for a project refresh. Only clean, proven app-created worktrees are removed without force; retained paths stay visible, and the project folder and Git branches are preserved. No paid or installed-desktop qualification is implied.

Workspace no longer freezes every 30 seconds and scrolls its boards and lists without dropped frames. The facts poll used to serialize every project's facts to text to compare them, send every project's facts again after a full read, and move a re-read project's records to the end of each list, so the screens redrew the whole sidebar and board: a 1.0 to 1.3 s frame each time on a 31-project fixture. Facts are now compared as data, a full read counts as delivered, and unchanged records keep their place. Board columns and the task list render their rows in normal flow instead of building and measuring cards on every scroll event. A working task's streamed update now redraws only what changed in a long worker feed (it used to throw thousands of JSON errors and rebuild every row each time), and a task title cut in the title band of a started task opens whole on a press. The look is unchanged: captures before and after are pixel-identical except a board column's scrollbar thumb, now sized from the column's real height. A new WebKitGTK scroll gate, `node packages/workspace-ui/test/scroll-perf.mjs <label>`, scrolls every surface by scrollbar drag and the real wheel and fails on dropped or long frames, scroll-time script and DOM changes; its `KNOWN` table names what is not at target yet: the code editor, the first jump across a long never-painted history, and very fast streaming into a 1,500-note feed. See [Scrolling, recorded](docs/cortex-workspace/performance.md#scrolling-recorded). Measured with software rendering, not on the owner's GPU or in the installed app.

Workspace can edit a task's saved title, description, attachments, model/effort/settings and checkout before its first start, including pending tasks in an active sprint. The shared composer retains refused drafts and requires saved files to load before saving; stale edits and edits after Start are refused. New tasks really join the selected sprint, append without changing existing dependencies and inherit its checkout. Close sprint now has a visible, explicit confirmation: it stops the queue and moves the sprint to Archive, keeping unfinished tasks accessible in the project and running tasks running. It never silently accepts, merges, cancels or deletes work. Archived sprint summaries remain readable; closed sprints leave the active sidebar and lists.

At phone width a project's toolbar no longer cuts its tabs ("Cod" with Git, Worktrees, Knowledge and skills and Settings out of reach): it keeps the scope, Board and List or the open area, and a More project areas menu holds the rest. A narrow title band moves a sprint's New task and Close sprint…, and a task's Edit task…, into the band's menu, so the queue control and the window's own controls are no longer pushed out of the window at 820 × 1180 and 390 × 844. The card menu's Archive item states its reason once. `pages-shots.mjs --set review-fixes` captures these and the close-sprint, Archive, running-sprint and edit-task flows at five window sizes in both themes.

Workspace's Code view shows the file's text again in the desktop app. The startup page's inline `<style>` made Tauri add a CSP nonce for styles, and a nonce switches `'unsafe-inline'` off, so the styles the code editor (and the terminal) add at run time were blocked: the editor drew its line numbers with growing gaps and no text. The startup styles are now a file (`public/boot.css`), and a test keeps inline styles out of the desktop page. The startup logo is asked for at the page's first address, so it no longer turns into a missing-image glyph once the interface has moved to another address. A sprint now has four tabs, Chat, Plan, Board and Summary, so the planning chat and the plan each have the whole width; Open plan, the plan's chip, Request changes and Ctrl+\\ move between them. Controls keep the desktop's compact sizes at every width: the 44 px phone and touch sizes are gone from the composer, queued messages and answer choices. A composer that cannot send says why once, in its writing area. Every `?` help button has a fill and an edge, and in a bar it is a control of the bar's height. Menus and popovers stay whole inside the window, including Layers opened from the graph's More menu. The rendered layout gate (`layout-fit.mjs`) now fails a control larger than the desktop size, a help button with no background and a popover outside the window, at 390 px and the window minimum too, and proves each rule can fail.

Workspace labels sit in the middle of their buttons, chips, tabs and badges. The interface face was already within half a pixel; the code face drew its capitals about a pixel and a half high, so a code label in a control or bar is drawn 0.12 em lower. A new rule (`packages/workspace-ui/test/label-fit.js`) holds every such label to the middle of its box within 1 px and to one line, in the rendered layout gate (`layout-fit.mjs`, which also proves it can fail) and in the page captures (`pages-shots.mjs --set polish`, every page at 1920 × 1080, 1280 × 800, 960 × 540, 820 × 1180 and 390 × 844 in both themes), which also measure the drawn pixels and report overlapping text. The Inbox is now the Needs you list for every connected machine, in the same order (questions, permissions, conflicts and planning questions, then results to review, then work to finish, then unmerged work), and each row opens the task's conversation where it is answered or accepted. A board card or list row shows one concise fact (the question itself, the result's check counts, what was accepted and where it goes, or Working without a worker count) and one next action; a running card offers Open, and Stop stays in the card's menu and on the task. Settings and orchestration tables read as labelled rows on a phone. Fixed at phone and compact widths: a bar's left group running under its actions (now on its own line), the sprint summary counts (two by two), cut words in the title band's counts and project path, the settings context bar, the machine switcher's label, blank model and profile picks (now None or Choose a model), the voice settings rows and the New task and New sprint footers. A conversation reader at the end stays at the end when the composer grows, and a row's cut title opens whole after the font arrives.

New sprint now offers the same Project folder or Separate worktree choice as New task. The saved choice appears in Planning and Summary and applies to every task created from the plan, in both manual and automatic mode. Worktree mode uses a separate worktree per task on start; planning never creates one. Older sprints and existing checkouts are unchanged.

Workspace reads as a conversation on every page. A task opens on its conversation: progress is said in words, questions are answered inside it (choosing an option only prepares the answer, and a paper-plane button sends it), and Work details, Changes, Code and Terminal are one press away instead of a permanent plan strip and five tabs. Every message box, from task, answer, worker and planner messages to New task, New sprint, Request changes, diff comments, conflict guidance, stop reasons and the Collect knowledge, Refresh knowledge and New skill requests, is the same composer with Attach and Voice in the same places, model and effort where an engine is called, and the paper plane named for what it does. Where a control cannot be used it stays visible with its reason. Enter sends in the Send now or Queue mode shown beside the plane; Ctrl+Enter no longer queues, and Help and Shortcuts say so. Home shows one Needs you list in which each task waiting on you appears once (its question, a result to review, or accepted work to finish), and engine limits stay in the sidebar and Connections. Sprint planning shows the proposed plan as a line in the planning conversation. Controls standing side by side in one row now share one height everywhere, so no button sits smaller or larger than its neighbour, and the program's name no longer appears in the interface. Below 820 px of window width every page holds the sidebar away, and at phone width no page scrolls sideways: Home, the Archive, the list, settings rows, New task and the planning conversation reflow, and the planning conversation stays readable with Open plan. Rendered WebKitGTK captures from fake data cover these pages at 1280 × 800, 960 × 540 and 390 × 844; this is not native-window, installed-package or phone qualification. See [Conversation adoption](docs/cortex-workspace/SCREENS.md#conversation-adoption).

Workspace's worker activity loads its whole recorded history by itself. Opening a worker reads each available page in turn, so there is no Load older control to press. A failed page keeps the rows already loaded and offers Retry. Rows that arrive live while pages load stay in place, and Follow and the reading position are kept.

Checkout locations are now labelled Project folder or Worktree, separately from the branch name, on the board, task and Workers pages, Git, merge dialogs, New task and settings. The former Main checkout label is gone. The internal checkout kind is unchanged.

The title band's branch chip shows the project's current open branch, not the Git default branch. A long branch name is capped in width and opens in full on a press, as other cut text does; the Open branch setting does the same.

The machine connection notices no longer name the product, and the unconfirmed-task notice reads plainly: the machine is connected but cannot confirm whether the task is running or stopped. Re-check state and Machine settings → Diagnostics are unchanged.

Workspace's queued-message row no longer shows attachments. A file name used to run into Edit and Send now; the row is now one line of text with Edit, Send now and remove in their own box. The queued message keeps its files: Edit returns them to the composer, and Send now sends them, after which they open from the conversation.

Command output, tool output, command output tails and edit diffs in Workspace's steps no longer clip at the right edge. Each is one labelled box that scrolls sideways and vertically inside itself, keeps lines as written and can be reached by keyboard, on the Workers tab and in the Conversation.

A task whose state Cortex cannot confirm while its machine is connected now shows one status, Not confirmed, and one alert notice that says the machine is connected but cannot confirm whether the task is running or stopped. Re-check state rereads that state only; it resumes, stops and replays nothing. The duplicate icons and the reconnect advice for a connected machine are gone. If the state is still unconfirmed after a re-check, the notice says so and links to Machine settings → Diagnostics. A disconnected machine keeps its own notice.

Large sprint plans can be read as a graph. The plan pane has a List and Graph switch (List stays the default and the choice is remembered), and the Graph view has zoom, pan, Fit, Fit matches, a minimap in the canvas corner that appears only while part of the graph is off screen, search with a count and next and previous, a status filter, highlighting of only the picked task's links (needs solid, unblocks dashed, direct or the whole chain), collapsible dependency layers and four levels of detail that keep titles 12 px on screen. Full view, the button or E, fills the window under the title band without changing any panel setting, and Esc leaves it. Every part works by keyboard. Panels no longer float over the plan: when the window narrows, the planning conversation and then the sidebar give way, Details stays docked beside the plan, and the planning conversation has its own toggle (Ctrl+\\) with a chip for a waiting question, planning in progress or an unsent message. The small graph under the list keeps its labels readable or gives way to Open graph, and it no longer lights the first task's links before one is picked. See [the sprint plan screen](docs/cortex-workspace/SCREENS.md#sprint-plan-list-graph-and-full-view-desktop).

Workspace lays out every bar, heading, row, action group and dialog footer with one set of shared layout primitives, so the layout no longer drifts from screen to screen. A disabled control's reason sits in its bar's own reason slot, or, for a control with no slot, in a box that a press or keyboard focus opens beside it, so the reason never changes the control's box or pushes its neighbours. Cut text opens whole with a press or with Enter, and no longer shows a tooltip. The window shows only the Cortex logo until the first read of the machine, an offline relaunch opens the offline workspace, and a worker card opens its live view directly. A sprint's queue rule sits behind a `?` button, the review comment bar is hidden in a project without Git, and the focus line is also drawn on the control that a dialog or panel focuses as it opens.

Workspace text fields grow with their text. The new task and sprint description, the task, worker and planner messages, a free-form answer, review comments and the message fields in dialogs now grow as you type or paste and shrink as you delete, up to 40% of the window height, and only then scroll inside. Send and form buttons stay in the window: the New task page keeps its Save and Start buttons at the bottom edge, and the conversation leaves room for a taller message box.

Workspace has browser-style tabs. Each tab is its own workspace with its own Back and Forward, and anything from the sidebar, a link or the palette changes only the active tab; Ctrl+T, Ctrl+W, Ctrl+Tab, Ctrl+Shift+Tab and Ctrl+1 to Ctrl+9 manage them, tabs can be dragged, and a middle click closes one. A tab keeps what you typed, its scroll, expanded rows, selected worker and terminal while you are elsewhere, and asks before closing with unsent text. The tab list and each tab's route survive a restart. A tab that is not on view runs no timers, polls or animation and reads fresh data when you come back.

Workspace uses far less CPU. Each project's background queue re-read every task result every 2 seconds, about 15% of a core per project with no window open; it now checks cheap file and version stamps first and does a full pass only when something changed. An open window polls every 30 to 120 seconds instead of every 5, refreshes only the projects that changed, and stops polling while hidden. The status clocks tick only while they are on screen, and journal polling backs off when nothing is happening.

The worker and main feeds no longer collapse while you read them. An expanded block or step, and your scroll position, now survive live updates. Tool rows follow Paseo: the same calls are shown, with human names only, never a raw tool name, and a row opens only when there is something to see. A read of a report, source, draft or skill names what it read. Several rows for one tool call are merged into one in the coordinator feed.

Workspace no longer refuses a new task because unrelated tasks in the same project are running, waiting or recovering, including old ACP tasks without desktop cards. Pending, unknown or running Workspace starts likewise no longer block ordinary ACP first prompts. Duplicate starts remain refused for the same task; crashed starts remain unknown and are never replayed. The busy message now says this task was already started. Sprint queue dependency selection and pauses are unchanged; manual Start does not add server-side dependency enforcement.

Tasks working directly on their target branch no longer offer a merge from that branch to itself. Recorded file-write hashes identify work already committed, including older records without a Git baseline and files edited again later by another task. Pending recorded files offer a confirmed task-only commit that preserves unrelated staging; unknown ownership explains the limit and offers explicit closure. Existing Ready records update on refresh without being rerun. Separate-worktree merges and last-fetched push information keep their behavior.

Workspace switches turn with a mouse click again wherever they have no label around them: a model's On and Auto in Settings → Models, MCP servers in Tools, fallback consent, notifications, Autosave and remote access. The painted track lay over the hidden checkbox and took the click, and a checkbox does not stretch to `inset: 0`, so only a surrounding label or the keyboard reached it. The track now lets clicks through and the checkbox fills it. `focus.measure.py` now clicks every switch on every screen with real X input, centre and edge, in both themes; it failed on the 56 model switches before the fix and passes all 92 switches after it.

A started task whose engine has not opened yet no longer shows "Process state unknown" with Stop disabled. Until the runtime records the task, the projection called the execution contradictory, so the launch was invisible, normal Stop was off, and Code's Stop the task first failed with "This session no longer owns the task". A start whose session and owner agree now projects as starting; each engine-launch retry shows the runtime's own notice as the task status, and a give-up shows as needing the owner. Stop cancels such a launch through the runtime's task stop, and a stopped launch no longer keeps the checkout busy. Any other missing or disagreeing fact still projects as unknown, and a restarted service still doubts an open launch. `tests/test_workspace_launch_state.py` and `launch-state.test.ts` cover the projection, Stop and the status line.

Clicks are no longer sometimes lost, and controls no longer redraw under a resting pointer, when the machine is read again. Four places built their rows with a helper called inside the page, so a change elsewhere rebuilt them even when they were the same: the worker cards of the Workers tab at every change to the running task, the machine links of the Settings sidebar at every machine read, the Inspector's Open task at every change to the selected task, and a project's Limits and fallback rows at every machine read. A control replaced between press and release swallows the click, which is how opening a worker sometimes did nothing; a replaced field also lost its focus while the owner was typing. They are now components, which keep their elements until what they show really changes. jsdom regressions keep each of them in place through such a read and still move a worker that finishes into Finished.

A project activated before the dev store moved to `.cortex-dev` now starts again by itself after the upgrade. The earlier build wrote its own ownership marker, so the new service found no `.cortex-dev-owner.json` and refused every automatic restart with `dev_project_refused`; the project stayed stopped after the upgrade, a unit restart and a login. Activation now reserves such a project when it has no `.cortex-dev` yet, since there is nothing to adopt. An existing unreserved `.cortex-dev` is still refused, older markers and `.cortex` data stay untouched, and the desktop's refusal text no longer blames the released `.cortex` store. `packaging/container/check_systemd.py --prior-archive` now activates a project under the prior archive and requires it ready after the apt upgrade, a unit restart and a login, and Rust regressions cover both outcomes.

Resume right after Stop no longer fails with "This task changed on the machine". Stop answered as soon as it cancelled the running turn, and the turn then recorded its end, which moved the task's version past the one the window had just read. Stop now answers after the interrupted turn has settled, at most 10 seconds later, without blocking other requests, so Resume carries the current version. The stale check itself is unchanged. A service regression resumes with the version read after Stop, and the installed journey now waits for the exact Stop and Resume buttons instead of any "Stop" text.

Workspace no longer freezes when a project's runtime is away while its saved sprint plan names tasks. The shared task index looked up every sprint task as if it were loaded, so when an upgrade restart left the project's runtime stopped, every page change threw and no link, Settings or sidebar toggle responded. Tasks that are not loaded are now skipped, and the sprint's Run button waits with "This sprint's tasks are not loaded yet". `packaging/container/check_real_input.py` clicks the installed window with real X server input in a fresh home and in that state, and jsdom regressions cover the index and the Run button.

A crashed workspace service no longer leaves a project's Cortex running. Linux sends the parent-death SIGTERM again each time the killed service hands the child to another of its exiting threads, and the handler's `Event.set` took a lock the interrupted first run still held, so the child sometimes deadlocked in its own handler and stayed alive for ever. The handler now only assigns a flag the main thread polls, in the service child and in `cortex_acp` alike. A regression re-enters the handler at every line of its first run.

`scripts/run_tests_fast.py` now rebuilds the workspace service before the first batch and stops with exit 2 when it cannot, so the suite never launches an older build. Direct pytest runs refuse a service binary older than its sources and name the build command. Owner pause and resume are listed as covered public writers, with a regression for their invalidations and rollback. Debian packaging no longer depends on the builder's umask: under 0077 it failed at `dpkg-deb`, and voice files could have kept private modes. Test cleanup also removes the mode-000 work directory an overlay leaves, which had turned a passing batch into a failure on hosts with working bubblewrap.

Workspace follows one set of interface rules on every screen. Route, connection, report and build identifiers are gone outside Diagnostics, and a version reads as 1.0.6. Labels no longer name the program, and hints are English only, so the help language setting is gone. Titles and names are edited by clicking them. Zero counters are hidden. Left and right gaps are equal everywhere: scrollers keep their track, because WebKitGTK reserved no gutter and rows touched the right edge. Modified now says in words what changed, Tiers read from Light to Flagship, and the Routing table fits its Work kind and Tier columns. Text can be selected and copied with Ctrl+C, and anything cut shows whole in its tooltip. Moving with Tab or an arrow key draws one quiet 1 px line around the focused control, and a mouse click draws nothing and clears it. `packages/workspace-ui/test/sweep.measure.py` checks these rules on every screen in both themes, and `focus.measure.py` checks the line.

Workspace task and board flows follow the owner's review. Several open questions show as numbered tabs and a settled question reads as question and answer. The conversation reads as messages and folded steps, with no machine identifiers or display updates. Plans list titles with Read more, All work groups tasks by sprint, tasks waiting for the owner stand out, and a card crosses to its new column unless reduced motion is on. Every action shows its loader on the pressed control and blocks a second press, and an identical command already under way is never sent twice. A read that says nothing new no longer redraws anything, which ends the plan blinking under a resting pointer. Live data no longer shows a Confirmed stamp, the Inspector names checks that did not pass, and a check row and a checkbox no longer share a shape. Sprint planning keeps its questions across refreshes.

The Workspace rendered journey now records how the desktop process, its web process and the compositor really ended, and it no longer stops when the page replaces an accessibility node it was holding. A real death of any of them stays a hard failure, and `cortex-xtask journey-series` repeats the journey one run at a time.

Workspace Code is now an editor. CodeMirror 6 with pinned versions edits project and task checkout files, with tabs, a dirty marker, find and replace, Save and an Autosave setting. A save writes only over the revision the owner read, so a change from a worker or on disk becomes a conflict with Reload from disk or Keep mine. Saving waits until the runtime shows the checkout's task stopped, and unsaved text survives navigation and asks before a tab or the window closes. Open in VS Code now works from Code and the worktree menu at the exact file and line, falls back to the default application, and Open logs says what to install when no opener exists.

Workspace adds projects the way Paseo does: the empty state, sidebar and palette open one flow with Search for folder, Browse and New folder. The search runs on the selected machine as you type, through a new bounded and cancellable `directories.search` service operation, and marks Git repositories. Choosing a folder adds it, named after the folder. `projects.register` gains `newFolder`. Clone from GitHub now shows its icon instead of a solid block.

Workspace refreshes configuration after first activation and background setup, retains revision-bound dirty drafts for explicit conflict review, and avoids writing local defaults on a refused save. Git commits and amend use the visible message immediately, and disabled Pull names its prerequisite. Sprint planning refuses missing or unavailable models before creating work and keeps the description. Worktree reset, tooltip and example share the service-reported channel default without moving existing checkouts. Empty-HOME helper/UI and fake-engine service journeys cover these source fixes; installed-package/native-window and real-account qualification remain outstanding.

Workspace no longer reports a healthy machine as offline. Any refused request used to mark the machine disconnected until restart, so one wrong path disabled the whole app with "result not known". Now only the status check decides the connection, and it recovers by itself. Projects can live anywhere the owner can write, including other disks, and the Add project dialog gained the system folder chooser, name-from-folder and "Create folder and register". Every refusal shows its reason in the dialog. The service supervises each project's Cortex: a crashed child restarts with backoff, a slow one is no longer abandoned, a missing folder no longer stops the service, and activation no longer freezes it. A project that cannot run names its reason on its own page with Try again. Starting a task with no engine ready is refused with the reason and leaves the task startable, where it used to wedge the project.

Workspace Debian versions now use `1.0.6+z<UTC-microsecond-build-stamp>.sha256.<content-hash>`, above every legacy hash-only version. New builds reserve increasing explicit ordering inputs; cache reuse gets a new stamp unless an identical-input replay is explicitly requested. Runtime content identity remains embedded and unchanged. Reproducible archive checks and Debian ordering comparisons do not constitute a new installed-package qualification.

Workspace dialogs show command refusals inside the modal, retain unsaved settings after a failed save or profile switch, and restore focus when closed. The native journey now waits for review and worker-control dialogs to close instead of treating a new runtime result as the submit acknowledgement; worker-message input still uses real native focus and typing.

Workspace adds local Russian voice input with Ctrl+D/mic start and stop, live composer previews, Cancel, Insert and explicit Insert and send. The package includes locked sherpa-onnx-node and uses bundled Node; a first-use verified model download shows progress. Default-source PulseAudio capture and local inference keep audio on-device; stopping or losing a recording never sends it.

Workspace adds a service-owned local project terminal (Ctrl+J), pinned portable-pty/xterm.js, bounded output/input and explicit shell/job cleanup. Window closure detaches without stopping the shell; service restart never replays terminal input. Closed native/service messages cannot choose executable, cwd or environment. The owner shell intentionally has same-user authority, not a sandbox. No-paid Rust socket round-trips and a no-image WebKitGTK check cover input/output, resize, window reattachment and theme/font behavior; package installation, remote terminals and task/worktree-specific shells are not qualified.

Workspace wire 1.3 adds bounded attachment bytes to task messages and multiline Other answers. Picker, drop and pasted-image input preserve files through queue Edit; the runtime retains private prompt resources instead of filename-only hints. The Linux window keeps webview drops enabled and handles WebKitGTK empty clipboard/URI-only drop behavior through closed native byte snapshots. No-paid resource journeys and a fresh WebKitGTK 2.52.6 binary check cover clipboard paste, an external GTK file drop, queue/Edit and fake-engine resource receipt without screenshots or image inspection. This is not package-install or paid-engine qualification.

Workspace wire 1.2 saves per-task configuration and checkout choices. Task worktrees run against the shared project store and supply their own diff. Integration adds target-branch squash and GitHub CLI actions with verification, dirty-target and ambiguous-PR guards, plus retained source conflicts and owner/Cortex resolution controls. Archive deletion now removes runtime rows through journaled cleanup and attempts space reclamation. Automatic sprint advancement runs in the service child, not the window. Project routing/ceiling overrides and project removal are connected; the project menu supports a native VS Code action. LLM titles remain unavailable. Pre-save MCP listing now connects the draft to a bounded sandboxed listing without saving or calling tools. Open logs opens private rotating status categories, never raw stderr. Large/non-text conflicts expose manual Git instructions, clean task worktrees can be removed immediately after merge, and source updates require fresh result evidence. Sprint routes/targets are inherited at task creation, duplicate-connection routes remain selectable, partial setting failures are explicit, and storage shows failed space-reclamation counts. These source changes are not newly installed-package or rendered-binary qualification.

Workspace conversation now reads durable Cortex history through wire 1.1, including plans and tool activity; owner messages, unsent queue and review drafts survive reopening. Worker prose is retained at tool boundaries without expanding native worker cards. Direct worker messages reach the next successful Cortex tool response; worker and gateway-command stops use existing runtime control paths. Review feedback returns to Cortex, clears stale acceptance and produces a new plain result ordinal instead of displaying the runtime entity version. Check reruns record a mandatory request, not a fabricated pass. Stop remains non-resumable. No-paid fake-engine checks cover the real service, screen and native binary; this does not qualify paid engines or a newly installed package.

Workspace Debian packages now include locked Codex/Claude adapters, their dependencies and checksum-verified Node 24.14.1. First activation needs no npm or adapter download; system Node/npm are no longer package dependencies. Discovery prefers the channel bundle and can relocate an existing same-version connection without changing account references. A no-network Ubuntu check installs the package and initializes both real adapters through Cortex without a prompt. On 2026-11-06 the owner reviewed Anthropic's published terms and chose public distribution with native-account login unchanged, accepting the unresolved authentication interpretation risk. This is not Anthropic permission or a legal conclusion; see the [licence inventory](docs/project/bundled-engine-licenses.md).

The real Workspace app now shows a task the way the accepted design does. The title band and plan bar name the real state and runtime phase, the Inspector and the worker pane are on, workers appear in the conversation, and Runtime verification reports the result's own checks. The Queue no longer sends straight away: a queued message waits in the app, Edit puts it back in the field, and Send now delivers it. Questions asked as a batch can be answered one by one and go to the machine together. A new task shows where it works and where it merges, and can start at once. A message to a worker goes through Cortex, and the field says so. Stop says it ends the run, because a run cannot be resumed yet. A task that has no route of its own shows what its machine sets for every task.

The Workspace desktop app now opens every screen of the accepted design on real data instead of "Not connected in this version". Code and Git read the real repository, with stage, commit, fetch, pull, push, branches and worktree removal. Workers come from the project store. Orchestration, Connections and Files edit real Cortex settings through its configuration API, with revision checks. Tasks can be stopped, steered, renamed, accepted, merged into the open branch, closed, archived, restored and deleted from the Archive. Sprints run a Cortex planning task, create their tasks from the plan and can run them in order. The Cortex logo is now the window, dock and launcher icon. Still off, with the reason on the control: the terminal, voice input, attachments, remote machines, per-task model choice and per-task worktrees.

The Workspace desktop window now looks like the accepted design. It has one title band with working minimize, maximize and close, a drag region and edge resize, instead of an OS frame above a fake button group. The page fills the window with the theme surface from the first paint, the split button no longer stretches across the band, the setup notice no longer pushes the band down, and Roboto and Cousine ship with the app. A new rendered audit measures all of this in the real binary on X11 and Wayland and fails on regressions.

Workspace Debian packages now bundle the complete Cortex runtime and host for both isolated channels. Units no longer need a checkout or environment file. Login startup, bounded native app-driven service start/retry, best-effort upgrade restart and removal stop replace manual service setup. First activation runs the existing native-account onboarding, and the desktop shows engine setup/login diagnostics separately from service connectivity. The Docker installer smoke checks normal-user activation, dependency installation, legacy archive upgrade and data-preserving removal; systemd-manager and sandbox gaps are not treated as passes.

Cortex Workspace, a desktop application with a user-level machine service, has its local vertical slice built and checked without paid calls: a real Ubuntu window in a container build, a closed local service with a `_cortex/workspace` adapter in Cortex, identified question answers, explicit starts and read-only results. The slice is not released. Isolated dev and release channels, a dev settings clone and a refusal rule for unowned project stores are included; manual GitHub Actions runs on `main` publish production as Latest (`v<debian-version>`), while `dev` runs publish a separate dev prerelease (`dev-v<debian-version>`) explicitly excluded from Latest. Both share one release list; dev installs manually and has no in-app updates.

Paseo 0.15.0 is qualified for the CLI runtime and protocol behavior exercised by the owner-requested paid `cli` checks, through the isolated dev Paseo daemon and disposable projects only, on `codex-acp/gpt-6-luna:high`. A lifecycle run with mandatory independent review and verification passed; a model remark on a Git-initialized fixture passed with the unchanged-project review waiver; and a user question whose answer resumed the run passed. A first model-remark run used a non-Git fixture that did not satisfy the review-or-waiver baseline; that failure is retained in the dev environment guide and is not attributed to Paseo 0.15.0. This release changes no runtime behavior and records the CLI qualification and the package version bump. It does not establish rendered desktop/GUI behavior, release readiness or qualification of other Paseo versions.

## 1.0.5 (unreleased)

A worker that published its report but ended its turn without `result_submit` is now shut down and released. Before this, the runtime recorded that report as the worker's result under the assignment key instead of the attempt id. The finished-worker shutdown then tried to stop an engine under the assignment key, matched nothing and never retried, so the worker's engine stayed live. Every later close-out stopped with `workers_still_live_or_unknown`. A live Codex worker held a finished task this way for about three and a half hours. The shutdown now resolves that key to the provisioned attempt; a key with no attempt is not stopped. That stop question now names each worker still live or unconfirmed by node id, role, engine and observed state, in English and Russian. This does not establish live qualification.

A failed automatic model discovery now says why. The per-engine first-run status and the Settings engine row keep a fixed failure code instead of only `catalog_failed`, never adapter output. A new code, `engine_home_read_only`, names an engine account directory such as `~/.codex` that the Cortex process cannot write; Codex app-server exits at startup then. A fresh dev install hit this when its daemon was started inside a Cortex command sandbox that mounts `~/.codex` read-only; seeding the default profile and disabled MCP examples did not cause it, and model discovery never passes MCP servers to an engine. The guarded dev wrapper now also refuses to start the daemon or GUI when `~/.codex` or `~/.claude` is mounted read-only. This does not establish live qualification.

Connections no longer replaces its form with a loading placeholder every three seconds. Passive refreshes keep row identity/order, draft inputs and removal confirmation, skip structurally identical data, and still update live status and newly registered connections. Failed refreshes retain visibly stale data with retry controls instead of hiding rows; save/conflict feedback is preserved. These changes do not establish rendered desktop qualification.

Fresh installations now show one selected, applied profile named `default`, not just default-looking routing values. The bundled preset also registers disabled `codebase-memory-mcp`, `playwright` and `vivaldi` examples using placeholder paths and no environment secrets. Settings shows and can edit/toggle these entries; no MCP executable is downloaded or launched by seeding. Existing settings, MCP registries and saved profiles remain untouched, including malformed files. Custom settings are not relabeled as an applied default. Inactive OpenCode model examples were removed to match the current portable owner baseline; OpenCode remains manual and disabled. This does not establish live or rendered desktop qualification.

Claude plan usage no longer uses the legacy session / 5-hour window. Weekly and scoped windows still drive quota bands, routing eligibility and reset times; older session events/caches are ignored and weekly-only CLI output is valid. Settings help and current window display follow the same policy. Missing usage remains unavailable, never inferred hot. Codex windows and actual engine-refusal handling are unchanged. This does not establish live qualification.

A coordinator that answers a simple question in plain text instead of calling `answer_submit` is now told why its reply did not count. Before this, the runtime resent an identical prompt twice and then stopped with `structured_proposal_missing`. A live Codex coordinator hit this after the user asked to "answer only the number" and "not use tools". The second and third planning or reconciling prompts now start with a repair note: the text was not recorded, so the turn must call the named structured tool. When `answer_submit` is offered, the prompt also says that even a one-word answer goes through it and that a user's no-tools request does not forbid it. This does not establish live qualification.

Plugin activation and ACP startup now run automatic first-run setup in the background: install the qualified pinned Codex/Claude adapters, connect an existing native account login when its marker exists, and load models immediately without a model prompt. Settings shows progress and failures while keeping manual actions available. Removing a connection records a persistent per-engine opt-out so startup cannot recreate it; explicit Add connection can restore it. Registering from Settings also refreshes models immediately; a discovery failure keeps the connection and previous catalogue. `CORTEX_ENGINE_UPDATE=off` disables the automatic pass. OpenCode stays manual and API keys remain command-line only. These changes do not establish live or rendered desktop qualification.

First-run setup in Paseo is more complete. Each Cortex agent start now creates a missing `routing.json` or `defaults.json` from the bundled default preset and never replaces an existing file. Paseo Settings shows each engine's status, can download Cortex's pinned `codex-acp` or `claude-agent-acp` adapter, and can add a native-account connection after an explicit opt-in; API-key connections remain setup-command only. The default-preset card applies the preset to a missing file, or resets both user files after confirmation with a private backup of each. The configuration, packaging, security and engine pages describe these paths. These changes do not establish live qualification, paid-model behavior or rendered Paseo GUI behavior.

A Git install of the Paseo plugin into a second Paseo instance with its own `XDG_CONFIG_HOME`, such as the isolated dev daemon, no longer fails in its build step. Setup now writes `paseo-plugin-launch.json` into the Cortex config root of the environment it runs in (`$XDG_CONFIG_HOME/cortex/`, or `~/.config/cortex/` without a custom `XDG_CONFIG_HOME`), so each instance has its own record and the owner's existing record is never replaced. Previously every instance shared `~/.config/cortex/paseo-plugin-launch.json`, and the build step refused to overwrite the other installation's record. The provider no longer falls back to another instance's `~/.config` record. A relative `XDG_CONFIG_HOME` is refused. The guarded dev wrapper's new `--installed-runtime` flag runs a Git-installed plugin from its staged runtime instead of the checkout, and the wrapper refuses Git installs and updates without that flag. These changes do not establish live qualification.

In Paseo, choosing a Cortex model without effort levels, such as Claude Haiku, no longer fails to start the chat with "ACP session does not expose thinking configuration" when the composer still holds a thinking choice from another model. The plugin drops a thinking choice the selected model does not offer, so Cortex keeps that model's own effort; an offered effort is applied as before. This does not establish live qualification.

Settings' **Download engine** and the background engine update now install the same adapter: the manifest pin, raised to the current qualified codex-acp 2.3.0 and claude-agent-acp 0.97.0. Previously Settings installed the stale pins 2.0.1 and 0.78.0, while the background update downloaded npm's latest, unqualified releases, which can never launch. A sandboxed engine whose Cortex-owned install lives under `/var/tmp`, as with an `XDG_DATA_HOME` there, now starts: the jail's private `/var/tmp` hid the install, so every Codex task launch failed with `EngineProtocolError` while model lists still loaded. The verified install is now mounted read-only into the jail. These changes do not establish live qualification.

Routing settings now offer each exact model's observed effort levels for effort
pins, `effort_floor`, `max_effort` and ordered `effort_ladder` values.
Unknown models use a marked global fallback, while stored unsupported values
remain visible and unchanged. These changes do not establish live qualification
or release readiness.

## 1.0.2

This release refreshes Terra routing preferences in the fresh-install default
preset, adds exact support for codex-acp 2.3.0 and claude-agent-acp 0.97.0,
and automatically follows newer approved installed adapters. Set
`CORTEX_ENGINE_FOLLOW=off` to opt out; the startup environment handling is
corrected. These changes do not establish live qualification or release
readiness.

## 1.0.1

Since 1.0.0, orchestration and recovery fixes improve steering, worker
visibility and evidence handling. In Paseo, the busy-steering acknowledgement
is now kept within its own message boundary, so it cannot consume or hide the
following worker-card update; worker cards remain visible while their workers
continue. The release also includes the routing, lifecycle, storage-inspection,
qualification and packaging safeguards detailed below. These changes do not
establish live qualification or release readiness.


A model requirement naming a dotted identity now routes to that identity. A
period inside a token (`codex-acp/gpt-6.1-sol`, `AGENTS.md`, `1.0.0`) no longer
ends a sentence, so `All audits and reviews must use codex-acp/gpt-6.1-sol.`,
the same identity in backticks and the bare `gpt-6.1-sol` are read whole in
instruction files, user messages and chosen question options. Previously the
rule split into fragments that named no model, granted nothing, and the
mandatory reviewer ran on another model. A question about a "must use"
requirement ("Do reviews have to use Sol?", "Почему аудит должен использовать
Sol?") now requests nothing, like the passive forms; negations, alternatives
lists and remarks still grant nothing, and ordinary sentences such as "Use Opus.
Then review." still split as before, including after closing Markdown emphasis
("**Use Opus for the review.** Never use Sonnet." still requests Opus).

Selected recheck options now retain the user's model at explorer dispatch:
`recheck`/`перепроверить` and commit-list/history work objects are recognized,
without assigning a model to unrelated role purposes. Previously an explicit
choice could be dismissed as an unconfirmed hint, starting a cheaper worker
that then had to be cancelled. Negated, past-work, unselected and ambiguous
options still grant nothing. Mandatory-review regression checks also ensure
project model requirements override the opposite-provider preference before
engine open, or refuse without a substitute when hard policy denies the model.

Orchestration audit fixes tighten path reservations, dropped-check certification,
worker-question continuation and consultant access. Model routing now requires
recorded user consent; storage inspection remains non-mutating and recovery
failures stay visible. Role guidance and generated profiles are synchronized,
and cancellation and installed-Paseo regression checks are corrected. These
changes do not establish live qualification or release readiness.

A model requirement in the project root's own `AGENTS.md` or `CLAUDE.md` now counts
as the owner's model request for the role purpose it names, with the same precedence
as a recorded user request: ahead of the coordinator's hint, pins, owner preferred
models and usage, and still refused visibly by hard owner policy, unavailability or
exhausted quota. Previously such a rule reached routing only as an unconfirmed
hint, so a re-audit required to run on Opus ran on Sonnet under the owner's review
preference. Dispatch and the plan preview record it as `project_instruction` with
the file path and line; a newer user message outranks it, prohibitions never grant,
and a sentence naming no role purpose grants nothing. The request grammar also
recognizes passive requirements ("must be performed by Claude Opus", "аудит должен
проводить Opus") in user messages and instruction files alike, but not questions
about them ("Does the audit have to be performed by Opus?"). A model in a sentence
covers only its own work, so "Implementation must be performed by Sonnet and
reviewed by Opus" never gives reviewers Sonnet, while the work named before the
model stays its purpose ("Проведи ревью на Opus и обнови документацию", "Run all
audits on Opus before deploying"). A list of alternatives ("Audits must be performed
by Opus or Sonnet") permits a choice and demands none of them. Only the file as it
stood when the task started counts: a file changed during the task, including by
the task's own workers, grants nothing for it, and the dispatch decision and plan
preview show `changed_during_task` with the file path. Newer imported history no
longer hides an older user refusal of the file's model. Regression coverage also
checks an unhinted runtime-created reviewer: the project requirement beats the
opposite-provider preference, and a hard-denied required model refuses at the
dispatch boundary instead of selecting a substitute.

Recorded user model requests now also recognize restrictive selections
("Выполнить аудит только Claude Opus", "только Opus", "do the review only on Opus")
and an exact identity in parentheses after a model name. A real user answer that
is exactly one of its question's options grants the single model that option
selects, in plan preview and at dispatch alike. It covers the work the option names,
or else the purpose its question asks a model for, for unhinted review nodes; a
sentence judging a result never supplies that purpose. A bare model name, alone or
after a lone verb of choice ("Выбрать Opus", "Pick Opus"), grants only under a
question that asks which model to use (including "Какой модели поручить
аудит?"). An option that only mentions a model
("Принять результат аудита Opus", "Opus исчерпал квоту"), such a name chosen
under "Чей результат аудита принять?", "Чью проверку использовать?", "Какой
аудит выбрать?", "Какой выбрать аудит?" or "Какой в итоге выбрать аудит?", question text, unchosen options,
coordinator `model_reason`, remarks, negations and imported history still never
grant. "Какую выбрать на аудит?" and "Which one should we use on the audit?" ask
which model to use, and a parenthesized usage note ("Opus (квота 52%)") does not
stop an option from selecting its model. Quota and limit words are not refusals:
"Сделай аудит на Opus в пределах лимита" still requests Opus and revokes no earlier
grant. Hard owner deny, unavailability and exhausted quota still refuse without
substitution.

Plan usage is now only the last tie-breaker between otherwise equal routes (rank
version 15): the 10-point "meaningfully less used ranks first" step is removed, so
usage no longer outranks engine affinity or retry diversification. A route the
coordinator names that is eligible is kept whatever its usage, unless its known
usage is at least 95% while another candidate is not hot; the replacement is
recorded as `ignored:near_quota_limit`. Owner preferred models for the work kind
still outrank a hint naming another route, and the plan preview now reports that
(`ignored:owner_preferred` with the `owner_preferred` routes) instead of
`pending_dispatch`, so a legitimate choice among the owner's models is not
treated as a mismatch. Explicit user requests keep precedence. `list_route_options`
still shows quota bands and used percentages. New audits drop `usage_band` and the
usage `margin`.

The default install preset now matches the refreshed production routing values:
Sol's `preferred_when` is `null`, so its configured work-kind preferences no longer
need an exact route hint and remain active on quality retries. At equal quota
without a provider preference, standard generic design/frontend work now selects
Sol instead of Sonnet. Affinity-bound frontend design still prefers eligible Opus,
and quota pressure still applies. The portable Sol coordinator now uses medium
effort from the saved `default` profile, as confirmed by the owner; Russian language
and the schema envelope are unchanged. No local identities or session bindings
are copied, existing installations are not overwritten, and no live qualification
is claimed.

The built-in default preset (`policy/model-settings-default.json`) was previously synced from the
owner's edited production `routing.json`: `claude-agent-acp/opus[1m]` drops the `standard` tier
entirely (now only `flagship` and `deep`) and its `preferred_for` regains `implementation_backend`
and `implementation_frontend` alongside `design`, `planning`, `implementation_frontend_design`,
`debugging` and `analysis`; `codex-acp/gpt-6.1-sol`'s `preferred_for` gains `testing`,
`investigation` and `writing`. Because Opus no longer configures the `standard` tier, standard-tier
design and implementation work -- hinted or not -- no longer reaches Opus (an explicit Opus hint on
standard-tier work is now ignored on tier mismatch rather than ranked against the owner's
preference) and falls to the owner's Sonnet/Sol preference instead; Opus still serves those kinds
once work is eligible for its `deep`/`flagship` tiers. The complete routing values
matched production at that time; no other preset field changed in that sync. No live or paid qualification is
claimed for this change.

The built-in default preset was previously synced from the owner's edited production `routing.json`:
`claude-agent-acp/claude-sonnet-5` also served the
`deep` tier alongside `standard`, and `claude-agent-acp/opus[1m]`'s `preferred_for` narrowed to
`design`, `planning`, `implementation_frontend_design`, `debugging` and `analysis`, dropping
`implementation_backend`, `implementation_frontend` and `review`; an explicit Opus hint on plain
backend/frontend/review work was no longer honoured and fell back to the owner's Sonnet/Sol
preference. Matching production routing values at that time; no other preset field differed. No live
or paid qualification was claimed for that change.

Coordinator-submitted final answers now show the structured “Not verified”
disclosure only once: the runtime removes a duplicate coordinator-authored
section with the English or Russian heading before appending its canonical list.
Both synthesis and planned-work answer submissions use this path; runtime-composed
synthesis is unaffected.

Paseo now keeps shell output when command input arrives in a later update, while
native reads remain metadata-only. Native Edit/Write fields are aliased for
presentation only, and worker-card projections preserve the first terminal
status across contradictory updates and replay. Qualification assertions now
require exact identity-bound confirmations and complete execution/output
metadata (not output-body verification); steering continuation stays pending
until matching capture and same-attempt progress are observed. Owner preferences
continue to suppress unrelated usage ranking when preferred routes disappear
from the catalog. Removed temporary qualification inspection helpers that could
print unrestricted payload text. No live or rendered Desktop qualification is
claimed by these no-paid changes.

A worker that steering left running no longer ends as Canceled in Paseo while it keeps
working. Paseo sends `session/cancel` before each follow-up prompt; the cancelled prompt's ACP
feed silently dropped every later update of the worker cards it had created, so the worker's
real outcome (and a lone Stop's cancellation) never reached the card and was never journaled.
`worker:` card updates now outlive their turn's cancellation; the turn's other updates still
stop with it. The Paseo 0.11.4 bridge settles a cancelled turn's open tool calls as `canceled`
(0.9.1 used `failed`); the provider's worker-card fence now withholds that settlement too.
Coverage is no-paid unit/provider tests plus a provider run against the bridge bundled with the
installed Paseo 0.11.4; no live, paid or rendered Desktop check is claimed.

The built-in default preset (`policy/model-settings-default.json`) is synced again from the
owner's slightly edited production `routing.json`: `codex-acp/gpt-6-astra`'s `preferred_for`
is `["design"]` once more (the generic design/architecture work kind), and `claude-agent-acp/opus[1m]`'s
configured `tiers` list is reordered strongest-first (`flagship`, `deep`, `standard`), matching
production byte-for-byte. Astra's tier membership stays flagship-only (never a deep-tier automatic
candidate) and `implementation_frontend_design` remains Opus-only; restoring Astra's generic `design`
preference does not change either rule. No other preset field differs from production. No live or
paid qualification is claimed for this change.

Coordinator `plan_submit` no longer refuses legitimate plans on the first attempt for the
patterns retained in real sessions. A read-only task may name a not-yet-existing literal path
when a writing task of the same plan whose scope overlaps it is ordered with it (a reviewer of
the directory its predecessor writer creates, or a task whose dependent writer creates the
target); an unordered or absent writer is still refused with `read_only_scope_not_writable`, now
listing every such path in `violations`. JSON `null` for an optional task field (`model_reason`,
`complexity`, `work_kind`) is treated as omitted. An unexpected field that the tool advertises
elsewhere (such as `verification` inside a task) gets a correction naming its location. The
advertised scope and optional-field descriptions state these rules. No live or paid
qualification is claimed for this change.

Implementation routing now distinguishes `implementation_backend`, `implementation_frontend`
(without design), and `implementation_frontend_design`. The optional plan-node `work_kind`
overrides role defaults and survives retries/fixes. Legacy `implementation` preferences,
standard allowances and affinity expand in memory without rewriting owner configuration.
The built-in preset makes `codex-acp/gpt-6-astra` flagship-only and lists frontend-with-design
only in `opus[1m]` preferences. Frontend-design automatic routing now preserves
structured preferences across quality retries, escalation and review fixes,
including eligible fallback tiers, independently of conditional hints.
Unavailable/exhausted preferred routes first fall back on the work kind's
affinity engine (Claude in the preset). Owner correction: only once that
engine has no authorized, available, non-exhausted route at all does selection
widen to the normal cross-engine candidates (Sol/Astra on Codex in the
preset); while the affinity engine still has one eligible route, no other
engine is ever chosen, including on retries, escalations and review fixes. No
route anywhere, or a neutral affinity, refuses visibly. Cross-engine pins are
always refused, never substituted, regardless of this fallback. Confirmed user
model requests still take precedence; hard authorization, quota and
tier/effort policy remain enforced in whichever engine pool applies. Rank
version 12 records the required affinity boundary and whether cross-engine
fallback occurred (`affinity.cross_engine_fallback`). Other work kinds retain
their existing conditional-hint rules. No live or paid qualification is
claimed for this change.
The Paseo plugin's Settings screen already drew its work-kind vocabulary, labels, affinity
choices and preferred-work switches from the runtime's configuration API, so it shows and edits
the three new kinds without a protocol change; its own static help text for the old
`implementation` kind is replaced with one entry per new kind, and the affinity help names them
by their exact ids. Settings edits the routing file's own raw rules, not the expanded effective
view, so saving an unrelated change keeps a stored legacy `implementation` affinity, preference or
standard allowance exactly as written. Such a stored value is shown as a checked
`implementation (legacy: …)` choice that the owner can turn off explicitly; it is never offered
for new selections.

A fix node created after a `REQUEST_CHANGES` review no longer keeps a read-only source role. The runtime gives it a writing role chosen from its effective scope: `worker-technical-writer` for documentation-only scopes, `worker-qa-engineer` for tests-only scopes and `worker-backend-dev` otherwise. A failed check of a worker's own standalone Python probe can be retracted by a later verified passing correction with the same name, operation, cwd and profile; the failure stays as `retracted` history, never as a pass, and runtime verification failures are never retracted. An `outside_command` identity now includes its profile, so an exact outside rerun closes its own failure, and a later unrelated recorded run no longer makes that rerun ambiguous. No live or paid qualification is claimed for these changes.

The 2026-10-14 orchestration audit's owner decisions are recorded in
[the decisions log](docs/project/decisions.md). Read-only roles, reviewers and
automatic runtime verification now run commands in one read-only host profile (receipts name it `read_only`) in every access
mode: HOME is read-only except an explicit tool-cache allowlist, scratch is private, and the
command has its own PID namespace. Inside a verification check only the project's git-ignored
directories are writable. Writing workers keep the access-mode profile. A reviewer's read-only
pass closes a writer's declared gap again in every mode. A mode switch no longer stops a read-only command or a verification check; only a host-writable
writing-worker command is stopped. The deletion classifier is lexical:
it recognises more commands than `rm`, and its tool text says so. No paid or live
qualification is claimed for these changes.

The runtime stop question's Finish now force-finishes the task: it ends `CANCELLED` with
`USER_FORCE_FINISHED`. Continue resumes the flow and never releases the stop. The stop question is recognised by a
structured runtime marker, not by its wording, so a coordinator question worded the same way never
force-finishes the task. A direct answer
that would close planned work which did not pass review and verification ends `FAILED` with
`answer_not_certified:<gap>`, never `COMPLETED`. Only an explicit `deny-write: <path>` creates
a write denial; a natural-language request such as "do not modify X" creates none, and a
wildcard value is not stored. A denial that an earlier runtime stored with a wildcard still guards
writes, matched as a glob. Denials match after path normalization, so a `./` spelling no longer
bypasses one. The outside-write refusal also resolves real paths and protects a worktree's git
metadata directories.

Automatic routing honours a user's model revocation in live dispatch (`revoked_by_user`).
Infrastructure failures (engine and transport failures), and workers lost to a runtime restart or host disconnect (`worker_lost`), retry without escalating the model
tier. Rank version 11 ranks a known usage at or above 95% behind every non-hot route, known or
unknown. Expired quota windows are pruned or expire, and a stretch exclusion no longer removes a
dual-tier model. A pinned effort that the policy does not permit is applied at the nearest
permitted step and recorded as `pin_effort_clamped`.

The senior consultant is reached only through `escalation_request`. The runtime builds its
packet from the reports the escalation names and the escalated revision's reports, each with its
SHA-256 and artifact versions. The consultant receives no `ask_question` or `access_request`, and
a plan that names it is refused.

In Paseo, a `session/cancel` followed by a `session/prompt` on the same session is steering: the
cancelled coordinator turn ends, the task is not paused, and running workers continue. A lone
cancel still pauses the task after a 5-second grace window; the pause notice appears on the next turn, and a failed deferred pause is recorded as a progress event. The Paseo plugin answers the bridge's
`session/close` locally, and the dev wrapper strips inherited `PASEO_*` variables.

The brokered MCP relay's handshake token is no longer in process argv or its environment. The
broker writes it to a 0600 `relay.token` file in its private socket directory, and the relay reads
it there without following symlinks. Claude's `--mcp-config` had carried the previous environment
entry on its command line.

A worker's first or continuation prompt carries its assignment objective exactly once. The
recovery capsule keeps the full brief verbatim and points to it from its other copies. Role
instructions delivered to engines omit generated skill frontmatter and footer comments. The
prompt generator asserts 23 profiles and fails on a dropped preamble. Skills no longer copy tool
argument contracts; the live tool schema is authoritative.

Paseo's plugin dependency `shell-quote` moved to 1.12.0, which fixes a critical advisory. Fifteen
high and five moderate advisories remain, transitively through React Native, and are recorded in
the dependency audit. Compact argv launches fall back to an unlinked temporary file where
`os.memfd_create` is unavailable.

Project file tools now follow the read-before-edit discipline of native editors
and refuse the partial-read overwrite that truncated a shared research file. A
text `project_read` pages by 1-based line (`offset` and `limit`, at most 2000
lines each; `line_numbers` defaults to true), keeps each result under 24,000
bytes and marks lines longer than 2000 characters as truncated. Offsets are
line numbers now, not character offsets, so callers that passed character
offsets must change. A truncated line never counts as read. Complete,
non-truncated coverage of a file is kept per channel as digests and line
ranges only, and `full_content_sha256` is returned only after that coverage.
`project_write` replaces an existing file only with that complete-coverage
digest: the `sha256` of a partial page identifies the page and never authorizes
an overwrite, and a supplied hash alone does not either. Creation still uses
`absent`. Receipts report previous and new bytes and lines plus a
`drastic_shrink` flag; a shrink is visible, not refused. The new `project_edit`
applies one to 64 ordered exact `old_string`/`new_string` replacements
atomically: each must match uniquely unless `replace_all` is true, an append
replaces a unique tail anchor, any prior text read on the channel with an
unchanged digest is enough, and the receipt carries a bounded diff. Own read
coverage is cleared after every mutation. The write path fsyncs the temporary
file before the rename and the parent directory after it. Focused gateway tests
are in `tests/test_project_file_tools.py`; no live or rendered qualification is
claimed.

Paseo's live chat echo for an answered Cortex question now reads
`Question: <question> → Answer: <answer>`, with the batch topic label in front
for a labelled batch member. Single, free-text ("Other...") and batch answers
each get their own line. Only the echo text changed: the recorded answer and
the durable replay event are unchanged. Unit tests cover the text; no rendered
GUI check was run.

The no-paid suite can run inside Cortex's verification sandbox without making
lifecycle tests depend on nested user namespaces. Test-only launch seams keep
real command outcomes and retention; actual bubblewrap tests probe once and
skip known namespace denial explicitly. Production containment stays fail-closed
with no unsandboxed fallback.

Claude ACP now has an exact 0.88.0 instruction-delivery registration for its bundled SDK 0.3.293. Discovery prefers the newest registered adapter and pruning preserves registered installs, so a background engine auto-update that installs a newer adapter without a registration no longer displaces the usable registered version. Critical dispatch rejects unsupported or non-retryably unqualified engine/version pairs rather than retrying another model on that adapter. Qualification is static/source and no-paid protocol evidence, plus one owner-requested paid `desktop` check on 2026-10-08 (`claude-agent-acp/claude-sonnet-5`, adapter 0.88.0): its resumed scenario completed without `instruction_delivery_unqualified`. The first canary attempt was operator-cancelled, one version diagnostic assertion remains an open failed check, and the native agentInfo version is unverified. Paid CLI mode was not run (not requested by the owner). This is bounded evidence, not clean or full qualification, paid native-tool enforcement or release readiness.

A worker that published its result and ended while a scheduler pass was
running is no longer mistaken for a lost worker. The pass read the plan nodes
before the live worker set, so a reviewer finishing in between looked
"running" with no live worker; `retry_lost` then failed with
`lost_worker_result_changed` and returned an approved task to the coordinator
for an extra plan revision (the intermittent composed-runtime test failures
under load, where an escalation's consultant node vanished from the final
revision). The live worker set is now read first. No paid/live qualification
is claimed.

Paseo's model Refresh now refreshes the Claude catalog too. Paseo reads models
from a Cortex ACP process it stops about a second after `session/new`; the
refresh started there ran on a thread of that process, and a Claude refresh
(one effort probe per model, about 12 s) was killed every time, so no Claude
snapshot was ever saved by Refresh. The refresh now runs in its own process
session (`catalog/detached_refresh.py`, one per connection at a time) and
saves its snapshot after the requester exits. The isolated dev profile's
stale routing (bare `haiku` denied) was updated to the current preset after a
private backup; its picker now lists Haiku 5.5 at Luna parity. The owner's
production runtime predates the `+sdk` build ordering and repoints Claude to
the upstream SDK 0.3.287 install at each start, where `haiku` is Haiku 4.5; it
needs the current runtime deployed. No paid/live qualification is claimed.

`result_submit` now takes `report` as one plain string: the complete Markdown
(optional `report_title`, else its first heading) or only the exact `report_id`
from the worker's own `write_report`. The former `report: {title, markdown}`
object made Claude engines hand-escape long nested Markdown; Sonnet 5 broke it in
about a quarter of large inline reports and Claude Code refused those calls
before Cortex received them, which was the recurring "submit result error".
Object values and identifier-shaped strings that are not an exact report id are
refused with a corrective `invalid_arguments` before anything is published. The
advertised check contract and `open_failed_check` correction now say that a
mistyped command line that never started the check is not a failed check (cite the
corrected command's own receipt); exact-argv closure is unchanged, so a different
argv still never closes a cited failure. No paid/live qualification is claimed.

The `result_submit` schema's `checks[].outcome` field description and the
top-level `result_submit` tool description worded that mistyped-command
guidance with "unknown-option"; under Codex code mode that description text
renders verbatim inside the generated TypeScript declaration, so the literal
substring "unknown" appeared there even though every field and description
otherwise survived intact. Both descriptions now say "unrecognized-flag"
instead, with no change in meaning. No paid/live qualification is claimed.

Codex ACP now supports exact 2.1.1 private-adapter patches, instruction delivery,
compaction and discovery, with a 2.1.1 fallback. Role metadata reaches all
qualified versions, not only 1.12.0; diagnostics report the current registrations.
The owner already had 2.1.1 installed (CLI 0.159.3), so the update path made no
reinstall. No-paid bundle and CLI/jail checks passed; no production Paseo/plugin
refresh or paid/live qualification is claimed.

Claude ACP now has an exact 0.87.0 system-instruction registration and fallback,
observed/grouped model-option handling, diagnostics, and reactive compaction
recognition for its new non-AIR metadata. Unknown installed Claude versions no
longer borrow another release's qualification. The original SDK 0.3.287 only
reports Haiku 4.5; the independently inspected SDK 0.3.293 refresh now installs
side by side and exposes `haiku` = Haiku 5.5, with old Haiku separately named.
No-prompt ACP controls accepted high/xhigh/max. The default preset and owner's
active production profile enable the new alias at Luna policy parity, associate
`seven_day_haiku`, and deny old Haiku by exact ID. Owner settings were backed up;
unrelated settings and the original install were preserved. No paid/live
qualification is claimed.

Model policy gains an optional owner-written `preferred_when` condition. The
coordinator sees it in `list_route_options.preferred_when` and names the model
on the plan nodes that meet it; only there does it rank as the owner's preferred
model. Settings now choose `preferred_for` and `also_standard_for` work kinds
from the list instead of free-text entries, which the runtime always rejected.

Orchestration now records coordinator/engine timing milestones and changed
scheduler wait reasons, with a read-only phase timeline for checkpointed stores.
Review can start while accepted worker engines wind down; unchanged-file digest
hashes are reused where filesystem metadata supports it. Sandboxed verification
may overlap a running reviewer, but only an unchanged approved binding can credit
its result; stale candidates are discarded and interrupted work is rerun through
ordinary verification. Engine prewarming remains deferred pending live timing
evidence; no performance gain is claimed.

Routing now honors an explicit user-requested model before other routing
preferences, subject to hard authorization and availability checks; an unusable
requested route is refused visibly rather than replaced. Usage-based model
switching activates only at observed usage of 95% or higher; unknown usage is
unavailable. Fresh installs now receive the owner's portable default routing,
coordinator preference and language, while existing settings and machine-local
account, connection and MCP bindings are not replaced or copied.

Added opt-in developer wire tracing for the isolated dev Paseo daemon. It records
redacted ACP, MCP, verification and lifecycle frames to private, capped files
outside project storage; it is off by default and refuses installed production
state. Trace data can still include sensitive content and must be treated as
confidential. No paid/live qualification is implied by these changes.

Updated prompt and access documentation for the four-role unified base prompt,
profile `access_class`, class-selected verification guidance, skill-layer splits
and shorter shared worker report protocol. Clarified that Auto intentionally
permits outside-project writes/commands without a card while Guarded requires
accepted consent; documented accepted jail exposure of host `/tmp`, engine state
directories and `/run/user`. The consultant remains normally routed; Codex native
safe reads may reach source despite the reports-only prompt/tool boundary. Also
aligned routing-hint, code-index, review-reuse and host inactivity-nudge guidance
with current behavior.


Persisted Cortex sessions in Paseo now reopen without reapplying cached model,
mode, effort or settings. A removed or denied saved model no longer prevents
read-only history loading; new and explicit configuration choices remain validated.

Coordinator live-stream redaction now preserves path-embedded script names, including
digit-only, hyphenated and underscored names, while continuing to redact credentials
split across stream chunks.

Cortex is an incoming ACP stdio agent with a project-local runtime and
outgoing Codex and Claude ACP engines. The 1.0.0 release resets the package
identity to a content-stamped ACP payload.

This revision adds durable requirements, assignments and attempts; isolated
worker snapshots; exact staged candidates; candidate-bound checks; recoverable
integration; and explicit revision continuation. Instruction and context packets
record their sources and actual delivery, including session replacement. Report
publication remains separate from worker completion and coordinator decision.

Claude workers work again: resolving the installed claude-agent-acp version
(0.81.0, which has no delivery registration of its own) made every Claude worker
refuse its first prompt while the coordinator kept working; an unregistered
installed version now resolves to the pinned one as before. A failed worker is
recorded with its error code and source line instead of a bare class name, and
a coordinator can read its own role through `read_skill`.

A long chat reopens again. Replay sent every update of a worker card, and each
update carried the card's whole log, so one long worker made 17.7 MB of history.
That overflowed Paseo's 16 MiB history buffer, and Paseo showed "Couldn't
refresh agent history" with no way to continue. Replay now sends each card's
final state once (0.36 MB for the same chat). The Paseo relay also exits when
Cortex exits, so a failed load reports an error instead of hanging.

Paseo development checks now use the persistent isolated `~/.paseo-dev` home
and `python3 -B scripts/cortex_paseo_dev.py` CLI wrapper. CLI log retrieval
validates only what the isolated daemon returns; it is not rendered GUI evidence,
and incomplete history must remain unverified. The observed clone cannot resolve
agent `6956c56` (`Unknown sessionId`) because that archived provider session is
absent from the clone's Cortex store. Long-history completeness, binding of the
`Completed with issues` status to the right worker card, and Codex shell-command
expansion remain unverified in a rendered client. Replayed history may still
show raw model identifiers such as `gpt-5.6-luna`.

The model-specific `gpt-5.6-sol` skill-editing rule was removed; model routing,
including prompt and skill work, is decided by the orchestration runtime.
