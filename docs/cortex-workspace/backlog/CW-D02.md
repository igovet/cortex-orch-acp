# CW-D02: Single revised desktop prototype

**Status:** phases P0 to P6 built as a mock-only browser prototype; unqualified; Gate D passed 2026-11-05 · **Phase:** 1 · **Size:** L

**Build status:** on 2026-11-05 the owner agreed the static mockups in `prototypes/cortex-workspace/mockups/` as the design basis and authorised the clickable prototype. Phases P0 to P6 of section 10 of the [rework proposal](../design/desktop-rework-proposal.ru.md) are built as mock-only code in `prototypes/cortex-workspace/desktop/`. The owner accepted the prototype on 2026-11-05 (Gate D); the mock evidence itself is not runtime evidence. The revised direction is one Paseo-style variant based on A.

## Goal
One Paseo-inspired desktop direction based on A, with orchestrator-style project and sprint logic.

## Requirements
CW-R13, CW-R14; see [requirements](../requirements.md).

## Inputs
[Architecture](../architecture.md), [testing](../testing-and-acceptance.md), and the dependencies below. Validate current source and tool support before implementation.

## Deliverables
A fixture-only clickable prototype following P1–P6 of the [rework proposal](../design/desktop-rework-proposal.ru.md): shell, task lifecycle, review/code/Git/worktrees, project and sprint views, archive, machines and settings. Owner decisions supersede conflicting historical proposal passages.

## Acceptance
- Walk through task, sprint, machine, worktree, merge and archive flows in both themes; panels are recoverable and keyboard usable. Stale and offline states cannot be mistaken for completion.
- No production backend, credentials or private fixture data; mock outcomes are not runtime evidence.
- Prototype delta from the owner decisions of 2026-11-05. Status of each point in the mock code: the automatic-sprint and check-recovery rules are implemented and covered by mock domain and UI tests (350 tests passed on 2026-11-05); drag-and-drop and paste attachments are not implemented (the mock keeps only attached file names); the other points are not verified by a rendered check: no visible shadows (none, or barely visible); no dash for absent values, and no zero or success styling for unknown values; "Other…" answers with line breaks and attachments; queue items that show attachments, with Edit returning the item to the composer; the queue's transparent layout; a Worktrees settings page with its default path; an Application updates section (see [CW-U16](CW-U16.md)).

## Dependencies
CW-D01, CW-DM02, CW-D07. The design gate passed on 2026-11-05; implementation waits for these dependencies.

## Validation
Execute rework scenarios at 1280×800 and 1920×1080, both themes and by keyboard; retain rendered review evidence. Record commands, environment, outcomes and unrun cases.

## Documentation impact
Update the relevant planned contract and [roadmap](../roadmap.md) when behavior or qualification changes.

## Status record

- Build phases (2026-11-04 and 2026-11-05; mock-only, in memory; headless Chromium; not qualified):
  - P1 frame: built. App frame, Home, project board and list with sprint scope, command palette, and the theme in Settings only (default System). Layout was measured in the light theme only.
  - P2 task session and lifecycle: built. Brief, live plan, worker rows, decision and result cards, composer with queue, model, effort, access and language controls, Steer and Queue as separate modes, Stop, Resume, Close with both outcomes, and single-worker stop. Voice is simulated; no speech provider is connected.
  - P3 review, Git, worktrees, Code and terminal: built for review, merge, conflict, Code, Git and the worktree table. The terminal is a panel shell only; there is no PTY.
  - P4 Home, Inbox, board and sprints: built. Sprint planning with questions, plan versions and validated dependencies; manual and automatic runs; sprint summary.
  - P5 archive and purge: built. Seven-day archive, countdown groups, restore, guarded delete preview and the unmerged-work confirmation. The build record does not confirm the simulated daily purge with a freed-bytes receipt, so it is not treated as built.
  - P6 machines and settings: built. Machine switcher, three-step remote pairing, offline state, orchestration settings with six sections per machine, engine connections with weekly Codex and Claude usage only, worktree and storage settings, appearance, voice and shortcuts. The proposal's P6 lists seven orchestration sections; this reconciliation is open.
- Owner decisions recorded during the build: (1) a result with an unpassed check can be accepted only through the confirmation dialog, and only when the check cannot complete for a technical reason (decision of 2026-11-05; the mock enforces it); (2) an automatic sprint moves a checked task to Ready to merge by itself (decision of 2026-11-05; implemented in the mock). Still open: (3) the Resume or Continue wording, built as Resume; (4) the owner's 2026-11-04 remark on a screenshot-only bug ("such bugs must not exist"), which the build did not identify and therefore did not fix.
- Not built or not verified: a rendered visual comparison with the mockups; the native build; screen-reader and automated accessibility passes; WebKitGTK and other browsers; performance (each command clones the whole state); the rendered review package in [CW-D08](CW-D08.md).
- Round one (historical): clickable Task Desk at `/a` in `prototypes/cortex-workspace/desktop/src/concepts/a/`, implemented 2026-11-04; its route is no longer served. Its 6 concept tests passed on 2026-11-04.
- Walkthrough record: [concept A walkthrough](../design/desktop-concept-a-walkthrough.md). S1 and S2 passed by keyboard at 1280×800 and passed at 1920×1080 with some script-driven steps. S3 and S4 passed. S5 passed at 1280×800 (150% and the 200% preset). S6 passed at 1280×800 with the simulated phone dialog. S5 and S6 were not run at 1920×1080.
- Open: the dialog pattern gap in `src/concepts/a/ui.tsx` (finding A-1 in the [desktop comparison](../design/desktop-comparison.md)). The native build and an assistive-technology pass were not run. Mock outcomes are not runtime evidence.
