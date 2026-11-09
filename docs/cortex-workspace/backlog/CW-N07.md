# CW-N07: Network-change resilience: Android ConnectivityManager handler forces clean reconnect + cursor-based event resume on Wi-Fi↔cellular handover; handover test matrix

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
On Android, detect network loss and transport changes (Wi-Fi to cellular and back). Force a clean re-dial of the paired desktop, then resume the event stream from the last confirmed cursor. Any gap triggers a resync and is never assumed away.

## Requirements
CW-R07 (snapshots and cursors recover gaps and disconnects; unknown remains unknown) and CW-R08 (reconnect without extra installed software). See [requirements](../requirements.md).

## Inputs
- [Architecture](../architecture.md) and [protocol](../protocol.md): `changes.since` cursor and `resync_required` on gap or overflow.
- [Consistency and recovery](../consistency-and-recovery.md): show the age of the last confirmed state while disconnected.
- CW-H03 (bounded fan-out, history paging and slow-client resync) and CW-N03 (paired session).
- Dependency constraint: CW-A01 depends on this item. The network handler therefore lives in the native bridge from CW-N02, not in the app shell. Making this item depend on the app shell would create a cycle.
- Connection migration across interfaces is not assumed to work for iroh. The test must cover the forced re-dial path whether or not QUIC migration succeeds.

## Deliverables
- A `ConnectivityManager` network callback in the native bridge. On loss or a transport change, it closes the iroh session and re-dials the paired NodeId.
- Per-project cursor storage for the last confirmed event. On reconnect, the client calls `changes.since(cursor)`. A `resync_required` answer triggers a full snapshot.
- A bridge event carrying the age of the last confirmed state, so the app can show it while offline.
- A handover test matrix: Wi-Fi to cellular, cellular to Wi-Fi, Wi-Fi off, airplane mode, a Wi-Fi network with no internet, a desktop IP change, and direct versus relay paths.

## Acceptance
**Positive**
- A Wi-Fi-to-cellular handover re-establishes the session within the recorded budget. No duplicate events are applied; deduplication is by cursor.
- After resume, the event sequence matches the desktop sequence with no gaps. Event IDs are checked.

**Negative**
- A stale cursor beyond desktop retention produces `resync_required`. The app shows the resynced state, not an assumed one.
- A network change during a pending decision submission does not send the decision twice. The idempotency key yields one winner (CW-R04).
- The test fails if the item relies on QUIC migration alone without the forced re-dial path.
- A revoked device stays rejected after a network change (CW-N04).
- The age-of-state indicator does not advance to "fresh" while offline.

## Dependencies
CW-N03, CW-H03. Owner gates CW-D05 and CW-M03 are mandatory and are satisfied transitively.

## Validation
- Physical Android handover matrix on the owner's device, with the Ubuntu desktop. Network toggles through `adb shell svc wifi disable` and `svc data`, plus real Wi-Fi-to-cellular handover. Record device model, Android version, carrier and desktop hardware for each row.
- Emulator runs are not qualifying evidence.
- No check has been run for this planned item.

## Documentation impact
- [Remote access](../remote-access.md): reconnect and handover behavior.
- [Protocol](../protocol.md): resume and resync semantics as exercised on Android.
