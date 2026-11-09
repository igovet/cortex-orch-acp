# CW-Q03b: Physical-Android accessibility and long-history checks

**Status:** planned · **Phase:** 3+ · **Size:** L

Split from [CW-Q03](CW-Q03.md) for the physical Android device.

## Goal
On the owner's physical Android device, verify that the Now and task screens are usable with TalkBack, large text and a hardware keyboard. Measure startup and jank budgets with a long history loaded.

## Requirements
CW-R12 (Android is a first qualified platform) and CW-R14 (responsiveness and accessibility measured against budgets). See [requirements](../requirements.md).

## Inputs
- [Performance targets](../performance.md): Android Now cold start under 2 s and under 5% janky frames.
- [Platform policy](../platforms.md): Android requires physical-device qualification covering keyboard, lifecycle, networks and push.
- CW-T02 (advanced Android terminal attachment, which has keyboard requirements) and CW-Q02b (phone and device fault rows).

## Deliverables
- A TalkBack checklist for the Now screen, decision cards, task screens and the result screen.
- A large-text and hardware-keyboard checklist.
- A startup measurement (`adb shell am start -W`) and a frame-timing measurement (`adb shell dumpsys gfxinfo <package>`) after a 10,000-event history is loaded.
- Device model, Android version, build identifier and dataset recorded with every value.

## Acceptance
**Positive**
- TalkBack reads each decision card with its actions and state.
- Large text causes no clipping on the checked screens.
- Cold start and jank values are recorded and compared with the targets.

**Negative**
- Without a physical device, the item is unverified. Emulator results are not counted.
- A missing or wrong TalkBack label fails its row.
- Janky frames above 5% fail the budget.
- The long-history list stays bounded. An unbounded list fails.
- Reject stale, unauthorized, missing or conflicting input: a missing gfxinfo capture is reported as not measured.

## Dependencies
CW-Q02b, CW-T02. Owner gates CW-D05 and CW-M03 are mandatory and are satisfied transitively.

## Validation
- Physical-device run with the device model and Android version recorded. Screenshots and the `adb` outputs are retained.
- No check has been run for this planned item.

## Documentation impact
- [Performance targets](../performance.md): Android measurement method.
- [Platform policy](../platforms.md): Android accessibility qualification record.
