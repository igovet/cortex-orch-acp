# CW-S10: Connect the first owner journey and hide every unsupported control

**Status:** built; native journey passes in the container build with fake engines · **Phase:** 3+ · **Size:** L

## Goal

Connect the promoted project, new-task, session, question and result screens to the real service for one owner journey: register a project, activate Cortex, save a description-only draft, start it, watch observed progress, answer a named question, and read the runtime result. Everything else stays hidden or unavailable until it is connected individually.

## Requirements

CW-R13 (accepted direction), CW-R15 (explicit activation and the public adapter) and CW-R24 (description-first creation). See [requirements](../requirements.md).

## Inputs

- [CW-S06](CW-S06.md), [CW-S07](CW-S07.md), [CW-S08](CW-S08.md) and [CW-S09](CW-S09.md).
- [SCREENS](../SCREENS.md) and [desktop approval](../design/desktop-approval.md).

## Deliverables

- Wired screens for the journey only.
- Hidden or disabled controls for sprints, remote machines, file editing, terminal, Git merge and pull request, archive and purge, configuration management, worker messaging and steer, and quota graphs.
- Model, effort, access mode and language controls that use only the options the session advertises. Unsupported selectors are unavailable, not defaulted.
- Context, quota and voice remain unavailable and are never fabricated.
- Connection age and the last-confirmed time are shown for every displayed record.

## Acceptance

Positive:

- The full journey completes against a fake engine through the real Python runtime.
- Closing and reopening the window during a question, and during streaming, keeps the state and allows the answer.

Negative:

- No silent fallback to demo data when the service is unavailable.
- Disconnected data shows its age; it is never presented as current.
- Runtime results are never labelled accepted.

## Dependencies

[CW-S06](CW-S06.md), [CW-S07](CW-S07.md), [CW-S08](CW-S08.md) and [CW-S09](CW-S09.md).

## Validation

- Vitest for the view models; a scripted fake peer for the screens; then the real Python runtime backed by fake engines. No paid call.
- Rendered WebKitGTK walk-through on Ubuntu, recorded as structured outcomes. Browser results do not qualify it.

## Documentation impact

- [SCREENS](../SCREENS.md), [DESIGN](../DESIGN.md), [protocol](../protocol.md) and [roadmap](../roadmap.md).
