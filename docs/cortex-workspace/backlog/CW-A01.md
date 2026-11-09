# CW-A01: Android app shell and Now summary in React Native

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
Build the Android app shell in React Native with the New Architecture enabled. It contains the Now screen (decisions that need the owner, results ready to review, running work), a task list, project navigation and an away summary of changes since the last open. The layout follows the mobile concept that Gate M selects ([mobile approval](../design/mobile-approval.md); not recorded at audit time). The React Native choice remains provisional until CW-F02 reports.

## Requirements
- CW-R03: decisions are separate from results and reachable from Now.
- CW-R08: enrollment and reconnection work without extra user-installed applications.
- CW-R09: the push-unavailable state is visible.

See [requirements](../requirements.md).

## Inputs
- [Mobile concepts](../design/mobile-concepts.md): Now-first and Task-first navigation, 360×800 and 412×915 logical viewports.
- [Screens](../SCREENS.md) (Android Now, tasks and projects) and [platforms](../platforms.md) (physical-device qualification of lifecycle, keyboard and networks).
- [Performance](../performance.md): Android Now under 2 s cold start and under 5% janky frames.
- [ADR 0003](../adr/0003-tauri-stack.md) and [risks](../risks-and-open-decisions.md): React Native is provisional until the CW-F02 device comparison.

## Deliverables
- React Native shell with New Architecture (Fabric, TurboModules) and navigation among Now, Tasks, Projects and advanced entries.
- Now screen built from typed summaries in three groups: needs the owner, ready to review, running.
- Away summary built from events after the last-seen cursor. Each line opens its task.
- Connection state with the age of the last confirmed state.
- Returning from a detail view preserves task identity and scroll position.

## Acceptance
Positive:
- On the owner's physical Android device, cold start is under 2 s and gfxinfo reports under 5% janky frames during a Now-feed scroll. Device model and Android version are recorded.
- Returning from a task detail to Now restores that task and position.

Negative:
- While disconnected, Now shows the last confirmed state with its age. It never shows "nothing needs you" as the current state.
- Decision actions are disabled while offline, with the reason shown.
- A snapshot with a version older than the cached snapshot is ignored.
- Emulator timings are never reported as device qualification.

## Dependencies
- Gate records: CW-D05 (Gate D) and CW-M03 (Gate M).
- CW-C04 (typed projections), CW-N07 (network-change resilience), CW-F02 (Android feasibility bake-off).

## Validation
- Physical device: `adb shell am start -W <package>/<activity>` for cold start; `adb shell dumpsys gfxinfo <package> framestats` during a scripted scroll.
- Unit tests of the view-model for the three groups, away summary and offline state.
- Rendered captures at 360×800 and 412×915 logical viewports, including large text and keyboard navigation from the mobile concept.
- Record device, build identifier and exact commands. Mark any missing device measurement as not run.

## Documentation impact
- [Screens](../SCREENS.md): Android Now and task-navigation states.
- [Platforms](../platforms.md): Android qualification record once measured.
- [Risks](../risks-and-open-decisions.md): status of the React Native decision.
- [Roadmap](../roadmap.md) when qualification changes.

## Size
L: app shell, navigation, Now feed, away summary and a physical-device performance gate.
