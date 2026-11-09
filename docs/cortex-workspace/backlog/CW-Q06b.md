# CW-Q06b: Android pilot checklist and Android qualified-support statement

**Status:** planned · **Phase:** 3+ · **Size:** M

Split from [CW-Q06](CW-Q06.md) for the Android client. This is a mobile qualification item and is gated by Gate M. The desktop pilot is in [CW-Q06a](CW-Q06a.md).

## Goal
Extend the desktop pilot with the owner's Android phone: setup, pairing, push, network changes, revoke and upgrade. Each Android claim links to evidence rows. Each limitation is stated plainly. Anything without evidence is marked unqualified.

## Requirements
CW-R08 (secure enrollment and reconnect without extra installed software), CW-R12 (Android qualification) and CW-R14 (measured evidence). See [requirements](../requirements.md).

## Inputs
- [CW-Q06a](CW-Q06a.md): desktop checklist and qualified-support statement, which this item extends.
- Evidence from CW-Q01b, CW-Q02b, CW-Q03b and CW-Q04b.
- [Platform policy](../platforms.md) and [remote access](../remote-access.md): relay dependence, no-GMS devices and FCM delivery limits.
- Gate M record: [mobile approval](../design/mobile-approval.md).

## Deliverables
- An Android pilot checklist for the owner's phone: install, pairing (CW-N03), push (CW-P02 and CW-P03), network changes (CW-N07), revoke (CW-N04) and an APK upgrade (CW-Q04b).
- An Android addition to the qualified-support statement. It lists the exact Android version and device model, and states the evidence location for each claim.
- A limitation list: relay dependence (direct-connect rates for this product are unmeasured; the relay must be available when direct connections fail), no-GMS devices (CW-P04), FCM with no delivery guarantee, and unqualified platforms.

## Acceptance
**Positive**
- Every Android claim links to an executed evidence row.

**Negative**
- Any claim without evidence is marked unqualified.
- No statement implies universal connectivity or guaranteed push delivery.
- iOS, macOS and Windows are not described as supported.
- Reject stale, unauthorized, missing or conflicting input: a checklist item without its evidence row is incomplete, not passed.

## Dependencies
CW-Q06a, CW-Q01b, CW-Q02b, CW-Q03b, CW-Q04b. Owner gate CW-M03 (Gate M) is mandatory and is satisfied transitively. Owner gate CW-D05 is satisfied transitively.

## Validation
- Local link and dependency scan of the checklist and the support statement.
- Physical-device evidence only. Emulator results are not counted.
- Owner sign-off is recorded by the owner; it is not inferred.
- No check has been run for this planned item.

## Documentation impact
- A qualified-support page in the workspace documentation bundle, extended with the Android section when this item is implemented. It is outside this item's declared scope.
- [Platform policy](../platforms.md): Android qualification status.
