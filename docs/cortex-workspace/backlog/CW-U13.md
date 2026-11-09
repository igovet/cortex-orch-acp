# CW-U13: Task Workers tab: worker list, live and finished worker views

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies · **Phase:** 3+ · **Size:** L

## Goal

Show the owner everything about a task's workers: the list of every worker (running, waiting and finished), what each running worker is doing as it happens, and the full record of each finished worker. No worker disappears when it ends.

## Requirements

- CW-R27: every worker of the task is listed; finished workers stay visible and restorable.
- CW-R28: the live activity of each worker and its history after it finishes; a single running command can be stopped with a reason.
- CW-R32: each fact shows its provenance (recorded command, native action, recorded edit, engine-reported value); unknown stays unknown.
- CW-R02: status and wait reason are shown separately.
- CW-R05: results show their evidence state and missing checks.
- CW-R07: snapshot plus cursor; a gap triggers a visible resynchronization.
- CW-R14: the history budget is measured.

See [requirements](../requirements.md).

## Inputs

- [CW-U02](CW-U02.md): the task detail host, the Dialog's compact worker rows and the plan strip.
- [CW-C04](CW-C04.md) (typed read projections for status, wait reason and checks), [CW-C05](CW-C05.md) (result and evidence completeness) and [CW-H03](CW-H03.md) (history paging and activity push).
- [Observability](../../features/observability/index.md): the worker activity rows and their cursor order, the check outcomes (passed, failed, not run, unverified, closed by rerun, retracted, superseded) and the disclosure that prose arrives in complete blocks. Confirm the exact row contract in source before implementation.
- Node statuses `pending`, `running`, `completed`, `failed` and `cancelled`, with stage `execution` or `review`: `src/cortex_runtime/orchestration/state.py`.
- The command-stop path in `src/cortex_runtime/orchestration/command_control.py` is the semantics to expose as a UI control. The text command is not part of the UI.

## Deliverables

- Workers list: groups Coordinator, Execution, Review and Verification (in plan order), and Finished (collapsed while the task runs). Two-line rows show status, role, objective, elapsed time (frozen at a terminal state), the confirmed route, the current activity, context occupancy and tokens, each shown only when known (an absent value is not shown; it is never a dash or zero), and up to two badges. Sort by plan order, needs-you first or recently active. Filter by text. Empty and history-unavailable states.
- Live worker view: header; stream items for messages, thoughts (only when recorded), reads and searches, edits with expandable diffs, commands with a live tail and their state, native engine actions (labelled "native · no Cortex receipt", with no Stop), Cortex actions, owner messages with their delivery state, and errors. Follow behaviour, filters, search over loaded pages and bounded search of older history.
- Stop a command: a picker lists running commands and requires a reason of at most 2000 characters. States are sending, requested, stopped, missed and refused.
- Question card: "Other…" opens a free-text answer field with line breaks (Shift+Enter) and attachments, as the main composer does, including drag and drop and paste ([CW-R39](../requirements.md)). Enter sends.
- Finished worker view: status chip, Result (clipped at 16,000 characters with a visible note), Checks, Report, Changes by this worker (labelled "From recorded edits") and the full read-only Activity.
- Reviewer rows show the verdict and the findings. The verification lane lists each runtime command with its argv, elapsed time, exit code and output tail.
- The state matrix in [SCREENS](../SCREENS.md), including loading, partial history, history failed, offline, interrupted and narrow layouts.
- Freshness is labelled "Updated Ns ago". The stream is near-live, not token streaming.

## Acceptance

Positive:

- Running, waiting and finished workers are all listed. Re-opening a finished worker restores its full activity.
- Follow is on when a running worker is opened. Scrolling up turns it off and shows "↓ N new · Follow". New items never move the viewport while Follow is off.
- Stop a command requires a reason. A command that already finished shows "missed".
- Not-run checks are shown in neutral grey, never green.

Negative:

- A native engine action is never presented as a gateway-verified command.
- Changes by a worker are labelled "From recorded edits" and are not presented as the authoritative branch diff.
- A history failure shows "Activity could not be loaded · Retry", never "no workers".
- A partial history is labelled as partial. A cursor gap triggers a visible resynchronization.
- A revoked or unauthorized device shows permission denied and no cached transcript.

## Dependencies

- [CW-U02](CW-U02.md), [CW-C04](CW-C04.md), [CW-C05](CW-C05.md) and [CW-H03](CW-H03.md).
- [CW-U14](CW-U14.md) adds the worker message composer to this view. It is a dependent item, not a prerequisite.
- Gate records: CW-D05 (Gate D) must be recorded before implementation.

## Validation

- Rendered run with fixtures covering running, waiting, finished-with-issues, failed, cancelled and interrupted workers, at 1280×800 and 1920×1080. Capture screenshots.
- A 10,000-activity-row fixture: DOM node bounds and Follow behaviour, measured as in [CW-U02](CW-U02.md).
- View-model tests for provenance labels and check-outcome mapping.
- A forced cursor gap through the [CW-H03](CW-H03.md) paging path.
- Record exact commands, environment and outcomes. Live activity from a real engine run is not run in this item's checks until the owner requests it.

## Documentation impact

- [SCREENS](../SCREENS.md): the worker views (already updated with this item).
- [Observability](../../features/observability/index.md) and [protocol](../protocol.md), only after the projection ships.

## Size

L: a list, a live view, a finished view and the stop-command control. Split into list and detail before implementation if effort exceeds L.
