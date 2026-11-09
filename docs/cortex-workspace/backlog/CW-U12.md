# CW-U12: Task composer Steer and Queue send modes

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies · **Phase:** 3+ · **Size:** M

## Goal

Give the task composer two explicit send modes, Steer and Queue, with the mode visible before sending. Steer is captured at once and cannot be edited after capture. Queue is held by the client until it is sent, and its items can be edited, removed or reordered until then. Queue survives an app restart once durable queue storage exists.

## Requirements

- CW-R30: two labelled steer modes, with per-machine defaults.
- CW-R23: the modes are UI controls, not typed commands.
- CW-R26: the composer keeps the core session controls.
- CW-R02: the mode and states express intent. They never rewrite a runtime fact.

See [requirements](../requirements.md).

## Inputs

- [CW-C08](CW-C08.md): the steer command and its receipt and lifecycle states.
- [Steering and questions](../../features/steering-and-questions/index.md): steering is durable and applied after the current step; it is not editable after capture.
- Paseo 0.15.0 (read-only inspection of the installed bundle): its composer offers a "Send now" action for queued messages. This item adds Remove and reorder to the queue and states its durability honestly. Paseo's queue behaviour beyond "Send now" is not re-verified here.
- [CW-U02](CW-U02.md) and [CW-U10](CW-U10.md): the task composer that hosts the controls.
- [CW-U07](CW-U07.md): the per-machine default send mode.
- Open decision: key bindings. The proposed default is Enter = Steer and Ctrl/Cmd+Enter = Queue. The owner decides it.

## Deliverables

- A mode chip on Send that names the current mode, with both modes and a one-line explanation of each. The default comes from the machine settings in [CW-U07](CW-U07.md). Shift+Enter inserts a newline.
- Queued items shown above the composer: a preview of the text, the item's attachments, Remove, Send now (which sends the item as Steer) and reorder by drag or Alt+Up/Down. Edit removes the item from the queue and returns its text and attachments to the normal composer, from which it is sent again; the queue never edits an item in place.
- Queue layout: the queue is as wide as the chat column. Its background is transparent at its edges and in the gap between the queue and the composer, so the messages behind it stay visible, and its content is not clipped.
- Edit over a non-empty composer draft offers Replace draft, Keep both or Cancel.
- Steer states: Sending, Received (applies after the current step), Applied (with the plan revision), and Failed (not sent, with Retry).
- The Send button becomes Stop only when the composer is empty. Stop opens the stop dialog of [CW-U08](CW-U08.md).
- Durable queue: queued items are stored in the Workspace local store and sent as an ordinary message when Cortex is idle (waiting for the owner, or finished). Until durable storage exists, the label reads "Queue is kept until the app closes". If durable storage exceeds this item's size, split it into its own item before implementation.
- While a coordinator question is open, the composer shows "Replying to: <question>", and the text is sent as the answer.
- Attachments: a file dropped onto the composer, or a copied screenshot pasted into it, becomes an attachment before sending, as in common chat applications. The attach button stays. Queued items keep their attachments ([CW-R39](../requirements.md)).

## Acceptance

Positive:

- The mode is shown on Send. Enter and Ctrl/Cmd+Enter send and queue as labelled.
- Queued items can be removed and reordered, and Edit returns an item with its text and attachments to the composer. Attachments are shown on each queued item. Send now produces a Steer receipt.
- A Steer bubble advances only on runtime-confirmed states.
- A file dropped onto the composer, and a pasted screenshot, appear as attachment chips before the message is sent.

Negative:

- A captured Steer cannot be edited or removed. The UI offers a correction as a new message only.
- A failed Steer capture restores the text and says nothing reached Cortex. Running work continues.
- Edit over a draft never overwrites it silently.
- A non-durable queue shows its non-durable label and implies no persistence.
- Stop never appears while the composer holds text.
- A Steer to a cancelled task shows the typed refusal from [CW-C08](CW-C08.md).
- When Cortex is idle, the mode chip is hidden because both modes behave the same.

## Dependencies

- [CW-U02](CW-U02.md), [CW-C08](CW-C08.md) and [CW-U07](CW-U07.md) (defaults).
- Gate records: CW-D05 (Gate D) must be recorded before implementation.

## Validation

- View-model tests: mode mapping, queue order, edit conflict, send-now conversion, the durable and non-durable labels, and the Edit round trip (queue item to composer with attachments, then sent).
- Contract tests on the [CW-C08](CW-C08.md) harness: steer receipt, refusal for a cancelled task, duplicate identity.
- Rendered run of the composer and queue in the Ubuntu Tauri 2 shell at 1280×800 and 1920×1080, with screenshots. Keyboard path to every control.
- Record exact commands, environment and outcomes. A check that was not run is recorded as not run.

## Documentation impact

- [SCREENS](../SCREENS.md): the task send modes (updated with this item).
- [Protocol](../protocol.md): the steer receipt states, once implemented.
- [Steering and questions](../../features/steering-and-questions/index.md) only after the behaviour ships.

## Size

M: two modes, a queue list and the state mapping. Durable queue storage may be split out before implementation.
