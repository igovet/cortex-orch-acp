# CW-A02: Android decision detail, discussion and desktop-synchronized outcomes

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
Build the Android decision detail for questions and permissions, with the effect and rejection consequence of each decision. Answer, approve or reject with request IDs. Provide a bounded discussion view with structured steering. Keep decision state synchronized with the desktop through the event cursor. When a push opens a decision, show its current state without re-triggering an action that is already resolved. Push delivery itself (FCM registration, data-message handling and the push gateway) belongs to CW-P01 to CW-P03; this item consumes it.

## Requirements
- CW-R03: questions and permissions are separate; no approve-all.
- CW-R04: version checks, idempotent requests and server-side authorization across devices.
- CW-R07: cursor resume after background or foreground; resnapshot on gap.

See [requirements](../requirements.md).

## Inputs
- [Screens](../SCREENS.md): decision detail, discussion and reconnection states.
- [Protocol](../protocol.md) and [consistency](../consistency-and-recovery.md): request IDs, expected version, one winner, no duplicate effects.
- [ADR 0009](../adr/0009-push.md): notifications are minimal and opaque; content is fetched over the secure channel.
- Decision handler (CW-C03) and structured steer, stop and recheck (CW-C08).

## Deliverables
- Decision detail with the content rules of [CW-U03](CW-U03.md).
- Submission with request IDs and the same typed outcomes as CW-U03.
- Bounded, paged discussion view and a structured steer command through CW-C08.
- Cursor resume after background and foreground; resnapshot on gap.
- Push-open handler that fetches the authorized decision and shows its current state. It renders no title or body from the push payload.

## Acceptance
Positive:
- A decision answered on the desktop while the Android app is backgrounded shows as resolved after resume, through cursor replay. No duplicate action is offered.
- The push latency for a decision answered on the desktop is measured on the physical device once CW-P02 and CW-P03 exist. The observed bound is documented, not promised.

Negative:
- A push for a decision whose version is already resolved opens the resolved state and offers no action.
- A decision submitted from Android against an older version is rejected as stale, with the reason.
- A push carrying plaintext title or body is not displayed, and the app reports a delivery-policy violation.
- Sending a discussion steer fails explicitly when the session has stopped or the device has been revoked.

## Dependencies
- [CW-A01](CW-A01.md) (app shell), CW-C03 (decision handler), CW-C08 (structured steer, stop and recheck), CW-P02 (Android push integration).
- Gate records: CW-D05 (Gate D) and CW-M03 (Gate M).

## Validation
- Physical Android device with the app backgrounded while the desktop answers. Record push receive timestamps and the resume result.
- Through the CW-P02 fixture path, replay one logical event and confirm no duplicate notification or second action is offered; then deliver two distinct events for the same task and confirm neither is suppressed as a duplicate. Fetch current authorized state before offering any action.
- Rendered captures of decision detail and discussion at 360×800 and 412×915.
- Record exact commands, device and outcomes; unrun delivery measurements are reported as not run.

## Documentation impact
- [Screens](../SCREENS.md): Android decision and discussion states.
- [Platforms](../platforms.md): push qualification on the physical device.
- [Roadmap](../roadmap.md) when qualification changes.

## Size
L: decision detail, submission, discussion, cursor resume and the push-open path, to be validated on a physical device. The push transport is out of scope here because CW-P02 owns it.
