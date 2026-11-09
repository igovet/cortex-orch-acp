# CW-C04: Typed task/execution/wait/decision read projections with explicit unknown states

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal

Publish typed, read-only projections for projects, tasks, executions, waits and decisions. Each board row carries a phase, the execution state and the wait reason as separate fields. Phase is derived by a versioned server-side mapping from the runtime's existing state vocabularies. A contradictory or unrecognized combination produces an explicit `unknown` with a warning, never "done". This item does not produce "Accepted" (that is CW-C07).

## Requirements

CW-R01 (stable user tasks separate from executions and sessions), CW-R02 (board shows phase, execution state and wait reason independently; actions express intent) and CW-R07 (unknown remains unknown). See [requirements](../requirements.md).

## Inputs

- [domain-model](../domain-model.md): a board phase is a projection, not a mutable runtime status.
- [protocol](../protocol.md) and [CW-C01](CW-C01.md): envelopes, bounded summaries and explicit unknowns.
- Existing vocabularies to map: `runtime_tasks.state`, `runtime_nodes.status` and `stage` (`src/cortex_runtime/orchestration/state.py`); `task_orchestration.state` and `wait_reason` (`src/cortex_runtime/delegation/events_store.py`).
- [performance](../performance.md): board summaries must stay compact (500-task baseline); no worker transcript payloads on the board.

## Deliverables

- Versioned mapping table from runtime state combinations to phase, execution state and wait reason, with an entry for every enumerated value and an explicit `unknown` fallback.
- Read projections: `projects.list`, `tasks.list` (bounded summaries with filters), `tasks.get`, `executions.get` and `decisions.list`.
- Decisions projection that excludes capacity waits and runtime policy refusals.

## Acceptance

Positive:

- Every enumerated runtime value and every combination in the mapping table produces the typed phase, execution state and wait reason listed for it.
- A `tasks.list` page for the 500-task baseline stays within the page size limit and includes no transcript payload.

Negative:

- An unrecognized runtime state yields `unknown` with a warning, not an error that hides the row.
- A contradictory combination (for example a terminal state with an active worker) yields `unknown` with a warning, never "done".
- A projection computed from an older mapping version is marked stale.
- A capacity wait does not appear as a decision.

## Dependencies

[CW-C01](CW-C01.md).

## Validation

- `projection-mapping-matrix`: one fixture per enumerated runtime value and per listed combination.
- `contradiction-yields-unknown`: contradictory fixtures produce `unknown` with warnings.
- `list-bounded-no-transcript`: page size and payload fields checked on a 500-task fixture.
- `capacity-wait-not-decision`: the decisions projection excludes it.

## Documentation impact

- [domain-model](../domain-model.md) (the mapping table), [protocol](../protocol.md) (projection shapes and unknown semantics).
