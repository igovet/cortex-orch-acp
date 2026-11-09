# CW-U14: Worker message composer and delivery states

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies · **Phase:** 3+ · **Size:** M

## Goal

Give the owner a composer at the bottom of a running worker's view. It sends text to that worker, shows each message's delivery state in words, and offers the correct recovery action for every undelivered message.

## Requirements

- CW-R29: direct writing to a running worker, with delivery states.
- CW-R32: "delivered" is never shown as "read" or as an action taken.
- CW-R23: the send-mode control is a UI element, not a typed command.

See [requirements](../requirements.md).

## Inputs

- [CW-C12](CW-C12.md): the message contract, its events and its refusals.
- [CW-C13](CW-C13.md): the optional interrupt-now mode. It is shown disabled until an engine is qualified.
- [CW-U13](CW-U13.md): the worker view that hosts the composer.
- [CW-U11](CW-U11.md) and [CW-U15](CW-U15.md): the microphone control in the composer.
- Attachment limits match steering: 10 MiB per block and per message.

## Deliverables

- The composer: "Message <role>…", an attach control, a send-mode chip ("At next step", with "Interrupt now" disabled and its reason shown until [CW-C13](CW-C13.md) is qualified), the microphone control and Send.
- Attachments by drag and drop into the composer and by pasting a copied screenshot ([CW-R39](../requirements.md)).
- The worker's model, effort and access mode shown read-only.
- Entry points: the worker row menu, and a Dialog worker row, which opens the side pane focused on the composer.
- Message states, shown as words: Sending; Pending · next step (Withdraw available until delivery); Delivered at a time; Withdrawn; Not delivered · worker finished (Send to Cortex instead, Discard); Not delivered · stopped, cancelled or replaced (Send to new attempt when one is running, Send to Cortex, Discard); Failed · not sent (text kept, Retry).
- Delivery hint for workers that use native tools: "Delivered when the worker next calls a Cortex tool".
- A pending badge on the worker row.
- A waiting-question state: the placeholder becomes "Answer the worker's question…", and the text is sent as an answer.
- A finished worker: the composer is replaced by "This worker has finished. Ask Cortex about this worker…", which pre-fills the task composer with a worker reference chip.

## Acceptance

Positive:

- A sent message appears in order as Pending. Withdraw before delivery succeeds.
- A message delivered after a tool result shows its delivery time.
- Send to Cortex instead creates a task steer that carries the worker reference.

Negative:

- "Delivered" is never labelled "Read".
- The composer does not offer sending to a finished worker.
- An offline machine disables the composer and shows the reason.
- Withdraw after delivery is refused with a reason, and the state stays Delivered.
- Message text is never parsed as a command.
- A failed send keeps the text in the composer.

## Dependencies

- [CW-C12](CW-C12.md) and [CW-U13](CW-U13.md).
- Gate records: CW-D05 (Gate D) must be recorded before implementation.

## Validation

- View-model tests for the state mapping, including the finished, stopped and offline cases.
- Contract tests on the [CW-C12](CW-C12.md) harness: the withdraw race and the refusals.
- Rendered run at 1280×800 and 1920×1080 showing each message state. Screenshots. Keyboard path to every control.
- Record exact commands, environment and outcomes.

## Documentation impact

- [SCREENS](../SCREENS.md): the worker messages section (already updated with this item).
- [Protocol](../protocol.md), once the message contract is implemented.

## Size

M: one composer, one state model and two entry points.
