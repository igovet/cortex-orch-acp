# CW-F02: Android stack go/no-go record (roll-up of CW-F02a and CW-F02b)

**Status:** planned · **Phase:** 3+ · **Size:** S

## Goal

Record one written go/no-go decision for the Android stack, based only on measured results from [CW-F02a](CW-F02a.md) (Tauri 2 Android) and [CW-F02b](CW-F02b.md) (React Native with the New Architecture), both run on the owner's physical Android device. This umbrella item has no build work of its own. The decision chooses between React Native, the provisional choice in [ADR 0003](../adr/0003-tauri-stack.md), and Tauri Android. No Android (CW-A) item starts before the record exists.

## Requirements

CW-R08 (secure enrollment and credential storage without extra applications), CW-R09 (privacy-preserving push from the first release), CW-R12 (Android qualified on a physical device), CW-R14 (startup and jank budgets) and CW-R16 (credentials and device identities protected). See [requirements](../requirements.md).

## Inputs

- [platforms](../platforms.md): Android qualification requires a physical device, including push and lifecycle.
- [remote-access](../remote-access.md): FCM with a metadata-only payload; an explicit "unavailable" state when push services are missing.
- [security](../security.md): device credentials stay in OS-protected storage.
- [performance](../performance.md): Android Now cold start below 2 s; below 5% janky frames, measured with `dumpsys gfxinfo`.
- [risks](../risks-and-open-decisions.md): the Android stack is provisional; push, Keystore and background behavior are unverified.

## Deliverables

- A dated decision section appended to this file. It states the owner's device model and Android version, each check result for both spikes with its record link, the chosen stack or a no-go for both, and the unresolved risks.
- After the owner accepts the record, an update of the status and consequences in [ADR 0003](../adr/0003-tauri-stack.md).

## Acceptance

Positive:

- Both spikes report all four checks (push, Keystore, jank, cold start) on the same physical device, with exact stack versions.
- A go is recorded for one stack only when every check for that stack passed on hardware.

Negative:

- A decision based on emulator results, console-simulated push, or a single spike is rejected.
- A check marked not run or incomplete cannot support a go. The record must name the blocker.
- If both spikes fail, the record states no-go for both. Android production stays blocked and [risks](../risks-and-open-decisions.md) is updated. No CW-A item starts without a recorded decision.

## Dependencies

[CW-F02a](CW-F02a.md) and [CW-F02b](CW-F02b.md). Their prerequisites are listed in those files.

## Validation

- Review of the decision section against the CW-F02a and CW-F02b result records. Each cited command and output must exist and match the record.
- This item has no runtime test of its own.

## Documentation impact

- [ADR 0003](../adr/0003-tauri-stack.md), [platforms](../platforms.md), [risks](../risks-and-open-decisions.md) and [roadmap](../roadmap.md), once the decision is recorded.
