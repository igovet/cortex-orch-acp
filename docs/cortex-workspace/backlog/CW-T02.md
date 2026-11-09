# CW-T02: Android advanced terminal attachment

**Status:** planned · **Phase:** 3+ · **Size:** M

## Goal
Provide an advanced Android screen that attaches to an existing service PTY to view output and send input, with keyboard and accessibility handling. As [screens](../SCREENS.md) specifies, the terminal is secondary: it is not on the Android home screen. Closing the view detaches and never stops the session.

## Requirements
- CW-R10: the terminal surface is bounded and authorized.
- CW-R06: the session belongs to the service and is not owned by the phone view.
- CW-R14: keyboard and accessibility are measured on a physical device.

See [requirements](../requirements.md).

## Inputs
- [Screens](../SCREENS.md): the advanced terminal is accessible but secondary.
- [CW-T01](CW-T01.md) (service PTY sessions and retained tail).
- [CW-A02](CW-A02.md) (Android app shell and synchronization).
- [Platforms](../platforms.md): keyboard, lifecycle and accessibility qualification on the physical device.

## Deliverables
- Attach list of sessions from the service.
- Terminal view that starts from the retained output tail and receives live output.
- Soft-keyboard handling, and a detach action.
- A read-only state for sessions that cannot accept input.

## Acceptance
Positive:
- A terminal attached on Android shows the same retained output tail as the desktop view at the time of attach.
- Closing the Android view detaches the session. The PTY keeps running and stays visible on the desktop.

Negative:
- The terminal is not reachable from the Now home screen. It is reached only through advanced navigation.
- Input to a lost or read-only session is refused, with the state shown.
- Detaching never sends a stop or interrupt signal.
- On the physical device, with the soft keyboard open, the latest output line stays visible.

## Dependencies
- [CW-T01](CW-T01.md), [CW-A02](CW-A02.md).
- Gate records: CW-D05 (Gate D) and CW-M03 (Gate M).

## Validation
- Physical Android device with TalkBack and the soft keyboard.
- Desktop cross-check: the same session's output and state are compared at attach and after detach.
- Screenshots with the keyboard open.
- Record device, Android version, exact commands and outcomes.

## Documentation impact
- [Screens](../SCREENS.md) and [platforms](../platforms.md) once qualified on the device.
- [Roadmap](../roadmap.md) when qualification changes.

## Size
M: an attachment client on top of the T01 session contract. The PTY lifecycle is not in this item.
