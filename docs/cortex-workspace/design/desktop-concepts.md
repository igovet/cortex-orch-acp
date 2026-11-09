# Desktop concepts

**Status: the round-one concepts A, B and C were implemented as mock-only prototypes and reviewed by the owner on 2026-11-04. None was approved. The work was narrowed to one candidate direction, [Project Desk](desktop-direction.md), which is now superseded by the [rework proposal](desktop-rework-proposal.ru.md) and the static mockups agreed on 2026-11-05. The clickable browser prototype built from them is mock-only. Gate D is OPEN, pending owner acceptance of that prototype. No native build, qualification or Gate D approval is implied.**

Build three distinct clickable Tauri 2 / SolidJS / TypeScript mock-only concepts under `prototypes/cortex-workspace/desktop/`, with a common fixture loader and separate A, B and C routes. The prototype is a development-only mock and not a shipped application. Its status and checks are recorded in the [desktop comparison](desktop-comparison.md). Build a local development shell without ACP, service or network integration; fixture actions change only in-memory mock state. Use [shared fixtures](prototype-fixtures.md).

- **A Task Desk:** start on board/list, open a task to discussion and plan, then decision and result/diff; code/Git is reached from the task.
- **B IDE with Mission Control:** start on a file/editor surface, open the orchestration overview, then the same task, decision and result/diff; board/list remains accessible.
- **C Focus Workspace:** start on attention and decision queue, progressively reveal task discussion, plan, result/diff, board/list and code/Git.

Each route includes empty, blocked permission, failed check, stale result and disconnected states. Do not imply that a mock action starts an agent or accepts a real artifact. Exercise the same create → decision → result and code/Git conflict scenarios in each concept at 1280×800 and 1920×1080, plus keyboard-only navigation and increased text size. Record the build instructions, screenshots and scenario outcomes for each route, including failures and unrun checks. Compare side-by-side with the owner and record selection or revisions in [Gate D](desktop-approval.md); an absent or conditional selection does not pass the gate.

## Owner review of round one (2026-11-04)

The owner reviewed the first three concepts and did not approve any of them. The owner's feedback, in summary:

- The round-one designs did not meet the owner's expectations. The owner asked for a desktop-native look comparable to Paseo rather than a web page.
- Projects are first-class: tasks belong to projects. The owner needs both an all-projects dashboard and a per-project dashboard.
- Concept A was the closest of the three, but no concept was selected.

Following that review, the owner asked for one new candidate direction in a Paseo-like desktop style, based on concept A, with dark and light themes. The candidate was specified in [desktop direction](desktop-direction.md), which is now superseded (see below). It was not a decision: Gate D remains open in [desktop approval](desktop-approval.md).

### Current design basis (2026-11-05)

The owner agreed the static mockups in `prototypes/cortex-workspace/mockups/` as the design basis and authorised a clickable prototype. The current rules are in the [rework proposal](desktop-rework-proposal.ru.md) (owner decisions of 2026-11-04 and 2026-11-05) and in the mockups. They correct earlier wording in this file:

- The theme preference (System, Light or Dark; default System) is set only in Settings. There is no theme toggle in the sidebar, the status bar or the command palette.
- Selection never changes label weight, font size or layout. Selected rows, tabs and filters use a neutral fill and a text or icon marker, with `aria-selected` or `aria-current`.
- Engine usage shows weekly limits only, for Codex and Claude. OpenCode is not shown.

Gate D stays open until the owner accepts the clickable prototype. Mock outcomes are not runtime evidence.

### Round-one exploration (superseded, kept for reference)

The round-one specifications below are superseded first-round exploration. They are kept, not deleted, so that the owner's comparison and the recorded evidence remain traceable. They do not describe the current direction.

## Concept specifications

Each round-one concept has a candidate specification covering navigation model, layout at 1280×800 and 1920×1080, candidate tokens with measured contrast ratios, keyboard map, component states, screen coverage and mock-state labelling. All three are **candidate directions pending owner Gate D**, not the approved design system:

- [Concept A: Task Desk](desktop-concept-a.md) (board first)
- [Concept B: IDE with Mission Control](desktop-concept-b.md) (code first)
- [Concept C: Focus Workspace](desktop-concept-c.md) (attention queue first)

The statement in [design direction](../DESIGN.md) that tokens, typography, colours and spacing are TBD until Gate D is unchanged.
