# CW-U02: Desktop task detail: discussion, plan, workers and bounded history

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
Build the desktop task detail surface: a header (goal, stage, execution state, wait reason, age of the last confirmed state), a plan strip with stage pills, the discussion with compact worker rows, the tab host (Dialog, Workers, Changes, Code and Terminal) and bounded history. All data comes from typed public projections and paged history. The history list must hold the 10,000-event budget with a viewport-bounded DOM and incremental row updates.

## Requirements
- CW-R02: the header shows stage, execution state and wait reason separately.
- CW-R05: plan and result links expose their evidence state.
- CW-R07: snapshot plus cursor; a gap triggers resynchronization, never silent omission.
- CW-R14: the 10,000-event budget is measured.
- CW-R27: every worker of the task is listed, and finished workers stay visible. Their views are specified in [CW-U13](CW-U13.md).
- CW-R26: the header and discussion carry the core session controls and indicators defined in [CW-U10](CW-U10.md); this item does not define them separately.

See [requirements](../requirements.md).

## Inputs
- [Screens](../SCREENS.md) (task discussion and plan, worker detail, history states).
- [Design direction](../DESIGN.md): no false progress percentages; observed, reported, estimated and unknown stay distinct.
- [Performance](../performance.md): 10,000 events at 60 fps with a viewport-bounded DOM; no full-screen rerender on streaming updates.
- [Protocol](../protocol.md) and [consistency](../consistency-and-recovery.md): snapshot and cursor, bounded replay, resnapshot on gap.
- Stable work-item and execution links from CW-C06; history paging from CW-H03; the 10,000-event fixture in [prototype fixtures](../design/prototype-fixtures.md).

## Deliverables
- Header and sections. Plan steps show the status reported by the projection (completed, active, blocked, unknown). No percentage progress and no time estimate.
- Discussion and history as virtualized rows. History loads through cursor pages from CW-H03.
- The discussion composer and the header host the controls and indicators of [CW-U10](CW-U10.md) (model, effort, access mode, language, context and usage). Voice input is added later by [CW-U11](CW-U11.md).
- Plan strip: stage pills (Planning, Execution, Review, Verification, Ready to merge) with the current stage marked by a text or icon marker and never by bold text, so no layout shifts (CW-R34), the plan revision with a "What changed" link, and context and task tokens. Selecting a stage filters the worker list.
- Tabs: Dialog, Workers, Changes, Code and Terminal. The Dialog shows a compact row for each worker in the turn. A row opens the worker in a side pane at 1600 px and wider, and otherwise opens the Workers tab. The Workers tab content is specified in [CW-U13](CW-U13.md), and the worker message composer in [CW-U14](CW-U14.md).
- The plan graph is read-only and opens from the Workers tab or the plan strip.
- Streaming updates: each token event updates only the affected row's Solid signal.
- All states listed in SCREENS.md.

## Acceptance
Positive:
- A 10,000-event fixture scrolls at 60 fps. The DOM node count stays bounded by viewport rows plus fixed overscan at the top, middle and bottom of the history; the counts are recorded.
- Each streamed token event re-executes only the affected row component, counted by instrumentation.
- Plan steps render exactly the statuses in the projection. The completed count is shown as a count, not a percentage.

Negative:
- A gap in the event cursor triggers a resnapshot and a visible "resynchronizing" state. Rows are never silently missing.
- A plan version superseded by a newer version is shown as superseded; it is not merged into the new version.
- Worker detail for an unauthorized or revoked device shows the permission-denied state and no cached transcript.
- A history page that fails to load shows an explicit error with a cursor-bounded retry. A partial page is never presented as complete.
- A worker that is still running or waiting is never hidden by the Dialog's worker rows or by the plan stage filter.

## Dependencies
- [CW-U01](CW-U01.md) (board and project context), CW-H03 (history paging, slow-client resync), CW-C06 (stable work-item and execution links).
- Gate records: CW-D05 (Gate D).

## Validation
- Rendered run in the Ubuntu Tauri shell with the 10,000-event fixture at 1280×800 and 1920×1080. Sample DOM node counts at top, middle and bottom; capture frame timing from a trace.
- Instrumented build counting row-component executions per streamed token.
- Forced cursor-gap test through the fixture transport and the CW-H03 paging path. Record the observed resynchronization.
- Capture the plan view with blocked and unknown steps.

## Documentation impact
- [Screens](../SCREENS.md): detail-view states.
- [Performance](../performance.md): measured node counts and frame results, only after execution.
- [Protocol](../protocol.md) if the history page shape changes.

## Size
L: four surfaces sharing one virtualized history model and the 10,000-event budget.
