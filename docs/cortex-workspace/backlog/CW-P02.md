# CW-P02: Android FirebaseMessagingService integration: high-priority data-message handling, on-open E2EE content fetch via N03's session, token registration/rotation flow

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
Receive high-priority data messages on Android. Register and rotate FCM tokens through the authenticated desktop channel. On open, fetch the actual content over the paired iroh session. Notification text is built only from fetched content, never from the push payload.

## Requirements
CW-R09 (push with an explicit unavailable state) and CW-R16 (protect device identities; reject unauthorized or revoked devices). See [requirements](../requirements.md).

## Inputs
- [Remote access](../remote-access.md): on-open fetch over the authenticated channel; duplicates, delivery delays and revoked devices need tests.
- CW-P01 (payload schema) and CW-N02 (Android bridge).
- CW-N03 (paired session) and CW-N04 (allow-list and revocation). Both are required for the on-open fetch and for revoked-device handling.

## Deliverables
- A `FirebaseMessagingService`. `onMessageReceived` decodes `event_type` and `task_ref` only.
- Token flow: on `onNewToken`, the app sends the token to the paired desktop over the authenticated RPC. The desktop stores it per paired device and removes it on revoke.
- On message: open or reuse the iroh session, fetch content through the typed read API, then build the local notification from fetched content only. If the fetch fails, show a generic "Cortex update available" notification with no content.
- Deduplicate by the event-scoped `task_ref` from CW-P01, not by task or FCM delivery ID. Persist seen references across process restarts for the supported delivery/retry window; document its bound and do not claim deduplication beyond it.

## Acceptance
**Positive**
- A data message with an allowed schema yields one local notification after the on-open fetch. Tapping it opens the item.
- Token rotation registers the new token. The desktop invalidates the old one.

**Negative**
- Content fields in a payload are ignored and never displayed. A test sends a payload with a title, and the app shows only fetched content.
- Duplicate deliveries of one `task_ref`, including after app restart within the supported window, produce one notification. Two distinct event references for the same task each produce a notification.
- A message for a device removed from the desktop triggers no fetch and no content notification. Its token is removed.
- With no network, the app shows the generic notification or retries silently. It never shows stale content as current.
- A message addressed to another installation's token is ignored.
- Logs contain no token and no payload.
- Reject revoked or expired credentials: revoked devices get no content (CW-N04).

## Dependencies
CW-P01, CW-N02, CW-N03, CW-N04. Owner gates CW-D05 and CW-M03 are mandatory and are satisfied transitively.

## Validation
- Android instrumented tests on the physical device with Google Play Services present.
- Unit tests for payload parsing and deduplication, using fixtures.
- Live FCM delivery requires explicit owner authorization. Without it, delivery is reported as unverified.
- No check has been run for this planned item.

## Documentation impact
- [Remote access](../remote-access.md): push flow and token rotation.
- [Security design](../security.md): push as a hint, not authority.
- [Platform policy](../platforms.md): Android FCM requirements and limits.
