# CW-H03: Bounded fan-out, history paging and slow-client resync

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal

Make the service's event fan-out and history reads bounded. Each connected client has a bounded outbound queue. A slow client is sent `resync_required` and recovers from a snapshot, instead of letting memory grow or events drop silently. History is served in cursor-based pages, so a 10,000-event history can be read completely without loading it whole. One stalled client never delays another.

## Requirements

CW-R07 (recover gaps and disconnects; unknown remains unknown) and CW-R14 (budgets measured, not claimed; the 10,000-event budget), and CW-R28 (live worker activity is pushed by cursor and searchable without loading the whole history). See [requirements](../requirements.md).

## Inputs

- [performance](../performance.md): 10,000 events at 60 fps with a viewport-bounded DOM; the baseline of 500 tasks and 10,000 events. The service's share of the budget is its paging and queue behavior.
- [consistency-and-recovery](../consistency-and-recovery.md) and [protocol](../protocol.md): bounded history must not silently drop state; resnapshot on a gap.
- [CW-H02](CW-H02.md) (authenticated local RPC) and [CW-C02](CW-C02.md) (snapshot, cursor and resync).

## Deliverables

- Per-client bounded queue with an explicit limit, a depth metric and a resync counter.
- Slow-client policy: when the queue is full, the client receives `resync_required` and reconnects from a snapshot.
- Paged history API (timeline pages) with cursors, a maximum page size and explicit out-of-range responses.
- Activity push: worker activity rows pushed by cursor, in the same order and with the same row identity as the paged read. Polling remains the fallback.
- Bounded search over an attempt's redacted activity, paged, with a maximum page size and an explicit count of matches returned.

## Acceptance

Positive:

- A synthetic 10,000-event history pages completely with cursors and no gaps; each page is within the size limit.
- A client that stops reading for more than the queue limit receives `resync_required` and recovers from a snapshot.
- Service memory per client stays within the recorded bound during a sustained-output run.
- A pushed activity row and the same row read from a page have identical identity and order.

Negative:

- A queue overflow never drops an event silently; it always produces `resync_required`.
- A request for a page beyond the retained window returns an explicit out-of-range response.
- A request for a page larger than the maximum is refused or capped with an explicit notice; it is not honored silently.
- A stalled client does not increase the delivery latency of a second, active client beyond the recorded bound.
- A search whose results exceed the page cap is paged or capped with an explicit notice, never truncated silently.

## Dependencies

[CW-H02](CW-H02.md) and [CW-C02](CW-C02.md).

## Validation

- `history-10k-paging`: complete read with cursors; check for gaps and duplicates.
- `slow-client-resync`: a stalled client receives resync and recovers.
- `overflow-no-silent-drop`: injected overflow produces resync, never a missing event.
- `page-size-cap` and `out-of-range`: explicit refusals.
- `other-client-unaffected`: latency of the active client measured with and without a stalled client.
- `activity-push-order`: pushed rows match paged rows by identity and order.
- `activity-search-capped`: search over an attempt returns explicit paging or a cap notice.

## Documentation impact

- [performance](../performance.md) (measured service-side rows, with method and hardware), [consistency-and-recovery](../consistency-and-recovery.md), [protocol](../protocol.md) (paging and resync).
