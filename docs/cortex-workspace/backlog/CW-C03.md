# CW-C03: Decision CAS/idempotency/revocation tests and handler

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal

Add one decision handler that answers a question or a permission request atomically. In one transaction it checks the decision version (compare-and-set), the request identity (idempotency), the device's authorization and revocation state, the owner epoch and supersession. It then writes the decision state and its public event together, or neither. Result acceptance is a different operation (CW-C07), not a decision.

## Requirements

CW-R03 (questions, permissions and result acceptance are distinct; no approve-all), CW-R04 (version checks, idempotent requests and server-side authorization) and CW-R16 (revoked devices and unauthorized commands are rejected). See [requirements](../requirements.md).

## Inputs

- [protocol](../protocol.md): decision commands, conflict semantics and receipts.
- [consistency-and-recovery](../consistency-and-recovery.md): one winner for concurrent distinct decisions; stale and revoked operations fail explicitly.
- Existing idempotency store: `runtime_requests` (`src/cortex_runtime/orchestration/state.py`), keyed by task, actor and request key, with a digest and a stored result. The handler reuses this pattern and adds device identity to the key. Confirm the existing key shape in source before reuse.
- [CW-C01](CW-C01.md) and [CW-C02](CW-C02.md): envelopes, versions and the outbox that the public event uses.

## Deliverables

- Decision handler with compare-and-set on a per-decision version (not a global counter).
- Idempotency record keyed by device identity and request identity, storing the payload digest and the receipt.
- Conflict responses with typed reasons: already resolved, stale version, superseded, revoked device, stale owner epoch, changed requirement revision, unsupported operation.
- Rule that scheduler-capacity waits and runtime policy refusals are not decisions and cannot be answered through this handler.

## Acceptance

Positive:

- Two devices racing different request identities on the same decision version produce exactly one persisted answer; the other receives "already resolved" with the current state.
- An identical retry (same request identity and payload) returns the original receipt and causes no second state change.
- The decision state change and its public event commit together, or neither persists.

Negative:

- The same request identity with a changed payload returns a conflict and changes nothing.
- A superseded question, a revoked device, an old owner epoch, or a changed requirement revision is rejected with its explicit reason.
- A capacity wait submitted as a decision is rejected.
- A decision from a device without the required grant is rejected before any state change.

## Dependencies

[CW-C01](CW-C01.md) and [CW-C02](CW-C02.md).

## Validation

- `decision-race-two-devices`: two request identities, one decision version, exactly one winner.
- `decision-idempotent-retry`: receipt equality and no second state change.
- `decision-payload-mismatch`: changed payload under the same identity changes nothing.
- `decision-revoked-device` and `decision-stale-epoch`: explicit rejection.
- `decision-atomic-commit`: injected failure between decision and event writes.
- `capacity-wait-not-decision`: the capacity wait is rejected.

## Documentation impact

- [protocol](../protocol.md) (decision commands and conflict table); [consistency-and-recovery](../consistency-and-recovery.md) (idempotency and race outcomes).
