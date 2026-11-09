# CW-C13: Interrupt-now worker message per engine

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies; engine capability unverified · **Phase:** 3+ · **Size:** M

## Goal

Offer an optional "interrupt now" delivery for a worker message. It cancels the worker's current engine turn and resumes the same session with the message. The current execution and its running gateway commands are not cancelled. The control is enabled per engine, and only after that engine is qualified for it. Interrupt is not a pause: [CW-C08](CW-C08.md) does not treat ACP turn cancellation as a safe pause.

## Requirements

- CW-R29: interrupt is a separate capability, enabled only for an engine qualified for it.
- CW-R15: engine capabilities are used only when negotiated.
- CW-R02: the action expresses intent and does not rewrite a runtime fact.

See [requirements](../requirements.md).

## Inputs

- [CW-C12](CW-C12.md): the message delivery contract and its states.
- [CW-C08](CW-C08.md): turn cancellation is not a safe pause.
- [Failures and cancellation](../../features/failures-and-cancellation/index.md) and [steering and questions](../../features/steering-and-questions/index.md): current turn-cancellation and resume semantics. Verify them in source before writing the contract.
- Engine pages: [Claude engine](../../features/claude-engine/index.md), [Codex engine](../../features/codex-engine/index.md) and [OpenCode engine](../../features/opencode-engine/index.md). Whether the adapter can accept a mid-turn user message, or can resume a session after cancel with an injected prompt, is unverified for each engine.

## Deliverables

- A per-engine capability flag, negotiated at session start. The control is enabled only when the flag is set and a qualification record exists for that engine.
- The interrupt sequence: cancel only the engine turn, keep the current execution and gateway commands running, resume the same session with the message as the next prompt, and record one event with the delivery point.
- Typed outcomes: accepted, delivered, refused (not supported for this engine) and failed. A failed resume leaves the message pending or moves it to undelivered, as CW-C12 defines.
- A qualification probe on the fake-ACP harness. Live per-engine checks run only when the owner requests them.

## Acceptance

Positive:

- On a qualified engine, the worker's turn resumes in the same session with the message. Gateway commands that were running keep running. The message shows as delivered with the interrupt delivery point.

Negative:

- On an engine without a qualification record, the control is disabled with the reason "Not available for this engine yet", and no turn is cancelled.
- A failed resume does not cancel the current execution and does not mark the attempt failed silently. The message is shown as undelivered.
- Interrupt is never listed as a pause in the capability list.

## Dependencies

- [CW-C12](CW-C12.md) and [CW-C08](CW-C08.md).
- Gate records: CW-D05 (Gate D). The engine qualification records are not started.

## Validation

- `interrupt-capability-gate`: the control is disabled when the engine flag is absent (fake ACP).
- `interrupt-keeps-commands`: a running gateway command survives the interrupt.
- `interrupt-resume-failure`: a failed resume yields undelivered, and the execution continues.
- Live per-engine qualification: not run. Blocker: "paid live check not requested by the owner".

## Documentation impact

- Engine feature pages and [protocol](../protocol.md), only after qualification. No shipped claim is made now.

## Size

M: one capability flag per engine, the interrupt sequence and a fake-ACP harness. Qualification may add effort; split if it exceeds M.
