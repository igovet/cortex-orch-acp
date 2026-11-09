# CW-U11: Voice input for the task composer

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies; speech provider is an open question · **Phase:** 3+ · **Size:** M

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
- [CW-U07](CW-U07.md): per-machine settings, which hold the provider choice and the transcription language.
- [Security](../security.md): credential and data handling rules.
- Open decision (blocks implementation): **which speech provider is the default?** Paseo 0.15.0 offers the first two options as provider choices. The owner decides the default at design agreement:
  1. On-device recognition, with no audio sent off the machine. Paseo's local models are sherpa-onnx models, including GigaAM v3 for Russian and Parakeet v3 for mixed Russian and English. The recommended default is local.
  2. An OpenAI-compatible speech service chosen by the owner, with its key stored on the machine under CW-R16 and never displayed again.
  3. A platform speech service through the Tauri webview. Its availability on the Ubuntu WebKitGTK target is not verified and must be checked before it is considered.
- Further open points, to decide with the provider: whether `Auto` language detection is offered (Paseo shows an Auto-detect placeholder and a per-recording language choice); whether press-and-hold is offered alongside the toggle (recommended: both); and whether audio is retained (proposal: not retained).
- The speech service, its model catalogue and its settings are specified in [CW-U15](CW-U15.md). Where the transcription of a remote machine's task runs is an open decision (recommended: on the local machine).

## Deliverables

- Microphone control in every composer (task, worker, sprint planning, Request changes, diff comments and New task description), with states: idle, listening, processing, unavailable (no provider, no downloaded model or no microphone), permission denied, error and interrupted.
- Recording bar in place of the composer toolbar, with elapsed time, a level meter (static under reduced motion), a language choice, Cancel (Esc), Insert (Enter) and Insert and send (button only).
- Live partial transcript shown in grey at the cursor in the draft; it becomes normal text on Insert.
- Recognised text inserted at the cursor in the composer. Nothing is sent until the owner presses send.
- Provider behind a port, so the provider decision can change without changing the composer.
- Per-machine provider and language settings through CW-U07 and the speech service of CW-U15.

## Acceptance

Positive:

- With a provider selected and a microphone available, dictated text appears in the composer and nothing is sent until the owner sends it.
- The transcription language follows the owner's language setting, or the behaviour agreed for Auto.
- Ctrl/Cmd+D starts and stops dictation in a focused composer; it is not bound while the terminal has focus. Esc cancels an active recording.

Negative:

- With no provider selected, the control reads "Voice input unavailable" with its reason, and no recording starts.
- Microphone permission denial shows an explicit denied state; the app does not retry in a loop.
- Audio is never sent to a provider the owner has not selected, and there is no silent fallback to another service.
- A transcription error or timeout leaves the composer text unchanged.
- Voice input never sends a message or applies a configuration change by itself.
- Dictation never starts without a user gesture, and the recording indicator stays visible for as long as audio is captured.
- Insert never sends a message. Only the explicit Insert and send button does, using the composer's current send mode.

## Dependencies

- [CW-U10](CW-U10.md), [CW-U07](CW-U07.md) and [CW-U15](CW-U15.md) (machine speech service).
- Gate records: CW-D05 (Gate D) must be recorded before implementation.
- The open speech-provider question must be answered before this item is started.

## Validation

- Adapter contract tests with a fake provider: success, error, timeout, and permission denied.
- Microphone access in the Ubuntu Tauri 2 shell (WebKitGTK `getUserMedia`) is unverified. Check it in the CW-F01 shell before choosing the desktop path, and record it as not run until then.
- Rendered run of the composer in the Ubuntu Tauri 2 shell (CW-F01) showing the unavailable, denied and recording states, with screenshots at 1280×800 and 1920×1080.
- Real speech recognition with the chosen provider: run only after the provider is decided; until then it is recorded as not run.
- For the chosen provider, a recorded statement of where audio is sent, or a network capture showing that no audio leaves the machine when on-device recognition is chosen.
- Record exact commands, environment and outcomes.

## Documentation impact

- [SCREENS](../SCREENS.md): voice-input states in the composer.
- [Security](../security.md): audio handling and provider data flow, once the provider is chosen.
- [Requirements](../requirements.md) and [roadmap](../roadmap.md) when the provider decision changes qualification status.

## Size

M: one composer control, one provider port and a state model. Split before implementation if the chosen provider adds more than M of effort.
