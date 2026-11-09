# Requirements

**Status: planned; no implementation or qualification is implied.**

| ID | Planned requirement |
|---|---|
| CW-R01 | Projects contain stable user tasks, separate from executions and ACP sessions. |
| CW-R02 | Board/list shows phase, execution state and wait reason independently; actions express intent, never rewrite runtime facts. |
| CW-R03 | A decision queue distinguishes questions, permissions and result acceptance; no approve-all. |
| CW-R04 | Decisions use version checks, idempotent requests and server-side authorization across devices. |
| CW-R05 | Results expose evidence, missing checks and exact revision; acceptance is version-bound and separate from Git actions. |
| CW-R06 | The machine service survives window closure, owns child sessions, and reconciles crashes without replaying prompts. |
| CW-R07 | Structured versioned snapshots and event streams recover gaps and disconnects; unknown remains unknown. |
| CW-R08 | Desktop and Android enroll securely, revoke access and reconnect without extra user-installed applications. |
| CW-R09 | The Android release, which follows the Ubuntu desktop first release, includes privacy-preserving push, with an explicit unavailable state. |
| CW-R10 | Provide bounded file tree, editor, search, terminal and LSP, protecting concurrent human and agent changes. |
| CW-R11 | Provide status, diff, stage, commit, branch, pull and push with explicit conflicts; no force push. |
| CW-R12 | First qualify Ubuntu and Android; later platforms require separate qualification. |
| CW-R13 | One revised Paseo-style desktop direction based on A, accepted at Gate D on 2026-11-05 for the clickable prototype; mobile concepts follow Gate D and require Gate M before production implementation. |
| CW-R14 | Responsiveness, accessibility and physical-device testing meet measured budgets, not claims. |
| CW-R15 | Generic ACP uses negotiated features only; Cortex-specific views require a public structured adapter. |
| CW-R16 | Protect credentials, project paths and device identities; reject stale, revoked or unauthorized commands. |
| CW-R17 | Model a machine as a running Workspace process with projects; start with one local machine and connect remote machines through a server connector. Switch one machine at a time; Inbox and Archive aggregate machines. |
| CW-R18 | Plan a sprint through a separate orchestration that asks questions and creates linked tasks; support manual and sequential automatic execution. |
| CW-R19 | At task creation choose the current checkout and open branch or a dedicated Git worktree; define safe concurrent access before implementation. The root folder for dedicated worktrees is set per machine on a Worktrees settings page, which has a default path. |
| CW-R20 | Every task reaches Ready to merge, including in automatic sprints; default the target to the Git default branch, overridable per task or sprint. Require owner confirmation for local squash merge or PR creation. A task in an automatic sprint moves to Ready to merge by itself after its checks pass; the owner confirms there. |
| CW-R21 | Automatically archive after seven days in Done; retain for 30 days by default, configurable per machine. At archive or purge, ask about unmerged work: delete only on confirmation; otherwise do not move the task into archive. |
| CW-R22 | Offer Stop and Continue, plus Close as is and Cancel task in the close dialog; do not silently discard work. |
| CW-R23 | Expose Cortex commands through UI controls rather than typed commands and provide detailed orchestration settings per machine. |
| CW-R24 | Require only a task description at creation; generate an editable title using an LLM. |
| CW-R25 | Apply DDD-lite to Workspace-owned domains and test-first development to domain rules, lifecycle, mapping, Git, storage and adapter contracts; use rendered and accessibility review for visual design. |
| CW-R26 | Keep Cortex's core working controls in the desktop app: main model (provider and model), effort, access mode (Development, Guarded, Auto), language, visible context occupancy, token and plan usage, and voice input. Each control shows the runtime-confirmed value; unknown stays unknown. Voice input is not implemented in Cortex today; Paseo's composer dictation is the reference for CW-R31, and the speech provider is an open decision. |
| CW-R27 | Show every worker of a task, including running, waiting and finished workers, each with status, route, elapsed time and checks. Finished workers stay listed in their own section, and re-opening one restores its full activity history. |
| CW-R28 | Show what each worker does while it runs (its messages, reads, searches, edits, commands, questions, Cortex actions and errors) and keep that history after it finishes. The owner can stop one running command, with a stated reason, without stopping the task. |
| CW-R29 | Let the owner write directly to a running worker. Each message is captured durably and shows a delivery state (pending, delivered, withdrawn or not delivered). A message grants no scope, permission, review or acceptance change. Interrupting the current engine turn is a separate capability, offered only for an engine qualified for it. |
| CW-R30 | Offer task steering as two labelled modes: Steer, captured at once, applied after the current step and not editable once sent; and Queue, held by the client until sent and editable, removable and reorderable until then. The mode is visible before sending, and its default is set per machine. Queued items show their attachments, and Edit returns an item with its text and attachments to the normal composer for sending again. |
| CW-R31 | Provide voice dictation in every composer, as Paseo does: a microphone control starts and stops dictation, shows a live transcript and inserts text into an editable draft. Insert-and-send is an explicit action, and nothing is sent or applied automatically. Transcription runs through a provider chosen per machine; the speech-provider default is an open owner decision. |
| CW-R32 | Label where each worker fact comes from: runtime-recorded commands and edits, native engine actions without a Cortex receipt, and engine-reported values. A message reported as delivered is never shown as read or acted upon, and unknown stays unknown. |
| CW-R33 | The theme (System, Light or Dark; default System) is chosen only in application settings; the desktop window has no theme switch in its sidebar or status area. |
| CW-R34 | Selection, active-tab and filter states never change font weight or size and never shift layout; selected items use a neutral fill, a visible text or icon marker and `aria-selected`. Shadows are either barely visible or absent. |
| CW-R35 | A merge conflict can be handed to Cortex for resolution; the result returns as a normal diff through Ready to merge for owner review, and no merge runs without owner confirmation. |
| CW-R36 | Each connected Codex and Claude engine shows its weekly limit usage: the percentage used, the window and the reset time, with the snapshot time; unknown values stay unknown. OpenCode usage is not shown because it spans multiple providers. |
| CW-R37 | Board card actions appear outside the card content and never cover its title, branch or fact line. |
| CW-R38 | The Ubuntu desktop app ships as a .deb package and updates itself: it detects a newer GitHub release, shows its version and release notes, and installs it only after owner confirmation and after verifying the signature and checksum of the downloaded package. Failure, verification-failure and rollback states are shown explicitly. |
| CW-R39 | The message composer and the free-form answer field accept attachments by drag and drop into the input and by pasting a copied screenshot, as common desktop chat applications do. |
| CW-R40 | Checks are recovered without waiving them: a check the system skipped is detected and run automatically; a failed or not-run check offers a forced re-run; a result with an unpassed check can be accepted only when that check cannot complete for a technical reason, and only with an explicit confirmation. An ordinary failed check cannot be accepted. |
| CW-R41 | Per-machine orchestration settings reach field-level parity with the Cortex configuration that Paseo exposes, in a more convenient form. MCP servers can be edited. The tools a connected MCP server lists show their access configuration in the same place, and no role is granted a tool by default. |
| CW-R42 | Development and release are isolated channels. A local build is the dev channel, with its own package, binaries, user unit, runtime, configuration and state, and it never reads or changes production Cortex stores. Manual GitHub Actions runs on `main` build the release channel; runs on `dev` publish dev prereleases excluded from Latest (owner decision 2026-11-06). Both channels bundle the complete Cortex runtime and start their own user service without manual environment setup. Qualified adapters and native-login connections are set up on first activation; the owner can optionally copy non-secret Cortex settings into dev. Dev uses its own `.cortex-dev` project store beside release `.cortex`, so a release store never blocks opening the folder. Dev refuses only an unowned or changed dev store; it never adopts or migrates the release store. |

