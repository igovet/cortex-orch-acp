# CW-C02b: Atomic snapshot cursor, bounded replay, gap detection and resync

**Status:** planned · **Phase:** 3+ · **Size:** L

Split from [CW-C02](CW-C02.md) to keep each part within the two-day limit.

## Goal

Serve a snapshot together with an opaque cursor taken inside one read consistency boundary, so that no outbox event committed between the snapshot and the cursor is lost. The cursor is bound to project identity, stream epoch, filter and authorization scope, and schema version. A client replays from a retained cursor, deduplicates by public event identity, and receives `resync_required` on a gap, an unknown epoch, or retention expiry. Delivery is at-least-once.

## Requirements

CW-R07 (recover gaps and disconnects) and CW-R15 (only the public structured adapter). See [requirements](../requirements.md).

## Inputs

- [consistency-and-recovery](../consistency-and-recovery.md): snapshot plus cursor; bounded replay; resnapshot on a gap; reject stale entity versions and old owner epochs.
- [protocol](../protocol.md): the cursor is opaque and scoped; history must not silently drop state.
- [ADR 0005](../adr/0005-public-contract.md).
- [CW-C02a](CW-C02a.md): the outbox, its sequence and its retention floor.

## Deliverables

- Snapshot function that reads the projection and the current outbox position in one read transaction.
- Opaque cursor encoding with the bound fields above. A cursor is validated against the requesting scope on every use.
- Replay API: returns events after a cursor in order, up to a page limit, with the next cursor.
- `resync_required` response carrying the reason (gap, epoch, retention expiry, schema version).

## Acceptance

Positive:

- With a concurrent writer appending events, replay from the cursor returns every event after the snapshot, in order. After public-identity deduplication, each event appears exactly once.
- A retained cursor replays correctly after a client disconnect.
- An expired cursor returns `resync_required`, and a new snapshot and cursor succeed.

Negative:

- A gap in the public sequence produces `resync_required`; it is never silently skipped.
- A cursor from another project, another authorization scope, or an old stream epoch is rejected.
- Events older than the retention floor are not replayed silently.
- An unsupported schema version returns an explicit error, not an empty stream.

## Dependencies

[CW-C02a](CW-C02a.md).

## Validation

- `snapshot-cursor-concurrent-writer`: snapshot taken during concurrent appends; replay compared with the event log.
- `replay-gap-detection`: an injected sequence gap produces `resync_required`.
- `retention-expiry-resync`: a cursor older than the floor returns `resync_required`, then a successful resnapshot.
- `cursor-scope-binding`: cursors from another project, scope and epoch are rejected.

## Documentation impact

- [consistency-and-recovery](../consistency-and-recovery.md) and [protocol](../protocol.md) (cursor, replay and resync semantics).
