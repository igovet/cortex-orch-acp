# CW-C02: Public project outbox + atomic snapshot cursor; no merged internal sequence IDs

**Status:** planned · **Phase:** 3+ · **Size:** S

## Goal

Roll-up item for the public event stream. The work is split into [CW-C02a](CW-C02a.md) (the transactional outbox written by runtime mutators) and [CW-C02b](CW-C02b.md) (the atomic snapshot cursor, bounded replay, gap detection and resync). This item has no build work of its own. It is complete when both parts pass their acceptance. The split keeps each part within the two-day limit for size L.

## Requirements

CW-R07 (structured versioned snapshots and event streams recover gaps and disconnects) and CW-R15 (Cortex-specific views require a public structured adapter). See [requirements](../requirements.md).

## Inputs

- [consistency-and-recovery](../consistency-and-recovery.md) and [protocol](../protocol.md).
- [ADR 0005](../adr/0005-public-contract.md).

## Deliverables

- Accepted results of CW-C02a and CW-C02b, recorded in those files.

## Acceptance

- Both parts pass their own positive and negative acceptance.

Negative:

- No internal sequence (`runtime_events.id`, `task_events.id` or transcript sequence) is exposed as, or merged into, the public cursor. A test or review finding of such a merge fails this item.

## Dependencies

[CW-C02a](CW-C02a.md) and [CW-C02b](CW-C02b.md). Their prerequisites are listed in those files.

## Validation

- Review of the two part records. No runtime test of its own.

## Documentation impact

- [consistency-and-recovery](../consistency-and-recovery.md) and [protocol](../protocol.md), once the two parts are accepted.