## Owner decisions (2026-11-05)

- Gate D passed for the clickable prototype ([desktop approval](design/desktop-approval.md)). Gate M remains open.
- Attachments by drag and drop and by paste in the composer and the free-form answer field: CW-R39.
- Automatic sprint: a checked task moves to Ready to merge by itself, and the owner confirms there: CW-R20.
- Check recovery, forced re-run, and acceptance with an unpassed check only for a technical blocker: CW-R40.
- Orchestration settings rework, requested after Gate D: field-level parity with the Cortex configuration exposed in Paseo, editable MCP servers, and per-tool access shown for the tools a connected server lists. This extends CW-R23 as CW-R41. The pages and shared rules are in [SCREENS](SCREENS.md#orchestration-settings-desktop); acceptance criteria are in [CW-U07](backlog/CW-U07.md) and [CW-C01](backlog/CW-C01.md). No owner approval of these pages is recorded yet.
- Native builds run in a Docker container. The host needs no development packages, and the owner does not install system libraries for the build. This replaces the earlier statement that the owner installs them (CW-S09, CW-S11, CW-F01).
- Dev and release channels are isolated. A local build is the dev channel, and manual GitHub Actions runs publish production from `main` and dev prereleases from `dev`: CW-R42, implemented by CW-S12, with the release workflow in CW-S11 and the update rule in CW-U16.
- Installing the `.deb` must provide Cortex and its services without a source checkout, manual `service.env` or a settings-clone prerequisite. Login startup, bounded native app-driven start, best-effort upgrade restart and removal stop apply to both isolated channels. This supersedes the earlier explicit-checkout-only and never-activate-units rules.
- Open, not decided: per-tool engine access, and access per worker role (for example, only frontend and QA workers). The current configuration schema stores neither.

Trace work and validation through the [backlog](backlog/index.md).
