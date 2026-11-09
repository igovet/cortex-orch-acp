# CW-C02a: Public project outbox appended in the same transaction as runtime state changes

**Status:** planned · **Phase:** 3+ · **Size:** M

Split from [CW-C02](CW-C02.md) to keep each part within the two-day limit.

## Goal

Add one project-level public event sequence and an append-only outbox. Runtime mutators write an outbox row in the same SQLite transaction as the state change the row describes. The outbox is separate from the existing sequence domains, which must stay separate: `runtime_events.id` (`src/cortex_runtime/orchestration/state.py`), `task_events.id` (`src/cortex_runtime/delegation/events_store.py`) and transcript sequence numbers. The public sequence is a new domain and is never a merge of those integers.

## Requirements

CW-R07 (durable, ordered public events that recover gaps). See [requirements](../requirements.md).

## Inputs

- [consistency-and-recovery](../consistency-and-recovery.md): commit runtime-owned state and its public event atomically where supported; reordered or duplicate events must not duplicate effects.
- [protocol](../protocol.md): outbox ordering, deduplication and retention.
- [ADR 0005](../adr/0005-public-contract.md).
- The transactional pattern in `src/cortex_runtime/orchestration/policy.py` (`steer` runs its event append inside `BEGIN IMMEDIATE`, using the `event(db, ...)` helper). New appends follow the same pattern; the helper's own definition location is to be confirmed before reuse.
- [storage](../../project/storage.md): additive schema under the existing writable-open migration rules; `initialize=False` opens must not create anything.
- Baseline test pattern: `tests/test_orchestration_events_store.py` (additive columns on an existing database).

## Deliverables

- Outbox table: public sequence, project identity, event type, entity reference, entity version, schema version, payload digest, and creation time. Payloads carry identifiers and state only, never raw prompt or transcript text.
- One append helper that every in-scope mutator calls inside its transaction.
- Retention rule with an explicit floor: the oldest retained sequence is recorded so readers can detect expiry.
- Migration note: additive, applied through `Store(initialize=True)`. Record whether a format bump is required.

## Acceptance

Positive:

- A runtime state change and its outbox row commit together, verified by fault injection between the two writes (neither or both persist).
- The public sequence strictly increases within a project, and rows read back in order.
- Opening the store with `initialize=False` does not create the outbox table.

Negative:

- An outbox append failure rolls back the mutation that caused it.
- A mutator path that writes state without the helper fails the coverage check.
- A payload containing raw prompt or transcript text is refused.
- The public sequence is never equal to, or derived from, `runtime_events.id` or `task_events.id`.

## Dependencies

[CW-C01](CW-C01.md).

## Validation

- `outbox-atomicity-fault-injection`: injected failure between state write and outbox write.
- `outbox-monotonic-sequence`: concurrent appends in one project.
- `outbox-migration-additive`: upgrade of an existing database keeps existing rows.
- `inspection-open-non-mutating`: `initialize=False` does not create the table.
- `mutator-coverage`: a list of in-scope mutators and a check that each calls the helper.

## Documentation impact

- [consistency-and-recovery](../consistency-and-recovery.md) and [protocol](../protocol.md) (outbox semantics); [storage](../../project/storage.md) only after the change ships.
