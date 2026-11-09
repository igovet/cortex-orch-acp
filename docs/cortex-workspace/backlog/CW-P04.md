# CW-P04: No-GMS-device detection and explicit "push unavailable" UX state; documented limitation

**Status:** planned · **Phase:** 3+ · **Size:** S

## Goal
Detect whether Google Play Services and FCM are usable on the phone. Show an explicit "push unavailable" state with the cause, and document the limitation. Foreground and on-open sync keep working.

## Requirements
CW-R09 (push with an explicit unavailable state). See [requirements](../requirements.md).

## Inputs
- [Remote access](../remote-access.md): devices lacking supported services or permission show unavailable and recover through normal reconnect.
- [Risks and open decisions](../risks-and-open-decisions.md): FCM has no delivery SLA; no-GMS support is not assumed.
- CW-P02 (token flow and notification permission).

## Deliverables
- Detection at app start and after app updates: Google Play Services availability, notification permission (Android 13 and later), and token availability.
- A state model with the values `available`, `unavailable_no_play_services`, `permission_denied` and `token_unavailable`. The state appears in settings and in the connection status area.
- A documented limitation in [remote access](../remote-access.md) and [platform policy](../platforms.md), plus matching in-app help text.
- No hidden polling loop is added as a substitute for push.

## Acceptance
**Positive**
- With Play Services present and permission granted, the state is `available` and a token is registered.
- Revoking notification permission shows `permission_denied` at the next app open.

**Negative**
- On a device without Play Services, the state is `unavailable_no_play_services`. No FCM call is made, and the app does not crash.
- The UI never presents an unavailable state as working. No success-looking indicator appears.
- Missing permission is never silently ignored.
- Reject revoked or expired credentials: not applicable to this item; device revocation is covered by CW-N04.

## Dependencies
CW-P02. Owner gates CW-D05 and CW-M03 are mandatory and are satisfied transitively.

## Validation
- Instrumented tests on one device with Play Services and one without. A non-GMS AOSP emulator image counts as emulator evidence only. A physical de-Googled device is needed for physical evidence and may not be available.
- Rendered screenshots of each state on the physical device.
- No check has been run for this planned item.

## Documentation impact
- [Remote access](../remote-access.md): the no-GMS limitation.
- [Platform policy](../platforms.md): Android qualification notes for push.
