# CW-U01: Desktop board, list and task creation

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
Build the desktop project board, a list view over the same data, and the create-task entry point, using the layout accepted at Gate D on 2026-11-05 ([desktop approval](../design/desktop-approval.md)). Each card shows execution stage, execution state and wait reason as three separate fields. Actions send intents to the service; the board never rewrites a runtime fact.

## Requirements
- CW-R02: the board shows phase, execution state and wait reason independently; actions express intent.
- CW-R03: a waiting decision is counted and linked to the decision queue ([CW-U03](CW-U03.md)), not merged into the stage.
- CW-R05: a card at the result stage links to the version-bound result ([CW-U04](CW-U04.md)) and is never shown as accepted by default.
- CW-R14: the responsiveness budgets are measured on the reference machine, not claimed.
- CW-R19: task creation offers the current checkout with its open branch, or a dedicated worktree under the machine's worktree root.
- CW-R37: card actions sit outside the card content.

See [requirements](../requirements.md).

## Inputs
- [Desktop concepts](../design/desktop-concepts.md) and the shared 500-task fixture in [prototype fixtures](../design/prototype-fixtures.md).
- [Screens](../SCREENS.md) (desktop project board/list, create task) and [design direction](../DESIGN.md).
- [Performance targets](../performance.md): cold board under 700 ms; scroll and drag at 16.7 ms per frame with no run over three dropped frames; 500-task baseline.
- Typed task projections from CW-C04 and project registration from CW-H02, consumed through the public contract in [protocol](../protocol.md).
- The accepted prototype code (Gate D, 2026-11-05) is the starting point. Validate the current service contract before implementation.

## Deliverables
- Board with stage columns and a list with grouping and filters. Both views read one view-model so counts agree.
- Card component rendering phase, execution state and wait reason as separate, labelled fields.
- Virtualized rows (@tanstack/virtual) in both views.
- Create-task form: one required goal field; project and machine come from context; optional settings expand on demand; creating a task never starts execution by default. The checkout choice (current checkout with its open branch, or a dedicated worktree) and the target branch (the Git default branch unless changed) are optional settings. The worktree root comes from the machine's Worktrees setting in [CW-U07](CW-U07.md).
- Loading, empty, offline/stale, permission-denied and error states as listed in SCREENS.md.

## Acceptance
Positive:
- With the 500-task fixture, process launch to interactive board takes under 700 ms on the reference Ubuntu machine, measured with the method recorded in performance.md.
- Scrolling and filtering 500 tasks keep frame time within 16.7 ms, with no run of more than three dropped frames.
- A task whose stage is "in progress" and whose wait reason is "permission" shows both values; neither overwrites the other.
- Filtering by wait reason or by stage gives the same counts in the board and in the list.

Negative:
- Dragging a card to another stage sends an intent. The displayed stage changes only when the service returns a newer projection version.
- Creating a task does not start execution unless the owner selects the explicit start intent.
- A missing or unknown field renders as "unknown", never as blank or success styling.
- A projection older than the version already shown is rejected; the card keeps its current version and age label.
- A project whose registration is revoked or unauthorized shows the permission-denied state, not cached cards without a label.
- Card actions never cover the card's title, branch or fact line, and selecting a column or card changes no font weight or size.
- An invalid worktree root blocks the dedicated-worktree choice and shows its reason; the description the owner typed is kept.

## Dependencies
- Gate records: CW-D05 (Gate D) must be recorded before implementation.
- CW-C04 (typed projections), CW-H02 (project registration and authenticated RPC).

## Validation
- Unit tests of the view-model: unknown fields, stale-projection rejection, board/list count parity.
- Rendered run in the Ubuntu Tauri 2 shell (CW-F01) with the 500-task fixture at 1280×800 and 1920×1080. Capture the board, list and create form; record WebKitGTK version, GPU driver and build identifier.
- Frame timing from a recorded trace over a scripted scroll and filter sequence. If frame instrumentation is unavailable, record the check as unverified, not passed.
- Negative cases (stale, unknown, revoked) exercised in the rendered app with fixture projections.

## Documentation impact
- [Screens](../SCREENS.md): board and list states once the selected concept is recorded.
- [Desktop approval](../design/desktop-approval.md): concept build identifiers and fixture revision.
- [Performance](../performance.md): measured results with hardware and method, only after execution.
- [Roadmap](../roadmap.md) when qualification status changes.

## Size
L: two views, the card model, the create form and virtualized rendering against a tight frame budget.
