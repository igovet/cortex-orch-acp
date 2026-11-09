# CW-C12: Durable worker message capture and delivery

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies · **Phase:** 3+ · **Size:** L

## Goal

Capture text that the owner writes to one running worker as a durable user source bound to that attempt. Deliver it at the worker's next Cortex tool result or, if the turn ends first, in the next prompt of the same session. Expose each message's lifecycle (accepted, pending, delivered, withdrawn, undelivered) through the public contract. A message is guidance only: it grants no scope, permission, review or acceptance change.

## Requirements

- CW-R29: direct writing to a running worker, with visible delivery states.
- CW-R32: "delivered" means recorded in a tool result or prompt, never that the model read or acted on it.
- CW-R04: request identity, expected attempt identity and server-side authorization.
- CW-R07: message state is part of snapshots and events; a gap resynchronizes.
- CW-R15: exposed through the public structured adapter, not the generic ACP option list.

See [requirements](../requirements.md).

## Inputs

- Precedent: the command-stop reason is delivered inside a gateway tool result (`src/cortex_runtime/orchestration/command_control.py`, the "Stopped by user" message).
- Existing user steering is a durable event keyed by message identity and requirement revision ([steering and questions](../../features/steering-and-questions/index.md)). `record_steering` (`src/cortex_runtime/acp/steering.py`) records task-level steering for the coordinator; it does not route text to a worker.
- The event enums already name a `coordinator_clarification` type (`src/cortex_runtime/delegation/events.py`, `src/cortex_runtime/delegation/events_store.py`), and the recovery capsule reads it (`src/cortex_runtime/recovery/capsule.py`). No producer exists in `src/`. Reusing it is a design decision to record in [protocol](../protocol.md); a dedicated worker-message event type is the default.
- Engine limit: native engine tools (read, edit, shell) are not mediated by the runtime. A worker that uses only native tools receives a message at its next Cortex tool call, and the UI must say so.
- [CW-C08](CW-C08.md) (steer command shape), [CW-C03](CW-C03.md) (idempotency), [CW-C06](CW-C06.md) (stable execution identities).

## Deliverables

- A structured command for worker messages, with its name fixed in [protocol](../protocol.md). It carries request identity, task, execution, attempt, message identity, text and attachments. Limits match steering: 10 MiB per block and per message.
- Events: accepted, delivered (with the delivery point: the gateway tool result or the prompt), undelivered (with a reason) and withdrawn.
- Delivery: the owner's text is appended verbatim to the next gateway tool result of that attempt, inside a runtime-owned framing that says it is guidance. Several pending messages are delivered together, in captured order.
- Withdraw: atomic. It succeeds only while the message is undelivered.
- Typed refusals: attempt terminal or cancelled, task stopped, attempt replaced, channel revoked, too large, invalid.
- Propagation: the message appears in the attempt history, the recovery capsule, the coordinator's next planning context (as a source bound to the worker, without forcing a replan) and reviewer evidence.
- Message text is never parsed for directives.

## Acceptance

Positive:

- An accepted message is shown as pending with its receipt. The next gateway tool result of that attempt carries it, and the delivered event records the delivery point.
- Withdraw before delivery succeeds, and the message is never delivered afterwards.
- Two pending messages are delivered together in captured order.
- A worker whose turn ends before any Cortex tool call receives the message in the next prompt of the same session.

Negative:

- A message to a terminal, cancelled or replaced attempt is refused with a typed reason. It is not stored silently.
- The same message identity with a different payload is rejected. The same identity with the same payload returns the existing receipt.
- Withdraw after delivery is refused with a typed reason, and the delivered record is unchanged.
- A message changes no scope, permission, review or acceptance state, and is not parsed as an instruction to change them.
- "Delivered" is never reported as read or acted upon.
- A revoked channel refuses the command and changes nothing on the attempt.
- After a crash or reconciliation, a pending message is either delivered or shown as undelivered. It is never dropped silently.

## Dependencies

- [CW-C03](CW-C03.md), [CW-C06](CW-C06.md) and [CW-C08](CW-C08.md).
- Gate records: CW-D05 (Gate D) must be recorded before implementation.

## Validation

- `worker-message-delivery-point`: delivered at the next gateway tool result; the delivery point is recorded.
- `worker-message-withdraw-race`: withdraw issued concurrently with delivery yields exactly one outcome.
- `worker-message-terminal-refused`: typed refusal for a terminal attempt.
- `worker-message-duplicate-identity`: same identity with a changed payload is rejected.
- `worker-message-native-only`: a worker that uses only native tools receives the message at its next Cortex call; while idle the message stays pending.
- `worker-message-crash-reconcile`: a host kill during delivery leaves the message delivered or undelivered, never dropped (see [CW-H04](CW-H04.md)).
- `worker-message-capsule`: the message is present in the recovery capsule.
- Use the fake-ACP harness. No paid engine call is part of these checks.

## Documentation impact

- [Protocol](../protocol.md): the command table and the event types.
- [Steering and questions](../../features/steering-and-questions/index.md) and [failures and cancellation](../../features/failures-and-cancellation/index.md) only after the contract ships. Do not describe the planned capability as shipped.

## Size

L: capture, delivery at the tool boundary, the withdraw race, and propagation to the capsule, the planning context and review evidence. Split before implementation if effort exceeds L.
