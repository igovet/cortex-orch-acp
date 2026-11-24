# CW-U11: Voice input for the task composer

**Status:** Partly implemented: local dictation in the composer with a Russian-only on-device recognizer ([maintenance and controls](../maintenance-and-controls.md)); no provider choice; a real microphone is not verified · **Phase:** 3+ · **Size:** M

## Goal

Add voice input to the task composer: a control starts dictation and places the recognised text in the composer for the owner to review and send. Voice input is a new capability for Cortex. A check on 2026-11-04 found no voice, speech or dictation implementation in Cortex source (`src/`) or documentation (`docs/`). Paseo 0.15.0 has composer dictation (a microphone button, Ctrl/Cmd+D, a live transcript, Insert and Insert and send), and it is the reference for [CW-R31](../requirements.md). The only "voice-mute" references in Cortex docs, in [observability](../../features/observability/index.md) and [gotchas](../../project/gotchas.md), concern a Paseo window shortcut. They are not about input and say nothing either way about Paseo dictation.

## Requirements

- CW-R26: voice input is a core composer control that must be preserved in the desktop app.
- CW-R31: dictation follows the Paseo reference: start and stop, a live transcript, insertion into an editable draft, and an explicit Insert and send.
- CW-R23: the control is part of the UI, not a typed command.
- CW-R16: audio and any provider credentials are protected; no audio leaves the machine without a provider the owner has selected.
- CW-R14: the control meets the keyboard and accessibility checks in the testing plan.

See [requirements](../requirements.md).

## Inputs

- [CW-U10](CW-U10.md): the composer that hosts the control.
- [CW-U07](CW-U07.md): per-machine settings, which hold the transcription language.
- [Security](../security.md): credential and data handling rules.
- **Decision:** Keep on-device sherpa-onnx recognition. Retain the Russian model, add a separate English-only model, and select Russian (RU) or English (EN) in Settings → Voice input. There is no automatic language detection or provider switch. Audio is not retained or sent off the machine. See [outgoing engine audio](../../features/input-history-and-replay/index.md#outgoing-engine-audio-2026-11-23).
- Press-and-hold alongside the existing toggle remains an open interaction decision.
- The speech service, its model catalogue and its settings are specified in [CW-U15](CW-U15.md). Where the transcription of a remote machine's task runs is an open decision (recommended: on the local machine).

## Deliverables

- Microphone control in every composer (task, worker, sprint planning, Request changes, diff comments and New task description), with states: idle, listening, processing, unavailable (selected model not downloaded or no microphone), permission denied, error and interrupted.
- Recording bar in place of the composer toolbar, with elapsed time, a level meter (static under reduced motion), Cancel (Esc), Insert (Enter) and Insert and send (button only).
- Live partial transcript shown in grey at the cursor in the draft; it becomes normal text on Insert.
- Recognised text inserted at the cursor in the composer. Nothing is sent until the owner presses send.
- Keep the speech provider behind a port so the composer stays independent of the recognizer.
- Per-machine RU/EN language setting through CW-U07, with separate local models managed by the speech service of CW-U15.

## Acceptance

Positive:

- With the selected language model downloaded and a microphone available, dictated text appears in the composer and nothing is sent until the owner sends it.
- Dictation uses the Russian or English model selected in Settings → Voice input.
- Ctrl/Cmd+D starts and stops dictation in a focused composer; it is not bound while the terminal has focus. Esc cancels an active recording.

Negative:

- If the selected language model is missing, show the model-not-downloaded state with a Download action; do not start recording without the model.
- Microphone permission denial shows an explicit denied state; the app does not retry in a loop.
- Audio is never uploaded, and there is no fallback to another model or service.
- A transcription error or timeout leaves the composer text unchanged.
- Voice input never sends a message or applies a configuration change by itself.
- Dictation never starts without a user gesture, and the recording indicator stays visible for as long as audio is captured.
- Insert never sends a message. Only the explicit Insert and send button does, using the composer's current send mode.

## Dependencies

- [CW-U10](CW-U10.md), [CW-U07](CW-U07.md) and [CW-U15](CW-U15.md) (machine speech service).
- Gate records: CW-D05 (Gate D) must be recorded before implementation.

## Validation

- Adapter contract tests with a fake provider: success, error, timeout, and permission denied.
- Microphone access in the Ubuntu Tauri 2 shell (WebKitGTK `getUserMedia`) is unverified. Check it in the CW-F01 shell before choosing the desktop path, and record it as not run until then.
- Rendered run of the composer in the Ubuntu Tauri 2 shell (CW-F01) showing the unavailable, denied and recording states, with screenshots at 1280×800 and 1920×1080.
- Real speech recognition with the local Russian and English models remains to be verified.
- Record evidence that microphone audio stays on the machine.
- Record exact commands, environment and outcomes.

## Documentation impact

- [SCREENS](../SCREENS.md): voice-input states in the composer.
- [Security](../security.md): on-device audio handling and data flow.
- [Requirements](../requirements.md) and [roadmap](../roadmap.md) when the implementation or qualification status changes.

## Size

M: one composer control, one provider port and a state model. Split before implementation if the chosen provider adds more than M of effort.
