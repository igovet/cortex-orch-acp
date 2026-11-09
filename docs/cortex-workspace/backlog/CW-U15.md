# CW-U15: Machine dictation service

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies; speech provider is an open decision · **Phase:** 3+ · **Size:** L

## Goal

Provide the machine-side speech-to-text service that the composer dictation of [CW-U11](CW-U11.md) uses. It offers a streaming dictation protocol, a local model catalogue with download and removal, an OpenAI-compatible provider, key storage on the machine and a per-machine language default. Audio is processed by the machine that owns the task, unless the owner decides otherwise.

## Requirements

- CW-R31: dictation starts, streams a live transcript and inserts text into an editable draft. Nothing is sent automatically.
- CW-R16: audio and provider credentials are protected. No audio leaves the machine without a provider the owner selected.
- CW-R17: the service belongs to a machine. A remote machine's service is reached through the server connector.
- CW-R26: voice input is a core composer control.

See [requirements](../requirements.md).

## Inputs

- [CW-U11](CW-U11.md) (the composer control), [CW-U07](CW-U07.md) (per-machine settings), [CW-H05](CW-H05.md) (machine registry and connector) and [Security](../security.md) (credential and data-flow rules).
- Observed in the installed Paseo 0.15.0 app (read-only inspection of its bundle, not a dependency): a host-side speech service with a local sherpa-onnx model catalogue (including GigaAM v3 for Russian and Parakeet v3 for mixed European languages), an OpenAI provider (whisper-1, gpt-4o-transcribe and gpt-4o-mini-transcribe) with an optional OpenAI-compatible base URL, and a stream of start, chunk, finish and cancel requests that returns ack, partial, final and error events. Errors carry a retryable flag, a reason code and the missing model identities. Model sizes and download times are not measured.
- Open decisions: the default provider (the recommendation is local), and where a remote machine's task is transcribed (the recommendation is the local machine). Both must be decided before implementation.

## Deliverables

- A dictation stream protocol: start, chunk, finish and cancel requests, and ack, partial, final and error events. Errors carry a retryable flag, a reason code and the missing model identities.
- A local provider behind a port: a model catalogue with size, download progress, removal, and a "model not downloaded" error.
- An OpenAI-compatible provider: the key is stored on the machine and is never returned. The base URL is configurable.
- Per-machine provider, model and language defaults, set through [CW-U07](CW-U07.md).
- Audio is held in memory only while it streams. It is not stored by default.
- A finalize timeout that returns an error and leaves the composer text unchanged. The Paseo reference value is five minutes; Cortex sets its own value.

## Acceptance

Positive:

- The local provider turns a fixture utterance into partial and final text.
- Download progress shows the size and the completion state.
- The per-machine language default applies to new recordings.

Negative:

- A missing model returns a typed error with a Download action. No cloud provider is used as a silent fallback.
- The key never appears in any response, log, report or diagnostic.
- A finalize timeout keeps the composer text unchanged.
- When the machine's service is offline, the composer shows "This machine's voice service is offline", and no audio is sent elsewhere.
- Audio is never written to disk by default.

## Dependencies

- [CW-U07](CW-U07.md), [CW-H05](CW-H05.md) and [CW-C01](CW-C01.md) (the versioned contract).
- Gate records: CW-D05 (Gate D). Open decisions on the default provider and the transcription location must be recorded.

## Validation

- Fake-stream tests: start, chunk, partial, final, error, cancel, and finalize timeout.
- Key redaction test across responses, logs and diagnostics.
- Local provider run on fixture audio: only after the default provider is decided; not run until then.
- A network capture showing that the local provider sends no audio off the machine. Required by [CW-U11](CW-U11.md); not run until the provider is implemented.
- The OpenAI-compatible provider is not called in any check here. A real call is paid and needs an owner request. Recorded as not run.

## Documentation impact

- [Security](../security.md): the audio data flow and the key storage rules, once the provider is chosen.
- [SCREENS](../SCREENS.md) and [CW-U11](CW-U11.md): the voice states.

## Size

L: a streaming protocol, a model catalogue with downloads, two providers and key storage. Split before implementation if effort exceeds L.
