# Prototype fixtures

**Status: deterministic mock data and plain TypeScript domain foundation implemented.** The owner approved the static design, and the clickable Workspace runs on this data; runtime integration and native qualification are separate work. Mock outcomes are never runtime evidence.

Location: [desktop prototype](../../../prototypes/cortex-workspace/desktop/README.md). Revision `cw-fixtures-v3`, seed `13014`. `src/fixtures.ts` returns fresh equal route data, retaining the existing 8 projects, 500 task IDs and 10,000 event IDs. Project distribution remains 220/90/65/50/35/25/15/0 for compatibility rather than reducing to the proposal's four-project example. Every event inherits its task's project. Names, relative paths, dates and messages are synthetic; the privacy test scans the complete fixture, including its domain seed, for URLs, emails, credential patterns and absolute home paths.

Machines are explicit process records, never generated from projects. The default `createMockWorkspace()` has exactly one local **This machine**, no projects and 30-day retention. The rich fixture includes that local machine with six projects, an offline remote with Mosaic Studio and the connected remote **GPU box** with Willow Archive, so Inbox and Archive have several machines to cover. Each machine carries its own orchestration configuration, connections, retention and worktree folder. All machines have stable IDs; a project stores its machine ID. Legacy selectors retain unknown worker counts for disconnected or missing machines, null absent activity and undefined unknown projects.

## New domain seed and API

`loadFixtures().workspace` contains machines, projects, sprints, tasks, workers, messages, weekly engine usage, Git and diff records for Cedar Notes, and the 10,000 baseline events. `src/workspace/domain/` contains plain TypeScript rules, command types and projections, with no Solid dependency. `src/workspace/adapter/mock.ts` exposes `createMockWorkspace(seed?)` through `WorkspacePort`: detached `snapshot()` and atomic `dispatch(command)`. Every state and receipt is labelled mock. Every action changes memory only. The [prototype README](../../../prototypes/cortex-workspace/desktop/README.md#workspace-domain-foundation) describes command groups and UI handoff.

Task creation requires a description and a current-checkout or dedicated-worktree choice; mock title generation has pending, success and failure states, and a manual rename wins over late generation. Check outcomes remain passed, failed, not run or unverified. Acceptance binds to a current result digest; a version that is not fully passed needs an explicit override, which records the open checks, and a stale version cannot be overridden. Every integration needs Ready to merge, the selected target and owner confirmation; a PR is not a merge. Target precedence is task → sprint → repository Git default, not the currently open branch. A merge finished by the owner or by Cortex returns a fresh mock version to Ready to merge, unaccepted; merging stays blocked until the owner accepts it. A failed Cortex attempt changes nothing, and Abort merge restores the status, acceptance and result from before.

Sprints include planning dialogue with notes, questions and owner messages, versioned validated dependency plans with their earlier versions, owner edits, cancelling and resuming, idempotent materialization, manual and sequential automatic execution. Dependencies unlock only on accepted current evidence or merged work. Questions, failures, conflicts and stops pause the queue. Workers retain status, activity and terminal results. Direct worker delivery is explicitly simulated; send-now recording, queued input and worker delivery are distinct.

Done tasks become eligible for auto-archive after seven days by default, set per machine. Unmerged work asks for discard confirmation; refusal leaves it outside archive. Retention starts at archive time and defaults to 30 days per machine. Purge blocks on live/unknown processes, unexpected changes, unmerged work, offline machines and active dependents; restore revokes discard permission and restarts the Done clock. A delete previews its counts and skips blocked tasks. No filesystem deletion is made; sizes are fixture values, never measurements, and an unknown size stays unknown.

Engine usage includes Codex and Claude weekly windows only. Unknown usage, reset or observation times remain null; OpenCode is excluded. There is no five-hour window.

## Named domain cases

`WORKSPACE_CASES` in `src/workspace/adapter/fixtures.ts` exports stable task IDs. Times use `WORKSPACE_FIXTURE_NOW` (2030-01-01 02:46:40 UTC), never the wall clock.

| Task suffix | Case |
|---|---|
| 00000–00006 | Offline last-confirmed execution, permission, question, failed check, stale result, merged result, Git conflict |
| 00228–00233 | Working worker with queued/direct input, paused, unconfirmed stop, closed as is, cancelled, Ready to merge |
| 00234–00237 | Archive with 29/12/2 days remaining, and a purge blocked by new files after archiving |
| 00009, 00010, 00316, 00317, 00487 | More archived tasks: on the offline remote (one overdue, cleanup waits for the machine), cancelled, closed as is, and on GPU box |
| 00238–00240 | Title generation failure, pending title, unmerged-work archive question |
| 00241–00243 | Linked sprint tasks with dependencies |
| 00244 | The whole approved task session: plan, six workers across every lane, activity, queued and direct messages in each delivery state, an open question. Also the worktree the Code and Git views open on |
| 00245 | Review: result v2 with all four check outcomes, five finished workers, six changed files and a draft comment |
| 00246–00247 | Ready to merge in the Usage billing sprint, three commits behind, and the task waiting for it. Updating it stops at a scripted conflict |
| 00250–00252 | The rest of Usage billing: a working task, the next one and a blocked one, so its summary has every row kind |
| 00253 | A writer in Cedar Notes' main checkout, so a second one is refused |
| 00488–00489 | On GPU box: a task waiting for an answer, which the Inbox shows from another machine, and a working one |
| 00248 | Merge conflict: one block left, one already chosen by the owner, four cleanly merged files |
| 00249 | Merged, with a clean worktree to delete |

Tasks 00001 and 00002 also carry their permission and question as decision cards. The Audit log export sprint is in planning: a conversation, Plan v2 with its earlier version, and one open question. Both checkout kinds, finished-worker results, all four check outcomes and unknown usage are represented. Cedar Notes alone has code, a commit history, branches and worktrees, including one orphaned worktree; other projects report that the fixture keeps none. Task 00006 keeps its small Amber Components conflict.

## Compatibility and limits

The old `src/store.ts`, project selectors and comparison scripts still use the historical four-phase arrays. The earlier surfaces that used them are no longer routed. They are not synchronized with the new domain port; screens use one domain snapshot, never both stores. Only mock title generation is shared with the old desk model. Runtime status translation is status-only and conservative: unsupported or disconnected facts remain unknown, and runtime completion maps to Review, not integration.

DDD-lite stops at Workspace-owned rules; the Python runtime is not refactored. No storage ADR, real direct-message delivery, runtime command support, pairing, planning-model or title-model integration is asserted. All checks are local TypeScript/Vitest/Vite checks. Screenshots are prohibited by the owner; visual, screen-reader, native-shell and live-engine qualification are not claimed.
