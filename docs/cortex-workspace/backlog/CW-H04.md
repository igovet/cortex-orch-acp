# CW-H04: Crash/EOF/owned-child reconciliation with no prompt replay

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal

After a service crash, a child EOF, or a child exit with unknown cause, reconcile the actual state of each owned child and of its external effects before any restart. Never replay a prompt, a terminal command or an ambiguous Git push. An outcome that cannot be confirmed is recorded as `outcome_unknown` and stays that way until someone inspects it. A new child is not started while the previous one is unconfirmed as gone.

## Requirements

CW-R06 (reconciles crashes without replaying prompts) and CW-R07 (unknown remains unknown; reconnect shows the age of the last confirmed state). See [requirements](../requirements.md).

## Inputs

- [consistency-and-recovery](../consistency-and-recovery.md): after a crash, reconcile actual child and external state; do not replay prompts, terminal commands or ambiguous pushes.
- [storage](../../project/storage.md): after a crash, reconcile the existing attempt and owned processes before starting another attempt; old sessions cannot mutate current state.
- [failures-and-cancellation](../../features/failures-and-cancellation/index.md): the existing cleanup and EOF boundaries.
- [CW-H01](CW-H01.md) (child registry and supervisor) and [CW-C08](CW-C08.md) (stop semantics and typed receipts).

## Deliverables

- Reconciliation procedure per child state: alive and owned, exited with a known status, EOF without exit, and unknown.
- `outcome_unknown` state for external effects (for example a Git push whose result was not recorded) with an inspection step that reads the actual remote and local state.
- Fault-injection harness that kills the service at named crash points.

## Acceptance

Positive:

- After the service is killed while a child is running, restart reconciles the child through the runtime's recovery checks, and the prompt count on the transport is unchanged.
- A child EOF without an exit status is recorded as unknown until it is inspected.
- An interrupted Git push is recorded as `outcome_unknown`, and inspection of the remote determines the final state.

Negative:

- No prompt, terminal command or push is retried automatically.
- No replacement child starts before the previous child is confirmed gone.
- A failed reconciliation leaves the state unknown, never completed.
- Restarting the service does not change the stored evidence of an earlier run.

## Dependencies

[CW-H01](CW-H01.md) and [CW-C08](CW-C08.md).

## Validation

- `crash-point-matrix`: kill the service at each named point (before spawn, after spawn, mid-prompt, after child exit, during push) and record the reconciled state.
- `no-prompt-replay-count`: the transport's prompt count before and after a restart.
- `eof-unknown`: EOF without exit stays unknown until inspected.
- `push-outcome-unknown`: an interrupted push fixture is recorded as unknown, then resolved by inspection.
- `no-second-child-before-confirm`: a replacement start is refused while the old child is unconfirmed.

## Documentation impact

- [consistency-and-recovery](../consistency-and-recovery.md), [failures-and-cancellation](../../features/failures-and-cancellation/index.md) (future crash behavior, marked as planned), [testing-and-acceptance](../testing-and-acceptance.md) (crash matrix), [roadmap](../roadmap.md).
