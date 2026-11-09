# CW-S05: Transactional invalidation journal and bounded snapshot reads

**Status:** built; no-paid checks pass · **Phase:** 3+ · **Size:** L

## Goal

Add the narrow foundation for consistent reconnect in the local slice: one project-level public sequence in an outbox, appended in the same transaction as each in-scope write; a snapshot returned with its high-water mark from one read transaction; a retained floor; and bounded paging. The journal is a new sequence domain and never a merge of existing integers.

## Requirements

CW-R07 (durable, ordered public events that recover gaps) and CW-R04 (versioned commands). See [requirements](../requirements.md).

## Inputs

- [CW-C02a](CW-C02a.md) and [CW-C02b](CW-C02b.md): this item implements the narrow slice subset of those items. They remain planned in full.
- [consistency-and-recovery](../consistency-and-recovery.md) and [protocol](../protocol.md): ordering, deduplication, retention and resynchronization.
- The existing runtime event helpers, which already receive the database handle, and the existing explicit read-transaction pattern used by the task context.

## Deliverables

- Outbox table and one append helper. The public append never calls the internal event helpers recursively.
- A coverage inventory of in-scope writers: lifecycle, node and result writes; delegation event appends; direct task orchestration updates; question create, answer, supersede, cancel and batch writes; adapter commands; and persisted host history changes. A coverage test fails for any uncovered writer.
- Snapshot and high-water mark read inside a single read transaction, with typed reads that resolve inside that snapshot.
- Retention of 10,000 invalidation rows per project, pruned with a transactional floor update.
- Reconnect rules: snapshot, then events after its cursor; duplicates ignored; backwards entity versions rejected; sequence gaps or floor expiry trigger a visible resynchronization.

## Acceptance

Positive:

- A state change and its outbox row commit together, verified by fault injection between the two writes.
- The public sequence strictly increases within a project, and rows read back in order.
- A snapshot and its cursor come from the same transaction. A 500-summary project pages in bounded pages.

Negative:

- An outbox append failure rolls back the mutation that caused it.
- A writer path that changes state without the helper fails the coverage test.
- Outbox payloads never contain raw prompt or transcript text.
- The public sequence is never equal to or derived from `runtime_events.id` or `task_events.id`.
- An `initialize=False` open does not create the outbox table.

## Dependencies

[CW-S04](CW-S04.md).

## Validation

- Targeted `tests/test_workspace_outbox.py`: fault injection, monotonic sequence, coverage inventory, floor expiry and bounded paging.
- Existing store and migration tests, unchanged in expectation.

## Documentation impact

- [consistency-and-recovery](../consistency-and-recovery.md) and [protocol](../protocol.md).
